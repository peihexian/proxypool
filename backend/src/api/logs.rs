use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::{AppState, TrafficEvent};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new().route("/logs", get(list_logs))
}

async fn list_logs(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Value>> {
    let mut items = state.snapshot_usage_logs();
    if items.is_empty() {
        items = crate::worker::traffic::load_recent(&state.db).await;
        if !items.is_empty() {
            state.seed_usage_logs(items.clone());
        }
    }

    let names: Vec<(String, String)> = sqlx::query_as("SELECT id, name FROM service_nodes")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let name_map: HashMap<String, String> = names.into_iter().collect();

    let items: Vec<Value> = items
        .into_iter()
        .map(|e: TrafficEvent| {
            json!({
                "ts": e.ts,
                "service_id": e.service_id,
                "service_name": name_map.get(&e.service_id).cloned().unwrap_or_default(),
                "client_ip": e.client_ip,
                "proxy_ip": e.proxy_ip,
                "dest": e.dest,
                "protocol": e.protocol,
                "bytes_up": e.bytes_up,
                "bytes_down": e.bytes_down,
            })
        })
        .collect();

    Ok(Json(json!({ "items": items })))
}
