use crate::auth::AuthUser;
use crate::error::{ApiResult, AppError};
use crate::models::ProxyNode;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pools/{id}/nodes", get(list))
        .route("/nodes/{id}", get(get_one).delete(delete))
        .route("/nodes/{id}/status", post(set_status))
        .route("/nodes/{id}/check", post(check_one))
}

#[derive(Deserialize)]
pub struct NodeQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub country: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<NodeQuery>,
) -> ApiResult<Json<Value>> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * size;

    let mut sql = String::from("SELECT * FROM proxy_nodes WHERE pool_id=?");
    let mut count_sql = String::from("SELECT COUNT(*) FROM proxy_nodes WHERE pool_id=?");
    if let Some(st) = &q.status {
        if !st.is_empty() && st != "all" {
            sql.push_str(" AND status=?");
            count_sql.push_str(" AND status=?");
        }
    }
    if let Some(c) = &q.country {
        if !c.is_empty() {
            sql.push_str(" AND country_code=?");
            count_sql.push_str(" AND country_code=?");
        }
    }
    if let Some(kw) = &q.q {
        if !kw.is_empty() {
            sql.push_str(" AND (host LIKE ? OR exit_ip LIKE ? OR asn_org LIKE ?)");
            count_sql.push_str(" AND (host LIKE ? OR exit_ip LIKE ? OR asn_org LIKE ?)");
        }
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let mut cq = sqlx::query_as::<_, (i64,)>(&count_sql).bind(&id);
    let mut lq = sqlx::query_as::<_, ProxyNode>(&sql).bind(&id);
    if let Some(st) = &q.status {
        if !st.is_empty() && st != "all" {
            cq = cq.bind(st);
            lq = lq.bind(st);
        }
    }
    if let Some(c) = &q.country {
        if !c.is_empty() {
            cq = cq.bind(c);
            lq = lq.bind(c);
        }
    }
    if let Some(kw) = &q.q {
        if !kw.is_empty() {
            let like = format!("%{kw}%");
            cq = cq.bind(like.clone()).bind(like.clone()).bind(like.clone());
            lq = lq.bind(like.clone()).bind(like.clone()).bind(like);
        }
    }
    lq = lq.bind(size).bind(offset);
    let total: (i64,) = cq.fetch_one(&state.db).await?;
    let items: Vec<ProxyNode> = lq.fetch_all(&state.db).await?;
    Ok(Json(json!({
        "total": total.0,
        "page": page,
        "page_size": size,
        "items": items,
    })))
}

async fn get_one(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ProxyNode>> {
    let row = sqlx::query_as("SELECT * FROM proxy_nodes WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("节点不存在"))?;
    Ok(Json(row))
}

async fn delete(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    sqlx::query("DELETE FROM proxy_nodes WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct StatusReq {
    status: String,
}

async fn set_status(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<StatusReq>,
) -> ApiResult<Json<Value>> {
    if !["active", "isolated", "disabled", "pending"].contains(&req.status.as_str()) {
        return Err(AppError::bad("无效状态"));
    }
    let isolated = if req.status == "isolated" {
        Some(chrono::Utc::now().to_rfc3339())
    } else {
        None
    };
    sqlx::query("UPDATE proxy_nodes SET status=?, isolated_until=?, fail_count=CASE WHEN ?='active' THEN 0 ELSE fail_count END WHERE id=?")
        .bind(&req.status)
        .bind(isolated)
        .bind(&req.status)
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn check_one(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let node: ProxyNode = sqlx::query_as("SELECT * FROM proxy_nodes WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("节点不存在"))?;
    let pool: crate::models::NodePool = sqlx::query_as("SELECT * FROM node_pools WHERE id=?")
        .bind(&node.pool_id)
        .fetch_one(&state.db)
        .await?;
    let policy = if let Some(pid) = pool.detection_policy_id {
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
    crate::worker::health::check_node(&state, &node, &policy)
        .await
        .map_err(|e| AppError::bad(e.to_string()))?;
    let updated: ProxyNode = sqlx::query_as("SELECT * FROM proxy_nodes WHERE id=?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!(updated)))
}
