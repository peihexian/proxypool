use crate::auth::AuthUser;
use crate::db::{get_setting, set_setting};
use crate::error::{ApiResult, AppError};
use crate::models::SettingsUpdate;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(get_settings).put(update_settings))
        .route("/settings/geoip/update", post(update_geoip))
}

async fn get_settings(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Value>> {
    let server_host = get_setting(&state.db, "server_host")
        .await?
        .unwrap_or_else(|| "127.0.0.1".into());
    Ok(Json(json!({
        "server_host": server_host,
        "geoip_country_updated_at": state.geoip.country_mtime(),
        "geoip_asn_updated_at": state.geoip.asn_mtime(),
        "default_password_hint": "首次登录默认密码为 admin，请尽快修改"
    })))
}

async fn update_settings(
    State(state): State<AppState>,
    _u: AuthUser,
    Json(req): Json<SettingsUpdate>,
) -> ApiResult<Json<Value>> {
    if let Some(host) = req.server_host {
        let host = host.trim();
        if host.is_empty() {
            return Err(AppError::bad("服务器地址不能为空"));
        }
        set_setting(&state.db, "server_host", host).await?;
    }
    if let Some(new_pwd) = req.new_password {
        if new_pwd.len() < 4 {
            return Err(AppError::bad("新密码至少 4 位"));
        }
        let old = req
            .old_password
            .ok_or_else(|| AppError::bad("请输入原密码"))?;
        let hash = get_setting(&state.db, "admin_password")
            .await?
            .ok_or_else(|| AppError::internal("未初始化密码"))?;
        if !bcrypt::verify(&old, &hash).unwrap_or(false) {
            return Err(AppError::bad("原密码错误"));
        }
        let nh = bcrypt::hash(&new_pwd, 10).map_err(|e| AppError::internal(e.to_string()))?;
        set_setting(&state.db, "admin_password", &nh).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn update_geoip(State(state): State<AppState>, _u: AuthUser) -> ApiResult<Json<Value>> {
    state
        .geoip
        .update()
        .await
        .map_err(|e| AppError::internal(format!("GeoIP 更新失败: {e}")))?;
    set_setting(
        &state.db,
        "geoip_updated_at",
        &crate::db::now_rfc3339(),
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "geoip_country_updated_at": state.geoip.country_mtime(),
        "geoip_asn_updated_at": state.geoip.asn_mtime()
    })))
}
