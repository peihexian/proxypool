pub mod chain;
pub mod http;
pub mod selector;
pub mod socks5;

use crate::models::ServiceNode;
use crate::state::AppState;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

pub async fn start_all(state: &AppState) {
    match crate::state::load_enabled_services(&state.db).await {
        Ok(list) => {
            for svc in list {
                if let Err(e) = start_service(state, &svc).await {
                    tracing::error!("start service {} failed: {e}", svc.name);
                    state.bind_errors.insert(svc.id.clone(), e.to_string());
                }
            }
        }
        Err(e) => tracing::error!("load services: {e}"),
    }
}

pub async fn restart_service(state: &AppState, id: &str) -> anyhow::Result<()> {
    stop_service(state, id).await;
    state.bind_errors.remove(id);
    if let Some(svc) = crate::state::load_service(&state.db, id).await? {
        if svc.enabled == 1 {
            if let Err(e) = start_service(state, &svc).await {
                state.bind_errors.insert(id.to_string(), e.to_string());
                return Err(e);
            }
        }
    }
    Ok(())
}

pub async fn stop_service(state: &AppState, id: &str) {
    if let Some((_, handle)) = state.listeners.remove(id) {
        let _ = handle.abort.send(());
        tokio::time::timeout(Duration::from_secs(2), handle.task)
            .await
            .ok();
    }
}

pub async fn start_service(state: &AppState, svc: &ServiceNode) -> anyhow::Result<()> {
    stop_service(state, &svc.id).await;
    let addr = format!("{}:{}", svc.listen_host, svc.listen_port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("service {} listening on {addr}", svc.name);

    let (abort_tx, mut abort_rx) = broadcast::channel::<()>(1);
    let state2 = state.clone();
    let svc2 = svc.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = abort_rx.recv() => {
                    tracing::info!("service {} stopped", svc2.name);
                    break;
                }
                acc = listener.accept() => {
                    match acc {
                        Ok((stream, peer)) => {
                            let st = state2.clone();
                            let s = svc2.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(st, s, stream, peer).await {
                                    tracing::debug!("client {}: {e}", peer);
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("accept error: {e}");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }
    });

    state.listeners.insert(
        svc.id.clone(),
        crate::state::ListenerHandle {
            abort: abort_tx,
            task,
        },
    );
    Ok(())
}

async fn handle_client(
    state: AppState,
    svc: ServiceNode,
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
) -> anyhow::Result<()> {
    stream.set_nodelay(true).ok();
    let mut peek = [0u8; 1];
    let n = stream.peek(&mut peek).await?;
    if n == 0 {
        return Ok(());
    }
    if peek[0] == 0x05 {
        socks5::handle(state, svc, stream, peer).await
    } else {
        http::handle(state, svc, stream, peer).await
    }
}
