use crate::geoip::GeoDb;
use crate::models::ServiceNode;
use dashmap::DashMap;
use sqlx::SqlitePool;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub jwt_secret: String,
    pub geoip: Arc<GeoDb>,
    pub listeners: Arc<DashMap<String, ListenerHandle>>,
    pub rr_index: Arc<DashMap<String, AtomicU64>>,
    pub sticky: Arc<DashMap<String, StickyEntry>>,
    pub traffic_tx: mpsc::Sender<TrafficEvent>,
    pub bind_errors: Arc<DashMap<String, String>>,
}

pub struct ListenerHandle {
    pub abort: broadcast::Sender<()>,
    pub task: JoinHandle<()>,
}

#[derive(Clone, Debug)]
pub struct StickyEntry {
    pub node_id: String,
    pub expire_ts: i64,
}

#[derive(Clone, Debug)]
pub struct TrafficEvent {
    pub service_id: String,
    pub client_ip: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

impl AppState {
    pub fn new(
        db: SqlitePool,
        jwt_secret: String,
        geoip: Arc<GeoDb>,
        traffic_tx: mpsc::Sender<TrafficEvent>,
    ) -> Self {
        Self {
            db,
            jwt_secret,
            geoip,
            listeners: Arc::new(DashMap::new()),
            rr_index: Arc::new(DashMap::new()),
            sticky: Arc::new(DashMap::new()),
            traffic_tx,
            bind_errors: Arc::new(DashMap::new()),
        }
    }
}

pub async fn load_service(db: &SqlitePool, id: &str) -> sqlx::Result<Option<ServiceNode>> {
    sqlx::query_as::<_, ServiceNode>("SELECT * FROM service_nodes WHERE id=?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn load_enabled_services(db: &SqlitePool) -> sqlx::Result<Vec<ServiceNode>> {
    sqlx::query_as::<_, ServiceNode>("SELECT * FROM service_nodes WHERE enabled=1")
        .fetch_all(db)
        .await
}


