//! 批量账号与固定出口 IP 的分配逻辑。
//!
//! 每个账号绑定一个“出口槽位”。槽位按出口 IP 去重：不同的上游节点如果
//! 出口 IP 相同，只算一个 IP。分配时优先让每个账号拿到唯一出口 IP；只有当
//! 账号数大于可用 IP 数时才复用，并且尽量均摊。IP 池或节点状态变化后，
//! [`reconcile_all`] / [`reconcile_service`] 会把失效的绑定重新分配。

use crate::db::now_rfc3339;
use crate::models::{ProxyAccount, ProxyNode, ServiceNode};
use crate::proxy::selector;
use crate::state::AppState;
use anyhow::{bail, Result};
use std::collections::HashMap;

/// 通过代理入口认证的身份：服务节点自身的账号，或批量账号。
pub enum Identity {
    Service,
    Account(ProxyAccount),
}

/// 出口 IP 的稳定标识。客户端看到的是出口 IP，所以优先用它去重；
/// 节点还没探测成功过时才退回到 host:port。
pub fn egress_key(node: &ProxyNode) -> String {
    match node
        .exit_ip
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(ip) => format!("ip:{ip}"),
        None => format!("node:{}:{}", node.host, node.port),
    }
}

/// 先比对服务节点自身的账号，再查批量账号表。
pub async fn authenticate(
    state: &AppState,
    svc: &ServiceNode,
    username: &str,
    password: &str,
) -> Option<Identity> {
    if username == svc.username && password == svc.password {
        return Some(Identity::Service);
    }
    let account: Option<ProxyAccount> = sqlx::query_as(
        "SELECT * FROM proxy_accounts WHERE service_id=? AND username=? AND password=? AND enabled=1",
    )
    .bind(&svc.id)
    .bind(username)
    .bind(password)
    .fetch_optional(&state.db)
    .await
    .ok()?;
    account.map(Identity::Account)
}

/// 账号请求优先走固定绑定的节点；绑定失效时先重新分配再取一次，
/// 避免账号在 IP 池刚更新、后台巡检还没跑到时直接 502。
pub async fn pick_for_account(
    state: &AppState,
    svc: &ServiceNode,
    account: &ProxyAccount,
) -> Result<ProxyNode> {
    if let Some(node_id) = account.node_id.as_ref() {
        if let Some(node) = assigned_active_node(state, svc, node_id).await? {
            return Ok(node);
        }
    }

    let _guard = state.account_lock.lock().await;
    reconcile_service_locked(state, svc).await?;
    let fresh: Option<ProxyAccount> = sqlx::query_as("SELECT * FROM proxy_accounts WHERE id=?")
        .bind(&account.id)
        .fetch_optional(&state.db)
        .await?;
    if let Some(fresh) = fresh {
        if let Some(node_id) = fresh.node_id.as_ref() {
            if let Some(node) = assigned_active_node(state, svc, node_id).await? {
                return Ok(node);
            }
        }
    }
    bail!(
        "账号 {} 暂无可用的代理 IP（节点池可能正在更新或全部被隔离）",
        account.username
    )
}

/// 账号绑定节点是否仍然可用：节点启用、状态 active、未隔离，且仍满足服务过滤条件。
async fn assigned_active_node(
    state: &AppState,
    svc: &ServiceNode,
    node_id: &str,
) -> Result<Option<ProxyNode>> {
    let pool_ids: Vec<String> = serde_json::from_str(&svc.pool_ids).unwrap_or_default();
    let mut sql = String::from(
        r#"SELECT n.* FROM proxy_nodes n
           JOIN node_pools p ON p.id = n.pool_id
           WHERE n.id=? AND p.enabled=1 AND n.status='active'
           AND (n.isolated_until IS NULL OR n.isolated_until < ?)"#,
    );
    if !pool_ids.is_empty() {
        let placeholders = pool_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND n.pool_id IN ({placeholders})"));
    }
    sql.push_str(" LIMIT 1");

    let mut q = sqlx::query_as::<_, ProxyNode>(&sql)
        .bind(node_id)
        .bind(now_rfc3339());
    for id in &pool_ids {
        q = q.bind(id);
    }
    let node = q.fetch_optional(&state.db).await?;
    let Some(node) = node else {
        return Ok(None);
    };
    let (countries, asns, residential) = selector::service_filters(svc);
    if selector::matches_filters(&node, &countries, &asns, residential) {
        Ok(Some(node))
    } else {
        Ok(None)
    }
}

