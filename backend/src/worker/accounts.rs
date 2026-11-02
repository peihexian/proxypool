use crate::state::AppState;
use tokio::time::{interval, Duration};

/// 账号固定 IP 的巡检。节点池同步 / 健康检查 / 手动改状态都会改变
/// 可用出口 IP，这里定期把失效或不再唯一的绑定重新分配。
pub async fn run(state: AppState) {
    // 启动时先对齐一次，避免重启后账号还指着已经删除的节点。
    if let Err(e) = crate::accounts::reconcile_all(&state).await {
        tracing::warn!("account reconcile at startup: {e:#}");
    }
    let mut tick = interval(Duration::from_secs(15));
    loop {
        tick.tick().await;
        match crate::accounts::reconcile_all(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("reassigned {n} proxy account bindings"),
            Err(e) => tracing::warn!("account reconcile: {e:#}"),
        }
    }
}
