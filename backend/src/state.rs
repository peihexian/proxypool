use crate::geoip::GeoDb;
use crate::models::ServiceNode;
use dashmap::DashMap;
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

pub const USAGE_LOG_LIMIT: usize = 100;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub jwt_secret: String,
    pub geoip: Arc<GeoDb>,
    pub listeners: Arc<DashMap<String, ListenerHandle>>,
    pub rr_index: Arc<DashMap<String, AtomicU64>>,
    pub sticky: Arc<DashMap<String, StickyEntry>>,
    pub traffic_tx: mpsc::Sender<TrafficEvent>,
    pub bind_errors: Arc<DashMap<String, String>>,
    pub usage_logs: Arc<Mutex<VecDeque<TrafficEvent>>>,
}

pub struct ListenerHandle {
    pub abort: broadcast::Sender<()>,
    pub task: JoinHandle<()>,
}

#[derive(Clone, Debug)]
pub struct StickyEntry {
    pub node_id: String,
    pub host: String,
    pub exit_ip: Option<String>,
    pub expire_ts: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrafficEvent {
    pub service_id: String,
    pub client_ip: String,
    pub proxy_ip: String,
    pub dest: String,
    pub protocol: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub ts: i64,
}

impl AppState {
    pub fn new(
        db: SqlitePool,
        jwt_secret: String,
        geoip: Arc<GeoDb>,
        traffic_tx: mpsc::Sender<TrafficEvent>,
    ) -> Self {
        Self {
            db,
            jwt_secret,
            geoip,
            listeners: Arc::new(DashMap::new()),
            rr_index: Arc::new(DashMap::new()),
            sticky: Arc::new(DashMap::new()),
            traffic_tx,
            bind_errors: Arc::new(DashMap::new()),
            usage_logs: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn push_usage_log(&self, ev: TrafficEvent) {
        let mut g = self.usage_logs.lock().unwrap_or_else(|e| e.into_inner());
        g.push_front(ev);
        g.truncate(USAGE_LOG_LIMIT);
    }

    pub fn snapshot_usage_logs(&self) -> Vec<TrafficEvent> {
        self.usage_logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn seed_usage_logs(&self, rows: Vec<TrafficEvent>) {
        let mut g = self.usage_logs.lock().unwrap_or_else(|e| e.into_inner());
        if !g.is_empty() {
            return;
        }
        *g = rows.into();
    }
}

pub async fn load_service(db: &SqlitePool, id: &str) -> sqlx::Result<Option<ServiceNode>> {
    sqlx::query_as::<_, ServiceNode>("SELECT * FROM service_nodes WHERE id=?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn load_enabled_services(db: &SqlitePool) -> sqlx::Result<Vec<ServiceNode>> {
    sqlx::query_as::<_, ServiceNode>("SELECT * FROM service_nodes WHERE enabled=1")
        .fetch_all(db)
        .await
}


