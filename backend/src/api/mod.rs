pub mod dashboard;
pub mod logs;
pub mod nodes;
pub mod policies;
pub mod pools;
pub mod services;
pub mod settings;

use crate::auth::{make_token, AuthUser};
use crate::error::{ApiResult, AppError};
use crate::models::LoginReq;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/auth/me", get(me))
        .merge(settings::router())
        .merge(policies::router())
        .merge(pools::router())
        .merge(nodes::router())
        .merge(services::router())
        .merge(dashboard::router())
        .merge(logs::router());

    Router::new()
        .route("/auth/login", post(login))
        .merge(authed)
        .with_state(state)
}

async fn login(State(state): State<AppState>, Json(req): Json<LoginReq>) -> ApiResult<Json<Value>> {
    let hash = crate::db::get_setting(&state.db, "admin_password")
        .await?
        .ok_or_else(|| AppError::internal("未初始化管理员密码"))?;
    if !bcrypt::verify(&req.password, &hash).unwrap_or(false) {
        return Err(AppError::unauthorized("密码错误"));
    }
    let token = make_token(&state.jwt_secret).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Json(json!({ "token": token, "username": "admin" })))
}

async fn me(_user: AuthUser) -> Json<Value> {
    Json(json!({ "username": "admin" }))
}
