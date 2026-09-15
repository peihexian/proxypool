use crate::state::{AppState, TrafficEvent};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

pub async fn run(state: AppState, mut rx: mpsc::Receiver<TrafficEvent>) {
    let mut buf: Vec<TrafficEvent> = Vec::new();
    let mut tick = interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            Some(ev) = rx.recv() => {
                if ev.bytes_up > 0 || ev.bytes_down > 0 {
                    buf.push(ev);
                }
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

async fn flush(state: &AppState, buf: &mut Vec<TrafficEvent>) {
    if buf.is_empty() {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let batch = std::mem::take(buf);
    for ev in batch {
        let _ = sqlx::query(
            "INSERT INTO traffic_logs(service_id, client_ip, bytes_up, bytes_down, ts) VALUES(?,?,?,?,?)",
        )
        .bind(&ev.service_id)
        .bind(&ev.client_ip)
        .bind(ev.bytes_up as i64)
        .bind(ev.bytes_down as i64)
        .bind(now)
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
