use crate::auth::AuthUser;
use crate::db::{get_setting, now_rfc3339};
use crate::error::{ApiResult, AppError};
use crate::models::{ServiceNode, ServicePayload};
use crate::proxy::selector;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/services", get(list).post(create))
        .route("/services/{id}", get(get_one).put(update).delete(delete))
        .route("/services/{id}/toggle", post(toggle))
        .route("/services/{id}/test", post(test_service))
        .route("/services/{id}/curl", get(curl_cmds))
}

async fn list(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Value>> {
    let rows: Vec<ServiceNode> =
        sqlx::query_as("SELECT * FROM service_nodes ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;
    let mut out = Vec::new();
    for s in rows {
        out.push(service_view(&state, s).await);
    }
    Ok(Json(json!(out)))
}

async fn get_one(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let s: ServiceNode = sqlx::query_as("SELECT * FROM service_nodes WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("服务节点不存在"))?;
    Ok(Json(service_view(&state, s).await))
}

async fn service_view(state: &AppState, s: ServiceNode) -> Value {
    let available = selector::count_available(state, &s).await;
    let listening = state.listeners.contains_key(&s.id);
    let bind_error = state.bind_errors.get(&s.id).map(|e| e.clone());
    json!({
        "id": s.id,
        "name": s.name,
        "listen_host": s.listen_host,
        "listen_port": s.listen_port,
        "enable_http": s.enable_http == 1,
        "enable_socks5": s.enable_socks5 == 1,
        "enable_socks5h": s.enable_socks5h == 1,
        "username": s.username,
        "password": s.password,
        "pool_ids": serde_json::from_str::<Value>(&s.pool_ids).unwrap_or(json!([])),
        "filter_countries": serde_json::from_str::<Value>(&s.filter_countries).unwrap_or(json!([])),
        "filter_asns": serde_json::from_str::<Value>(&s.filter_asns).unwrap_or(json!([])),
        "filter_residential": s.filter_residential == 1,
        "selection_strategy": s.selection_strategy,
        "sticky_ttl": s.sticky_ttl,
        "enabled": s.enabled == 1,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "available_nodes": available,
        "listening": listening,
        "bind_error": bind_error,
        "health": if s.enabled != 1 {
            "disabled"
        } else if bind_error.is_some() {
            "error"
        } else if listening {
            "healthy"
        } else {
            "stopped"
        }
    })
}

async fn create(
    State(state): State<AppState>,
    _u: AuthUser,
    Json(req): Json<ServicePayload>,
) -> ApiResult<Json<Value>> {
    validate(&req)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    insert_or_fail(&state, &id, &req, &now, true).await?;
    if req.enabled.unwrap_or(true) {
        if let Some(svc) = crate::state::load_service(&state.db, &id).await? {
            if let Err(e) = crate::proxy::start_service(&state, &svc).await {
                state.bind_errors.insert(id.clone(), format!("{e:#}"));
            }
        }
    }
    get_one(State(state), _u, Path(id)).await
}

async fn update(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ServicePayload>,
) -> ApiResult<Json<Value>> {
    validate(&req)?;
    let now = now_rfc3339();
    let r = sqlx::query(
        r#"UPDATE service_nodes SET name=?, listen_host=?, listen_port=?, enable_http=?, enable_socks5=?,
           enable_socks5h=?, username=?, password=?, pool_ids=?, filter_countries=?, filter_asns=?,
           filter_residential=?, selection_strategy=?, sticky_ttl=?, enabled=?, updated_at=? WHERE id=?"#,
    )
    .bind(req.name.trim())
    .bind(req.listen_host.as_deref().unwrap_or("0.0.0.0"))
    .bind(req.listen_port)
    .bind(if req.enable_http.unwrap_or(true) { 1 } else { 0 })
    .bind(if req.enable_socks5.unwrap_or(true) { 1 } else { 0 })
    .bind(if req.enable_socks5h.unwrap_or(true) { 1 } else { 0 })
    .bind(&req.username)
    .bind(&req.password)
    .bind(serde_json::to_string(&req.pool_ids.clone().unwrap_or_default()).unwrap())
    .bind(serde_json::to_string(&req.filter_countries.clone().unwrap_or_default()).unwrap())
    .bind(serde_json::to_string(&req.filter_asns.clone().unwrap_or_default()).unwrap())
    .bind(if req.filter_residential.unwrap_or(false) { 1 } else { 0 })
    .bind(req.selection_strategy.as_deref().unwrap_or("round_robin"))
    .bind(req.sticky_ttl.unwrap_or(300))
    .bind(if req.enabled.unwrap_or(true) { 1 } else { 0 })
    .bind(&now)
    .bind(&id)
    .execute(&state.db)
    .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::not_found("服务节点不存在"));
    }
    if let Err(e) = crate::proxy::restart_service(&state, &id).await {
        return Err(AppError::bad(format!("监听启动失败: {e:#}")));
    }
    get_one(State(state), _u, Path(id)).await
}

async fn insert_or_fail(
    state: &AppState,
    id: &str,
    req: &ServicePayload,
    now: &str,
    _create: bool,
) -> ApiResult<()> {
    sqlx::query(
        r#"INSERT INTO service_nodes
        (id,name,listen_host,listen_port,enable_http,enable_socks5,enable_socks5h,username,password,pool_ids,filter_countries,filter_asns,filter_residential,selection_strategy,sticky_ttl,enabled,created_at,updated_at)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
    )
    .bind(id)
    .bind(req.name.trim())
    .bind(req.listen_host.as_deref().unwrap_or("0.0.0.0"))
    .bind(req.listen_port)
    .bind(if req.enable_http.unwrap_or(true) { 1 } else { 0 })
    .bind(if req.enable_socks5.unwrap_or(true) { 1 } else { 0 })
    .bind(if req.enable_socks5h.unwrap_or(true) { 1 } else { 0 })
    .bind(&req.username)
    .bind(&req.password)
    .bind(serde_json::to_string(&req.pool_ids.clone().unwrap_or_default()).unwrap())
    .bind(serde_json::to_string(&req.filter_countries.clone().unwrap_or_default()).unwrap())
    .bind(serde_json::to_string(&req.filter_asns.clone().unwrap_or_default()).unwrap())
    .bind(if req.filter_residential.unwrap_or(false) { 1 } else { 0 })
    .bind(req.selection_strategy.as_deref().unwrap_or("round_robin"))
    .bind(req.sticky_ttl.unwrap_or(300))
    .bind(if req.enabled.unwrap_or(true) { 1 } else { 0 })
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            AppError::bad("监听端口已被占用")
        } else {
            AppError::from(e)
        }
    })?;
    Ok(())
}