/// 服务可用的、已通过检测的节点。
pub async fn active_eligible_nodes(state: &AppState, svc: &ServiceNode) -> Result<Vec<ProxyNode>> {
    let nodes = selector::eligible_nodes(state, svc).await?;
    Ok(nodes.into_iter().filter(|n| n.status == "active").collect())
}

#[derive(Debug, Default)]
struct AssignmentPlan {
    /// 每个不同出口 IP 的代表节点，值是 nodes 里的下标。
    reps: Vec<usize>,
    /// 每个账号对应的出口槽位，值是 reps 的下标。
    slots: Vec<Option<usize>>,
}

fn build_slots(
    nodes: &[ProxyNode],
) -> (Vec<usize>, HashMap<String, usize>, HashMap<String, usize>) {
    let mut reps = Vec::new();
    let mut by_key = HashMap::new();
    let mut by_node = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let key = egress_key(node);
        let slot = match by_key.get(&key).copied() {
            Some(slot) => slot,
            None => {
                let slot = reps.len();
                reps.push(i);
                by_key.insert(key, slot);
                slot
            }
        };
        by_node.insert(node.id.clone(), slot);
    }
    (reps, by_key, by_node)
}

/// 核心分配算法，纯函数，便于单测。
///
/// A. 保留仍然有效的旧绑定（同一个出口 IP 只保留第一个账号）；
/// B. 未绑定 / 失效的账号优先占用没人用的出口 IP，不够时才挑使用最少的；
/// C. IP 池扩容后，把复用中的账号迁移到空闲 IP，尽量恢复一对一；
/// D. 仍然需要复用时，把账号从拥挤的 IP 挪到空闲少的 IP，尽量均摊。
fn plan_assignments(accounts: &[ProxyAccount], nodes: &[ProxyNode]) -> AssignmentPlan {
    let (reps, by_key, by_node) = build_slots(nodes);
    let m = reps.len();
    let n = accounts.len();
    let mut slots = vec![None; n];
    if m == 0 || n == 0 {
        return AssignmentPlan { reps, slots };
    }

    let mut used = vec![0usize; m];

    // A
    for (i, account) in accounts.iter().enumerate() {
        let current = account
            .node_id
            .as_ref()
            .and_then(|id| by_node.get(id).copied())
            .or_else(|| {
                account
                    .exit_ip
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .and_then(|ip| by_key.get(&format!("ip:{ip}")).copied())
            });
        if let Some(slot) = current {
            if used[slot] == 0 {
                slots[i] = Some(slot);
                used[slot] = 1;
            }
        }
    }

    // B
    for slot in slots.iter_mut() {
        if slot.is_some() {
            continue;
        }
        let chosen = (0..m)
            .min_by_key(|&s| used[s])
            .expect("m > 0 checked above");
        *slot = Some(chosen);
        used[chosen] += 1;
    }

    // C
    for empty in 0..m {
        if used[empty] > 0 {
            continue;
        }
        let Some(crowded) = (0..m).filter(|&s| used[s] > 1).max_by_key(|&s| used[s]) else {
            break;
        };
        let Some(ai) = (0..n).rev().find(|&i| slots[i] == Some(crowded)) else {
            continue;
        };
        slots[ai] = Some(empty);
        used[crowded] -= 1;
        used[empty] = 1;
    }

    // D
    let high = n.div_ceil(m);
    while let Some(from) = (0..m).filter(|&s| used[s] > high).max_by_key(|&s| used[s]) {
        let Some(to) = (0..m).filter(|&s| used[s] < high).min_by_key(|&s| used[s]) else {
            break;
        };
        let Some(ai) = (0..n).rev().find(|&i| slots[i] == Some(from)) else {
            break;
        };
        slots[ai] = Some(to);
        used[from] -= 1;
        used[to] += 1;
    }

    AssignmentPlan { reps, slots }
}

/// 重新分配一个服务下的全部账号。
pub async fn reconcile_service(state: &AppState, svc: &ServiceNode) -> Result<usize> {
    let _guard = state.account_lock.lock().await;
    reconcile_service_locked(state, svc).await
}

