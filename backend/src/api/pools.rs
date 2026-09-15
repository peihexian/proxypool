use crate::auth::AuthUser;
use crate::db::now_rfc3339;
use crate::error::{ApiResult, AppError};
use crate::models::{NodePool, PoolPayload};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pools", get(list).post(create))
        .route("/pools/{id}", get(get_one).put(update).delete(delete))
        .route("/pools/{id}/toggle", post(toggle))
        .route("/pools/{id}/sync", post(sync))
        .route("/pools/{id}/import", post(import))
        .route("/pools/{id}/check", post(check_all))
}

async fn list(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Value>> {
    let pools: Vec<NodePool> =
        sqlx::query_as("SELECT * FROM node_pools ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;
    let mut out = Vec::new();
    for p in pools {
        let (node_count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM proxy_nodes WHERE pool_id=?")
                .bind(&p.id)
                .fetch_one(&state.db)
                .await?;
        let (active_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM proxy_nodes WHERE pool_id=? AND status='active'",
        )
        .bind(&p.id)
        .fetch_one(&state.db)
        .await?;
        let (isolated_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM proxy_nodes WHERE pool_id=? AND status='isolated'",
        )
        .bind(&p.id)
        .fetch_one(&state.db)
        .await?;
        out.push(json!({
            "id": p.id,
            "name": p.name,
            "source_type": p.source_type,
            "subscription_url": p.subscription_url,
            "data_format": p.data_format,
            "encoding_format": p.encoding_format,
            "format_config": p.format_config.as_ref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
            "charset": p.charset,
            "default_protocol": p.default_protocol,
            "update_interval": p.update_interval,
            "detection_policy_id": p.detection_policy_id,
            "enabled": p.enabled == 1,
            "last_sync": p.last_sync,
            "created_at": p.created_at,
            "updated_at": p.updated_at,
            "node_count": node_count,
            "active_count": active_count,
            "isolated_count": isolated_count,
        }));
    }
    Ok(Json(json!(out)))
}

async fn get_one(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let p: NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("节点池不存在"))?;
    Ok(Json(pool_json(&p)))
}

fn pool_json(p: &NodePool) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "source_type": p.source_type,
        "subscription_url": p.subscription_url,
        "data_format": p.data_format,
        "encoding_format": p.encoding_format,
        "format_config": p.format_config.as_ref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
        "charset": p.charset,
        "default_protocol": p.default_protocol,
        "update_interval": p.update_interval,
        "detection_policy_id": p.detection_policy_id,
        "enabled": p.enabled == 1,
        "last_sync": p.last_sync,
        "created_at": p.created_at,
        "updated_at": p.updated_at,
    })
}

async fn create(
    State(state): State<AppState>,
    _u: AuthUser,
    Json(req): Json<PoolPayload>,
) -> ApiResult<Json<Value>> {
    validate(&req)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let cfg = req
        .format_config
        .as_ref()
        .map(|v| v.to_string());
    sqlx::query(
        r#"INSERT INTO node_pools
        (id,name,source_type,subscription_url,data_format,encoding_format,format_config,charset,default_protocol,update_interval,detection_policy_id,enabled,created_at,updated_at)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(&req.source_type)
    .bind(&req.subscription_url)
    .bind(req.data_format.as_deref().unwrap_or("txt"))
    .bind(req.encoding_format.as_deref().unwrap_or("auto"))
    .bind(&cfg)
    .bind(req.charset.as_deref().unwrap_or("utf-8"))
    .bind(req.default_protocol.as_deref().unwrap_or("http"))
    .bind(req.update_interval.unwrap_or(3600))
    .bind(&req.detection_policy_id)
    .bind(if req.enabled.unwrap_or(true) { 1 } else { 0 })
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    if let Some(data) = req.local_data.as_ref().filter(|s| !s.trim().is_empty()) {
        let pool: NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
        crate::worker::subscribe::import_local(&state, &pool, data, true)
            .await
            .map_err(|e| AppError::bad(e.to_string()))?;
    }
    get_one(State(state), _u, Path(id)).await
}

async fn update(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<PoolPayload>,
) -> ApiResult<Json<Value>> {
    validate(&req)?;
    let now = now_rfc3339();
    let cfg = req.format_config.as_ref().map(|v| v.to_string());
    let r = sqlx::query(
        r#"UPDATE node_pools SET name=?, source_type=?, subscription_url=?, data_format=?, encoding_format=?,
           format_config=?, charset=?, default_protocol=?, update_interval=?, detection_policy_id=?,
           enabled=?, updated_at=? WHERE id=?"#,
    )
    .bind(req.name.trim())
    .bind(&req.source_type)
    .bind(&req.subscription_url)
    .bind(req.data_format.as_deref().unwrap_or("txt"))
    .bind(req.encoding_format.as_deref().unwrap_or("auto"))
    .bind(&cfg)
    .bind(req.charset.as_deref().unwrap_or("utf-8"))
    .bind(req.default_protocol.as_deref().unwrap_or("http"))
    .bind(req.update_interval.unwrap_or(3600))
    .bind(&req.detection_policy_id)
    .bind(if req.enabled.unwrap_or(true) { 1 } else { 0 })
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::not_found("节点池不存在"));
    }
    if let Some(data) = req.local_data.as_ref().filter(|s| !s.trim().is_empty()) {
        let pool: NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
        let replace = req.import_mode.as_deref() != Some("append");
        crate::worker::subscribe::import_local(&state, &pool, data, replace)
            .await
            .map_err(|e| AppError::bad(e.to_string()))?;
    }
    get_one(State(state), _u, Path(id)).await
}