async fn delete(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    crate::proxy::stop_service(&state, &id).await;
    sqlx::query("DELETE FROM service_nodes WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn toggle(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    sqlx::query("UPDATE service_nodes SET enabled = 1 - enabled, updated_at=? WHERE id=?")
        .bind(now_rfc3339())
        .bind(&id)
        .execute(&state.db)
        .await?;
    if let Err(e) = crate::proxy::restart_service(&state, &id).await {
        return Err(AppError::bad(format!("监听启动失败: {e:#}")));
    }
    get_one(State(state), _u, Path(id)).await
}

async fn curl_cmds(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let s: ServiceNode = sqlx::query_as("SELECT * FROM service_nodes WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("服务节点不存在"))?;
    Ok(Json(build_curl(&state, &s).await?))
}

async fn build_curl(state: &AppState, s: &ServiceNode) -> ApiResult<Value> {
    let host = get_setting(&state.db, "server_host")
        .await?
        .unwrap_or_else(|| "127.0.0.1".into());
    let user = urlencoding_min(&s.username);
    let pass = urlencoding_min(&s.password);
    let port = s.listen_port;
    let test_url = "https://api.ipify.org";
    let http_proxy = format!("http://{user}:{pass}@{host}:{port}");
    let socks = format!("socks5://{user}:{pass}@{host}:{port}");
    let socksh = format!("socks5h://{user}:{pass}@{host}:{port}");
    Ok(json!({
        "http_proxy": http_proxy,
        "socks5_proxy": socks,
        "socks5h_proxy": socksh,
        "curl_http": format!("curl -x {http_proxy} {test_url}"),
        "curl_socks5": format!("curl --socks5 {user}:{pass}@{host}:{port} {test_url}"),
        "curl_socks5h": format!("curl --socks5-hostname {user}:{pass}@{host}:{port} {test_url}"),
    }))
}

fn urlencoding_min(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

async fn test_service(
    State(state): State<AppState>,
    _u: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let s: ServiceNode = sqlx::query_as("SELECT * FROM service_nodes WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("服务节点不存在"))?;
    if s.enabled == 0 {
        return Err(AppError::bad("服务未启用"));
    }
    if !state.listeners.contains_key(&s.id) {
        return Err(AppError::bad("服务未在监听，请检查端口占用或先启用"));
    }
    let user = urlencoding_min(&s.username);
    let pass = urlencoding_min(&s.password);
    let proxy_url = if s.enable_http == 1 {
        format!("http://{user}:{pass}@127.0.0.1:{}", s.listen_port)
    } else {
        format!("socks5h://{user}:{pass}@127.0.0.1:{}", s.listen_port)
    };
    let start = std::time::Instant::now();
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).map_err(|e| AppError::bad(e.to_string()))?)
        .timeout(std::time::Duration::from_secs(15))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| AppError::internal(e.to_string()))?;
    match client.get("https://api.ipify.org").send().await {
        Ok(resp) => {
            let ip = resp.text().await.unwrap_or_default();
            let geo = state.geoip.lookup_str(ip.trim());
            Ok(Json(json!({
                "ok": true,
                "exit_ip": ip.trim(),
                "latency_ms": start.elapsed().as_millis() as u64,
                "geo": geo,
                "message": "探测成功"
            })))
        }
        Err(e) => Err(AppError::bad(format!("探测失败: {e}"))),
    }
}

fn validate(req: &ServicePayload) -> ApiResult<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::bad("服务名称不能为空"));
    }
    if req.username.trim().is_empty() || req.password.trim().is_empty() {
        return Err(AppError::bad("必须设置访问账号和密码"));
    }
    if !(1..=65535).contains(&req.listen_port) {
        return Err(AppError::bad("端口无效"));
    }
    if !req.enable_http.unwrap_or(true)
        && !req.enable_socks5.unwrap_or(true)
        && !req.enable_socks5h.unwrap_or(true)
    {
        return Err(AppError::bad("至少启用一种代理协议"));
    }
    let st = req.selection_strategy.as_deref().unwrap_or("round_robin");
    if !["round_robin", "lowest_latency", "least_used", "idle_first", "sticky"].contains(&st) {
        return Err(AppError::bad("未知的选择策略"));
    }
    Ok(())
}
