use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Datelike, Days, Local};
use serde_json::{json, Value};

/// 每日流量看板展示的天数（与 traffic 清理保留的 15 天窗口对齐）。
const DAY_WINDOW: i64 = 14;
const WEEKDAYS: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

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

    // 小时 / 天都按服务器本地时区切分，前端标签才不会出现时区错位。
    let now_ts = chrono::Utc::now().timestamp();
    let tz_offset = Local::now().offset().local_minus_utc() as i64;
    let now_hour = ((now_ts + tz_offset) / 3600) * 3600 - tz_offset;
    let since = now_hour - 7 * 3600;

    let hourly: Vec<(i64, i64, i64)> = sqlx::query_as(
        r#"SELECT ((ts + ?) / 3600) * 3600 - ? as bucket,
                  COALESCE(SUM(bytes_up),0),
                  COALESCE(SUM(bytes_down),0)
           FROM traffic_logs WHERE ts >= ?
           GROUP BY bucket ORDER BY bucket"#,
    )
    .bind(tz_offset)
    .bind(tz_offset)
    .bind(since)
    .fetch_all(&state.db)
    .await?;

    let mut hours = Vec::new();
    for i in 0..8 {
        let ts = now_hour - (7 - i) * 3600;
        let found = hourly.iter().find(|r| r.0 == ts);
        hours.push(json!({
            "ts": ts,
            "label": DateTime::from_timestamp(ts, 0)
                .map(|d| d.with_timezone(&Local).format("%H:00").to_string())
                .unwrap_or_default(),
            "up": found.map(|f| f.1).unwrap_or(0),
            "down": found.map(|f| f.2).unwrap_or(0),
            "total": found.map(|f| f.1 + f.2).unwrap_or(0),
        }));
    }

    // 最近 DAY_WINDOW 天（本地日）的每日用量。
    let today = Local::now().date_naive();
    let first_day = today - Days::new((DAY_WINDOW - 1) as u64);
    let daily: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        r#"SELECT date(ts, 'unixepoch', 'localtime') AS d,
                  COALESCE(SUM(bytes_up),0),
                  COALESCE(SUM(bytes_down),0),
                  COUNT(*)
           FROM traffic_logs WHERE ts >= ?
           GROUP BY d"#,
    )
    .bind(now_ts - (DAY_WINDOW + 1) * 86400)
    .fetch_all(&state.db)
    .await?;

    let mut days = Vec::new();
    let (mut sum_up, mut sum_down, mut sum_requests) = (0i64, 0i64, 0i64);
    let (mut active_days, mut peak_total) = (0i64, 0i64);
    let (mut peak_label, mut today_total, mut today_requests) =
        (String::new(), 0i64, 0i64);
    for i in 0..DAY_WINDOW {
        let date = first_day + Days::new(i as u64);
        let key = date.format("%Y-%m-%d").to_string();
        let found = daily.iter().find(|r| r.0 == key);
        let up = found.map(|f| f.1).unwrap_or(0);
        let down = found.map(|f| f.2).unwrap_or(0);
        let requests = found.map(|f| f.3).unwrap_or(0);
        let total = up + down;
        if total > 0 {
            active_days += 1;
        }
        sum_up += up;
        sum_down += down;
        sum_requests += requests;
        let label = date.format("%m-%d").to_string();
        if total > peak_total {
            peak_total = total;
            peak_label = label.clone();
        }
        if date == today {
            today_total = total;
            today_requests = requests;
        }
        days.push(json!({
            "date": key,
            "label": label,
            "weekday": WEEKDAYS[date.weekday().num_days_from_monday() as usize],
            "up": up,
            "down": down,
            "total": total,
            "requests": requests,
        }));
    }
    let days_summary = json!({
        "up": sum_up,
        "down": sum_down,
        "total": sum_up + sum_down,
        "requests": sum_requests,
        "avg": (sum_up + sum_down) / DAY_WINDOW,
        "active_days": active_days,
        "peak_label": peak_label,
        "peak_total": peak_total,
        "today_label": today.format("%m-%d").to_string(),
        "today_total": today_total,
        "today_requests": today_requests,
    });

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
        "days": days,
        "days_summary": days_summary,
        "top_clients": top5,
    })))
}
