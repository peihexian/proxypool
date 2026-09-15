mod api;
mod auth;
mod db;
mod error;
mod geoip;
mod models;
mod parser;
mod proxy;
mod state;
mod worker;

use crate::geoip::GeoDb;
use crate::state::{AppState, TrafficEvent};
use anyhow::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let db = db::init_db(&data_dir).await?;
    let jwt_secret = db::get_setting(&db, "jwt_secret")
        .await?
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let geoip = Arc::new(GeoDb::new(data_dir.join("geoip")));
    let (traffic_tx, traffic_rx) = mpsc::channel::<TrafficEvent>(4096);
    let state = AppState::new(db, jwt_secret, geoip, traffic_tx);

    worker::spawn_all(state.clone(), traffic_rx);
    proxy::start_all(&state).await;

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api_router = api::router(state.clone()).layer(cors);

    let frontend_dir = frontend_dist();
    let app = if frontend_dir.join("index.html").exists() {
        let index = frontend_dir.join("index.html");
        axum::Router::new()
            .nest("/api", api_router)
            .fallback_service(
                ServeDir::new(&frontend_dir).not_found_service(ServeFile::new(index)),
            )
    } else {
        axum::Router::new().nest("/api", api_router)
    };

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("MyProxy 管理端 http://0.0.0.0:{port}");
    tracing::info!("默认管理员密码: admin");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("MYPROXY_DATA") {
        return PathBuf::from(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join("data")
}

fn frontend_dist() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = [
        cwd.join("../frontend/dist"),
        cwd.join("frontend/dist"),
        cwd.join("dist"),
    ];
    for p in candidates {
        if p.join("index.html").exists() {
            return p;
        }
    }
    cwd.join("../frontend/dist")
}
