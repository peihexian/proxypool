use crate::state::{AppState, TrafficEvent, USAGE_LOG_LIMIT};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

pub async fn run(state: AppState, mut rx: mpsc::Receiver<TrafficEvent>) {
    seed_recent(&state).await;
    let mut buf: Vec<TrafficEvent> = Vec::new();
    let mut tick = interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            Some(ev) = rx.recv() => {
                buf.push(ev);
                if buf.len() >= 200 {
                    flush(&state, &mut buf).await;
                }
            }
            _ = tick.tick() => {
                flush(&state, &mut buf).await;
                cleanup(&state).await;
            }
        }
    }
}

pub async fn load_recent(db: &sqlx::SqlitePool) -> Vec<TrafficEvent> {
    let rows: Vec<(String, String, String, String, String, i64, i64, i64)> = sqlx::query_as(
        r#"SELECT service_id, client_ip,
                  COALESCE(proxy_ip,''), COALESCE(dest,''), COALESCE(protocol,''),
                  bytes_up, bytes_down, ts
           FROM traffic_logs
           ORDER BY ts DESC, id DESC
           LIMIT ?"#,
    )
    .bind(USAGE_LOG_LIMIT as i64)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(service_id, client_ip, proxy_ip, dest, protocol, up, down, ts)| TrafficEvent {
            service_id,
            client_ip,
            proxy_ip,
            dest,
            protocol,
            bytes_up: up as u64,
            bytes_down: down as u64,
            ts,
        })
        .collect()
}

async fn seed_recent(state: &AppState) {
    let rows = load_recent(&state.db).await;
    state.seed_usage_logs(rows);
}

async fn flush(state: &AppState, buf: &mut Vec<TrafficEvent>) {
    if buf.is_empty() {
        return;
    }
    let batch = std::mem::take(buf);
    for ev in batch {
        let _ = sqlx::query(
            r#"INSERT INTO traffic_logs(service_id, client_ip, proxy_ip, dest, protocol, bytes_up, bytes_down, ts)
               VALUES(?,?,?,?,?,?,?,?)"#,
        )
        .bind(&ev.service_id)
        .bind(&ev.client_ip)
        .bind(&ev.proxy_ip)
        .bind(&ev.dest)
        .bind(&ev.protocol)
        .bind(ev.bytes_up as i64)
        .bind(ev.bytes_down as i64)
        .bind(ev.ts)
        .execute(&state.db)
        .await;
    }
}

async fn cleanup(state: &AppState) {
    let cutoff = chrono::Utc::now().timestamp() - 15 * 24 * 3600;
    let _ = sqlx::query("DELETE FROM traffic_logs WHERE ts < ?")
        .bind(cutoff)
        .execute(&state.db)
        .await;
}