async fn reconcile_service_locked(state: &AppState, svc: &ServiceNode) -> Result<usize> {
    let accounts: Vec<ProxyAccount> = sqlx::query_as(
        "SELECT * FROM proxy_accounts WHERE service_id=? AND enabled=1 ORDER BY rowid",
    )
    .bind(&svc.id)
    .fetch_all(&state.db)
    .await?;
    if accounts.is_empty() {
        return Ok(0);
    }
    let nodes = active_eligible_nodes(state, svc).await?;
    // 池子暂时没有可用节点时保留上一次的绑定，等节点恢复后还能继续用。
    if nodes.is_empty() {
        return Ok(0);
    }

    let plan = plan_assignments(&accounts, &nodes);
    let now = now_rfc3339();
    let mut changed = 0usize;
    for (account, slot) in accounts.iter().zip(plan.slots.iter()) {
        let Some(slot) = slot else { continue };
        let target = &nodes[plan.reps[*slot]];
        let target_exit = target.exit_ip.clone().filter(|s| !s.trim().is_empty());
        let same_node = account.node_id.as_deref() == Some(target.id.as_str());
        if same_node && account.exit_ip == target_exit {
            continue;
        }
        sqlx::query("UPDATE proxy_accounts SET node_id=?, exit_ip=?, updated_at=? WHERE id=?")
            .bind(&target.id)
            .bind(&target_exit)
            .bind(&now)
            .bind(&account.id)
            .execute(&state.db)
            .await?;
        changed += 1;
    }
    Ok(changed)
}

