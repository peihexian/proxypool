use crate::models::{ProxyNode, ServiceNode};
use crate::state::{AppState, StickyEntry};
use anyhow::{bail, Result};
use chrono::Utc;
use std::sync::atomic::{AtomicU64, Ordering};

pub async fn pick_node(
    state: &AppState,
    svc: &ServiceNode,
    client_ip: &str,
) -> Result<ProxyNode> {
    let nodes = eligible_nodes(state, svc).await?;
    if nodes.is_empty() {
        bail!("没有可用的代理节点（请检查节点池、过滤条件和隔离状态）");
    }

    let node = match svc.selection_strategy.as_str() {
        "lowest_latency" => nodes
            .into_iter()
            .min_by_key(|n| n.latency_ms.unwrap_or(i64::MAX))
            .unwrap(),
        "least_used" | "idle_first" => nodes
            .into_iter()
            .min_by_key(|n| n.last_used.clone().unwrap_or_default())
            .unwrap(),
        "sticky" => pick_sticky(state, svc, client_ip, nodes),
        _ => round_robin(state, svc, nodes),
    };

    let now = crate::db::now_rfc3339();
    let id = node.id.clone();
    let db = state.db.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE proxy_nodes SET last_used=? WHERE id=?")
            .bind(now)
            .bind(id)
            .execute(&db)
            .await;
    });
    Ok(node)
}

pub fn advertise_ip(node: &ProxyNode) -> String {
    node.exit_ip
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| node.host.clone())
}

fn pick_sticky(
    state: &AppState,
    svc: &ServiceNode,
    client_ip: &str,
    nodes: Vec<ProxyNode>,
) -> ProxyNode {
    let now = Utc::now().timestamp();
    let key = format!("{}:{client_ip}", svc.id);
    let prev = state.sticky.get(&key).map(|e| e.clone());
    if let Some(ent) = prev.as_ref() {
        if ent.expire_ts > now {
            if let Some(found) = nodes.iter().find(|n| n.id == ent.node_id) {
                return found.clone();
            }
        }
    }

    let node = pick_rotated(&state.rr_index, &svc.id, nodes, prev.as_ref());
    state.sticky.insert(
        key,
        StickyEntry {
            node_id: node.id.clone(),
            host: node.host.clone(),
            exit_ip: node.exit_ip.clone(),
            expire_ts: now + svc.sticky_ttl.max(30),
        },
    );
    node
}

fn pick_rotated(
    rr_index: &dashmap::DashMap<String, AtomicU64>,
    svc_id: &str,
    nodes: Vec<ProxyNode>,
    prev: Option<&StickyEntry>,
) -> ProxyNode {
    let Some(prev) = prev else {
        return round_robin_with(rr_index, svc_id, nodes);
    };
    let different_egress: Vec<ProxyNode> = nodes
        .iter()
        .filter(|n| n.id != prev.node_id && !same_egress(n, prev))
        .cloned()
        .collect();
    if !different_egress.is_empty() {
        return round_robin_with(rr_index, svc_id, different_egress);
    }
    let different_id: Vec<ProxyNode> = nodes
        .iter()
        .filter(|n| n.id != prev.node_id)
        .cloned()
        .collect();
    if !different_id.is_empty() {
        return round_robin_with(rr_index, svc_id, different_id);
    }
    round_robin_with(rr_index, svc_id, nodes)
}

fn same_egress(node: &ProxyNode, prev: &StickyEntry) -> bool {
    if node.host == prev.host {
        return true;
    }
    match (&node.exit_ip, &prev.exit_ip) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => a == b,
        _ => false,
    }
}

fn round_robin(state: &AppState, svc: &ServiceNode, nodes: Vec<ProxyNode>) -> ProxyNode {
    round_robin_with(&state.rr_index, &svc.id, nodes)
}

fn round_robin_with(
    rr_index: &dashmap::DashMap<String, AtomicU64>,
    svc_id: &str,
    nodes: Vec<ProxyNode>,
) -> ProxyNode {
    let idx = rr_index
        .entry(svc_id.to_string())
        .or_insert_with(|| AtomicU64::new(0));
    let i = idx.fetch_add(1, Ordering::Relaxed) as usize % nodes.len();
    nodes[i].clone()
}

async fn eligible_nodes(state: &AppState, svc: &ServiceNode) -> Result<Vec<ProxyNode>> {
    let pool_ids: Vec<String> = serde_json::from_str(&svc.pool_ids).unwrap_or_default();
    let countries: Vec<String> = serde_json::from_str::<Vec<String>>(&svc.filter_countries)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.to_uppercase())
        .collect();
    let asns: Vec<i64> = serde_json::from_str(&svc.filter_asns).unwrap_or_default();
    let now = crate::db::now_rfc3339();

    let mut nodes: Vec<ProxyNode> = if pool_ids.is_empty() {
        sqlx::query_as(
            r#"SELECT n.* FROM proxy_nodes n
               JOIN node_pools p ON p.id = n.pool_id
               WHERE p.enabled=1 AND n.status IN ('active','pending')
               AND (n.isolated_until IS NULL OR n.isolated_until < ?)
               ORDER BY n.host, n.port, n.id"#,
        )
        .bind(&now)
        .fetch_all(&state.db)
        .await?
    } else {
        let placeholders = pool_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            r#"SELECT n.* FROM proxy_nodes n
               JOIN node_pools p ON p.id = n.pool_id
               WHERE p.enabled=1 AND n.pool_id IN ({placeholders})
               AND n.status IN ('active','pending')
               AND (n.isolated_until IS NULL OR n.isolated_until < ?)
               ORDER BY n.host, n.port, n.id"#
        );
        let mut q = sqlx::query_as(&sql);
        for id in &pool_ids {
            q = q.bind(id);
        }
        q.bind(&now).fetch_all(&state.db).await?
    };

    if !countries.is_empty() {
        nodes.retain(|n| {
            n.country_code
                .as_ref()
                .map(|c| countries.iter().any(|x| x.eq_ignore_ascii_case(c)))
                .unwrap_or(false)
        });
    }
    if !asns.is_empty() {
        nodes.retain(|n| n.asn.map(|a| asns.contains(&a)).unwrap_or(false));
    }
    if svc.filter_residential == 1 {
        nodes.retain(|n| n.is_residential == 1);
    }
    Ok(nodes)
}

pub async fn count_available(state: &AppState, svc: &ServiceNode) -> i64 {
    eligible_nodes(state, svc)
        .await
        .map(|v| v.len() as i64)
        .unwrap_or(0)
}
