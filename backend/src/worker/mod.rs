pub mod health;
pub mod subscribe;
pub mod traffic;

use crate::state::AppState;
use crate::state::TrafficEvent;
use tokio::sync::mpsc;

pub fn spawn_all(state: AppState, traffic_rx: mpsc::Receiver<TrafficEvent>) {
    tokio::spawn(traffic::run(state.clone(), traffic_rx));
    tokio::spawn(subscribe::run(state.clone()));
    tokio::spawn(health::run(state));
}