/// 对全部有账号的服务做一次巡检式重新分配。
pub async fn reconcile_all(state: &AppState) -> Result<usize> {
    let _guard = state.account_lock.lock().await;
    let services: Vec<ServiceNode> = sqlx::query_as(
        r#"SELECT s.* FROM service_nodes s
           WHERE EXISTS (
               SELECT 1 FROM proxy_accounts a
               WHERE a.service_id = s.id AND a.enabled = 1
           )"#,
    )
    .fetch_all(&state.db)
    .await?;
    let mut changed = 0usize;
    for svc in services {
        match reconcile_service_locked(state, &svc).await {
            Ok(n) => changed += n,
            Err(e) => tracing::warn!("reconcile accounts for {}: {e:#}", svc.name),
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, host: &str, exit_ip: &str) -> ProxyNode {
        ProxyNode {
            id: id.into(),
            pool_id: "pool".into(),
            protocol: "http".into(),
            host: host.into(),
            port: 8080,
            username: None,
            password: None,
            raw: None,
            exit_ip: Some(exit_ip.into()),
            country: None,
            country_code: None,
            asn: None,
            asn_org: None,
            is_residential: 0,
            latency_ms: None,
            fail_count: 0,
            status: "active".into(),
            isolated_until: None,
            last_check: None,
            last_used: None,
            next_check: None,
            last_error: None,
            manual_isolated: 0,
            created_at: String::new(),
        }
    }

    fn account(index: usize, node_id: Option<&str>, exit_ip: Option<&str>) -> ProxyAccount {
        ProxyAccount {
            id: format!("acc-{index}"),
            service_id: "svc".into(),
            username: format!("user{index}"),
            password: "pw".into(),
            node_id: node_id.map(str::to_string),
            exit_ip: exit_ip.map(str::to_string),
            enabled: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn unique_slots(plan: &AssignmentPlan) -> usize {
        plan.slots
            .iter()
            .flatten()
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    #[test]
    fn twenty_accounts_ten_ips_share_but_use_every_ip() {
        let nodes: Vec<ProxyNode> = (0..10)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("10.0.0.{}", i + 1),
                    &format!("1.1.1.{}", i + 1),
                )
            })
            .collect();
        let accounts: Vec<ProxyAccount> = (0..20).map(|i| account(i, None, None)).collect();
        let plan = plan_assignments(&accounts, &nodes);
        assert_eq!(plan.reps.len(), 10);
        assert_eq!(unique_slots(&plan), 10);
        let mut usage = vec![0usize; 10];
        for s in plan.slots.iter().flatten() {
            usage[*s] += 1;
        }
        assert!(
            usage.iter().all(|u| *u == 2),
            "usage should be balanced: {usage:?}"
        );
    }

    #[test]
    fn enough_ips_means_every_account_unique() {
        let nodes: Vec<ProxyNode> = (0..20)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("10.0.0.{}", i + 1),
                    &format!("1.1.1.{}", i + 1),
                )
            })
            .collect();
        let accounts: Vec<ProxyAccount> = (0..20).map(|i| account(i, None, None)).collect();
        let plan = plan_assignments(&accounts, &nodes);
        assert_eq!(unique_slots(&plan), 20);
    }

    #[test]
    fn duplicate_exit_ips_count_as_one_slot() {
        let nodes = vec![
            node("a", "10.0.0.1", "9.9.9.9"),
            node("b", "10.0.0.2", "9.9.9.9"),
            node("c", "10.0.0.3", "8.8.8.8"),
        ];
        let accounts: Vec<ProxyAccount> = (0..2).map(|i| account(i, None, None)).collect();
        let plan = plan_assignments(&accounts, &nodes);
        assert_eq!(plan.reps.len(), 2);
        assert_eq!(unique_slots(&plan), 2);
        assert_ne!(plan.slots[0], plan.slots[1]);
    }

    #[test]
    fn growing_pool_spreads_shared_accounts() {
        let before: Vec<ProxyNode> = (0..10)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("10.0.0.{}", i + 1),
                    &format!("1.1.1.{}", i + 1),
                )
            })
            .collect();
        let after: Vec<ProxyNode> = (0..20)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("10.0.0.{}", i + 1),
                    &format!("1.1.1.{}", i + 1),
                )
            })
            .collect();
        let mut accounts: Vec<ProxyAccount> = (0..20).map(|i| account(i, None, None)).collect();
        let first = plan_assignments(&accounts, &before);
        for i in 0..20 {
            let target = &before[first.reps[first.slots[i].unwrap()]];
            accounts[i].node_id = Some(target.id.clone());
            accounts[i].exit_ip = target.exit_ip.clone();
        }
        let plan = plan_assignments(&accounts, &after);
        assert_eq!(unique_slots(&plan), 20);
    }

    #[test]
    fn shrinking_pool_balances_reuse() {
        let before: Vec<ProxyNode> = (0..20)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("10.0.0.{}", i + 1),
                    &format!("1.1.1.{}", i + 1),
                )
            })
            .collect();
        let after: Vec<ProxyNode> = before.iter().take(10).cloned().collect();
        let mut accounts: Vec<ProxyAccount> = (0..20).map(|i| account(i, None, None)).collect();
        let first = plan_assignments(&accounts, &before);
        for i in 0..20 {
            let target = &before[first.reps[first.slots[i].unwrap()]];
            accounts[i].node_id = Some(target.id.clone());
            accounts[i].exit_ip = target.exit_ip.clone();
        }
        let plan = plan_assignments(&accounts, &after);
        assert_eq!(unique_slots(&plan), 10);
        let mut usage = vec![0usize; 10];
        for s in plan.slots.iter().flatten() {
            usage[*s] += 1;
        }
        assert!(
            usage.iter().all(|u| *u == 2),
            "usage should be balanced: {usage:?}"
        );
    }

    #[test]
    fn failed_node_is_reassigned_and_keeps_uniqueness() {
        let before: Vec<ProxyNode> = (0..4)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("10.0.0.{}", i + 1),
                    &format!("1.1.1.{}", i + 1),
                )
            })
            .collect();
        let after: Vec<ProxyNode> = before.iter().take(3).cloned().collect();
        let mut accounts: Vec<ProxyAccount> = (0..4).map(|i| account(i, None, None)).collect();
        let first = plan_assignments(&accounts, &before);
        for i in 0..4 {
            let target = &before[first.reps[first.slots[i].unwrap()]];
            accounts[i].node_id = Some(target.id.clone());
            accounts[i].exit_ip = target.exit_ip.clone();
        }
        let plan = plan_assignments(&accounts, &after);
        assert_eq!(unique_slots(&plan), 3);
        assert!(plan.slots.iter().all(|s| s.is_some()));
        // 失效节点绑定的账号必须被换到仍存活的节点
        assert!(plan.reps.iter().all(|i| after[*i].id != "n3"));
    }

    #[test]
    fn same_exit_ip_is_never_used_twice_with_enough_capacity() {
        let nodes = vec![
            node("a", "10.0.0.1", "5.5.5.5"),
            node("b", "10.0.0.2", "5.5.5.5"),
            node("c", "10.0.0.3", "6.6.6.6"),
            node("d", "10.0.0.4", "7.7.7.7"),
        ];
        let accounts: Vec<ProxyAccount> = (0..3).map(|i| account(i, None, None)).collect();
        let plan = plan_assignments(&accounts, &nodes);
        assert_eq!(unique_slots(&plan), 3);
    }
}
