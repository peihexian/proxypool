use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new().route("/dashboard", get(overview))
}

async fn overview(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Value>> {
    let (total_nodes,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM proxy_nodes")
        .fetch_one(&state.db)
        .await?;
    let (active_nodes,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proxy_nodes WHERE status='active'")
            .fetch_one(&state.db)
            .await?;
    let (isolated_nodes,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proxy_nodes WHERE status='isolated'")
            .fetch_one(&state.db)
            .await?;
    let (pending_nodes,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM proxy_nodes WHERE status='pending'")
            .fetch_one(&state.db)
            .await?;
    let (pool_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM node_pools")
        .fetch_one(&state.db)
        .await?;
    let (service_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM service_nodes WHERE enabled=1")
            .fetch_one(&state.db)
            .await?;

    let (total_up, total_down, total_requests): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(bytes_up),0), COALESCE(SUM(bytes_down),0), COUNT(*) FROM traffic_logs",
    )
    .fetch_one(&state.db)
    .await?;

    let since = chrono::Utc::now().timestamp() - 8 * 3600;
    let hourly: Vec<(i64, i64, i64)> = sqlx::query_as(
        r#"SELECT (ts / 3600) * 3600 as bucket,
                  COALESCE(SUM(bytes_up),0),
                  COALESCE(SUM(bytes_down),0)
           FROM traffic_logs WHERE ts >= ?
           GROUP BY bucket ORDER BY bucket"#,
    )
    .bind(since)
    .fetch_all(&state.db)
    .await?;

    let mut hours = Vec::new();
    let now_hour = (chrono::Utc::now().timestamp() / 3600) * 3600;
    for i in 0..8 {
        let ts = now_hour - (7 - i) * 3600;
        let found = hourly.iter().find(|r| r.0 == ts);
        hours.push(json!({
            "ts": ts,
            "label": chrono::DateTime::from_timestamp(ts, 0)
                .map(|d| d.format("%H:00").to_string())
                .unwrap_or_default(),
            "up": found.map(|f| f.1).unwrap_or(0),
            "down": found.map(|f| f.2).unwrap_or(0),
            "total": found.map(|f| f.1 + f.2).unwrap_or(0),
        }));
    }

    let top: Vec<(String, i64, i64)> = sqlx::query_as(
        r#"SELECT client_ip, COALESCE(SUM(bytes_up),0), COALESCE(SUM(bytes_down),0)
           FROM traffic_logs WHERE ts >= ?
           GROUP BY client_ip
           ORDER BY (COALESCE(SUM(bytes_up),0)+COALESCE(SUM(bytes_down),0)) DESC
           LIMIT 5"#,
    )
    .bind(since)
    .fetch_all(&state.db)
    .await?;
    let top5: Vec<Value> = top
        .into_iter()
        .map(|(ip, up, down)| {
            json!({
                "client_ip": ip,
                "up": up,
                "down": down,
                "total": up + down
            })
        })
        .collect();

    Ok(Json(json!({
        "pools": pool_count,
        "services": service_count,
        "total_nodes": total_nodes,
        "active_nodes": active_nodes,
        "isolated_nodes": isolated_nodes,
        "pending_nodes": pending_nodes,
        "total_up": total_up,
        "total_down": total_down,
        "total_bytes": total_up + total_down,
        "total_requests": total_requests,
        "hours": hours,
        "top_clients": top5,
    })))
}
