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
    /// Pools currently refreshing a remote subscription. Health checks skip
    /// these so a long probe round cannot isolate the still-serving old nodes.
    pub syncing_pools: Arc<DashMap<String, ()>>,
}

/// Clears the pool's syncing flag even if the refresh errors or is cancelled.
pub struct PoolSyncGuard {
    pools: Arc<DashMap<String, ()>>,
    id: String,
}

impl Drop for PoolSyncGuard {
    fn drop(&mut self) {
        self.pools.remove(&self.id);
    }
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
            syncing_pools: Arc::new(DashMap::new()),
        }
    }

    pub fn enter_pool_sync(&self, pool_id: &str) -> PoolSyncGuard {
        self.syncing_pools.insert(pool_id.to_string(), ());
        PoolSyncGuard {
            pools: self.syncing_pools.clone(),
            id: pool_id.to_string(),
        }
    }

    pub fn is_pool_syncing(&self, pool_id: &str) -> bool {
        self.syncing_pools.contains_key(pool_id)
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

    /// Drop sticky bindings whose node was removed so the next CONNECT picks a live IP.
    pub fn drop_sticky_for_nodes(&self, node_ids: &std::collections::HashSet<String>) {
        if node_ids.is_empty() {
            return;
        }
        self.sticky.retain(|_, ent| !node_ids.contains(&ent.node_id));
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