async fn delete(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    sqlx::query("DELETE FROM proxy_nodes WHERE pool_id=?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    let r = sqlx::query("DELETE FROM node_pools WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::not_found("节点池不存在"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn toggle(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    sqlx::query("UPDATE node_pools SET enabled = 1 - enabled, updated_at=? WHERE id=?")
        .bind(now_rfc3339())
        .bind(&id)
        .execute(&state.db)
        .await?;
    get_one(State(state), _u, Path(id)).await
}

async fn sync(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let pool: NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("节点池不存在"))?;
    let n = crate::worker::subscribe::sync_pool(&state, &pool)
        .await
        .map_err(|e| AppError::bad(e.to_string()))?;
    Ok(Json(json!({ "ok": true, "imported": n })))
}

#[derive(serde::Deserialize)]
struct ImportReq {
    text: String,
    replace: Option<bool>,
}

async fn import(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ImportReq>,
) -> ApiResult<Json<Value>> {
    let pool: NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("节点池不存在"))?;
    let n = crate::worker::subscribe::import_local(
        &state,
        &pool,
        &req.text,
        req.replace.unwrap_or(true),
    )
    .await
    .map_err(|e| AppError::bad(e.to_string()))?;
    Ok(Json(json!({ "ok": true, "imported": n })))
}

async fn check_all(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let pool: NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("节点池不存在"))?;
    let policy = if let Some(pid) = &pool.detection_policy_id {
        sqlx::query_as("SELECT * FROM detection_policies WHERE id=?")
            .bind(pid)
            .fetch_optional(&state.db)
            .await?
    } else {
        sqlx::query_as("SELECT * FROM detection_policies LIMIT 1")
            .fetch_optional(&state.db)
            .await?
    };
    let Some(policy) = policy else {
        return Err(AppError::bad("请先配置检测策略"));
    };
    let nodes: Vec<crate::models::ProxyNode> =
        sqlx::query_as("SELECT * FROM proxy_nodes WHERE pool_id=? AND status != 'disabled'")
            .bind(&id)
            .fetch_all(&state.db)
            .await?;
    let st = state.clone();
    tokio::spawn(async move {
        for n in nodes.into_iter().take(200) {
            let _ = crate::worker::health::check_node(&st, &n, &policy).await;
        }
    });
    Ok(Json(json!({ "ok": true, "message": "已开始后台检测" })))
}

fn validate(req: &PoolPayload) -> ApiResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::bad("节点池名称不能为空"));
    }
    if req.source_type != "remote" && req.source_type != "local" {
        return Err(AppError::bad("来源类型必须是 remote 或 local"));
    }
    if req.source_type == "remote"
        && req
            .subscription_url
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
    {
        return Err(AppError::bad("远程订阅需要填写订阅地址"));
    }
    Ok(())
}
