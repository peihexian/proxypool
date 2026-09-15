use crate::auth::AuthUser;
use crate::db::now_rfc3339;
use crate::error::{ApiResult, AppError};
use crate::models::{DetectionPolicy, PolicyPayload};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/policies", get(list).post(create))
        .route("/policies/{id}", get(get_one).put(update).delete(delete))
}

async fn list(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Vec<DetectionPolicy>>> {
    let rows = sqlx::query_as("SELECT * FROM detection_policies ORDER BY created_at")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<DetectionPolicy>> {
    let row = sqlx::query_as("SELECT * FROM detection_policies WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("策略不存在"))?;
    Ok(Json(row))
}

async fn create(
    State(state): State<AppState>,
    _u: AuthUser,
    Json(req): Json<PolicyPayload>,
) -> ApiResult<Json<DetectionPolicy>> {
    validate(&req)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    sqlx::query(
        r#"INSERT INTO detection_policies
        (id,name,check_interval,check_url,timeout_ms,on_fail,isolate_seconds,backoff_base_seconds,max_fails,created_at,updated_at)
        VALUES (?,?,?,?,?,?,?,?,?,?,?)"#,
    )
    .bind(&id)
    .bind(&req.name)
    .bind(req.check_interval.unwrap_or(300))
    .bind(req.check_url.as_deref().unwrap_or("https://api.ipify.org"))
    .bind(req.timeout_ms.unwrap_or(10000))
    .bind(req.on_fail.as_deref().unwrap_or("temp_isolate"))
    .bind(req.isolate_seconds.unwrap_or(600))
    .bind(req.backoff_base_seconds.unwrap_or(30))
    .bind(req.max_fails.unwrap_or(3))
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;
    get_one(State(state), _u, Path(id)).await
}

async fn update(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<PolicyPayload>,
) -> ApiResult<Json<DetectionPolicy>> {
    validate(&req)?;
    let now = now_rfc3339();
    let r = sqlx::query(
        r#"UPDATE detection_policies SET name=?, check_interval=?, check_url=?, timeout_ms=?,
           on_fail=?, isolate_seconds=?, backoff_base_seconds=?, max_fails=?, updated_at=? WHERE id=?"#,
    )
    .bind(&req.name)
    .bind(req.check_interval.unwrap_or(300))
    .bind(req.check_url.as_deref().unwrap_or("https://api.ipify.org"))
    .bind(req.timeout_ms.unwrap_or(10000))
    .bind(req.on_fail.as_deref().unwrap_or("temp_isolate"))
    .bind(req.isolate_seconds.unwrap_or(600))
    .bind(req.backoff_base_seconds.unwrap_or(30))
    .bind(req.max_fails.unwrap_or(3))
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::not_found("策略不存在"));
    }
    get_one(State(state), _u, Path(id)).await
}

async fn delete(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let used: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM node_pools WHERE detection_policy_id=?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    if used.0 > 0 {
        return Err(AppError::bad("仍有节点池引用该策略，无法删除"));
    }
    let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM detection_policies")
        .fetch_one(&state.db)
        .await?;
    if n.0 <= 1 {
        return Err(AppError::bad("至少保留一条检测策略"));
    }
    sqlx::query("DELETE FROM detection_policies WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

fn validate(req: &PolicyPayload) -> ApiResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::bad("策略名称不能为空"));
    }
    if let Some(ref s) = req.on_fail {
        if !["backoff", "isolate", "temp_isolate"].contains(&s.as_str()) {
            return Err(AppError::bad("失败策略必须是 backoff / isolate / temp_isolate"));
        }
    }
    Ok(())
}
