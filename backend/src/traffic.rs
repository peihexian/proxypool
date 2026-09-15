use parking_lot::Mutex;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct TrafficRecord {
    pub service_id: String,
    pub client_ip: String,
    pub proxy_node_id: Option<String>,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub recorded_at: String,
}

pub struct TrafficCollector {
    buf: Mutex<Vec<TrafficRecord>>,
}

impl TrafficCollector {
    pub fn new() -> Self {
        Self {
            buf: Mutex::new(Vec::new()),
        }
    }

    pub fn add(&self, rec: TrafficRecord) {
        if rec.bytes_up == 0 && rec.bytes_down == 0 {
            return;
        }
        self.buf.lock().push(rec);
    }

    pub async fn flush(&self, db: &SqlitePool) {
        let recs: Vec<TrafficRecord> = {
            let mut g = self.buf.lock();
            if g.is_empty() {
                return;
            }
            std::mem::take(&mut *g)
        };
        if recs.is_empty() {
            return;
        }
        let mut tx = match db.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!("traffic tx: {e}");
                self.buf.lock().extend(recs);
                return;
            }
        };
        for r in &recs {
            if let Err(e) = sqlx::query(
                r#"INSERT INTO traffic_logs (service_id, client_ip, proxy_node_id, bytes_up, bytes_down, recorded_at)
                   VALUES (?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&r.service_id)
            .bind(&r.client_ip)
            .bind(&r.proxy_node_id)
            .bind(r.bytes_up as i64)
            .bind(r.bytes_down as i64)
            .bind(&r.recorded_at)
            .execute(&mut *tx)
            .await
            {
                tracing::error!("traffic insert: {e}");
            }
        }
        if let Err(e) = tx.commit().await {
            tracing::error!("traffic commit: {e}");
        }
    }
}

pub fn spawn_flusher(state: crate::state::AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            state.traffic.flush(&state.db).await;
        }
    });
}

pub fn spawn_cleanup(db: SqlitePool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(e) = sqlx::query(
                "DELETE FROM traffic_logs WHERE recorded_at < datetime('now', '-30 days')",
            )
            .execute(&db)
            .await
            {
                tracing::warn!("cleanup traffic: {e}");
            }
        }
    });
}

#[allow(dead_code)]
fn _arc_hint() {
    let _ = Arc::new(0);
}
