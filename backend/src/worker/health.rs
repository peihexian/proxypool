use crate::db::now_rfc3339;
use crate::models::{DetectionPolicy, NodePool, ProxyNode};
use crate::state::{AppState, TrafficEvent, TRAFFIC_KIND_PROBE};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use regex::Regex;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::interval;

pub async fn run(state: AppState) {
    let mut tick = interval(Duration::from_secs(15));
    loop {
        tick.tick().await;
        restore_expired(&state).await;
        if let Err(e) = check_due(&state).await {
            tracing::debug!("health tick: {e}");
        }
    }
}

pub async fn check_node(state: &AppState, node: &ProxyNode, policy: &DetectionPolicy) -> anyhow::Result<()> {
    let report = probe(node, policy).await;
    let latency = report.latency_ms;
    // 探测无论成败都已经在上游产生了流量，先入账再处理结果。
    record_probe(state, node, &policy.check_url, report.up, report.down);
    match report.result {
        Ok(exit_ip) => {
            let geo = state.geoip.lookup_str(&exit_ip).unwrap_or_default();
            sqlx::query(
                r#"UPDATE proxy_nodes SET
                    status='active', fail_count=0, isolated_until=NULL,
                    latency_ms=?, last_check=?, next_check=NULL, exit_ip=?,
                    country=?, country_code=?, asn=?, asn_org=?, is_residential=?
                   WHERE id=?"#,
            )
            .bind(latency)
            .bind(now_rfc3339())
            .bind(&exit_ip)
            .bind(&geo.country)
            .bind(&geo.country_code)
            .bind(geo.asn)
            .bind(&geo.asn_org)
            .bind(if geo.is_residential { 1 } else { 0 })
            .bind(&node.id)
            .execute(&state.db)
            .await?;
        }
        Err(e) => {
            tracing::debug!("node {} check fail: {e}", node.id);
            apply_fail(state, node, policy).await?;
        }
    }
    Ok(())
}

async fn check_due(state: &AppState) -> anyhow::Result<()> {
    let pools: Vec<NodePool> =
        sqlx::query_as("SELECT * FROM node_pools WHERE enabled=1")
            .fetch_all(&state.db)
            .await?;
    let policies: Vec<DetectionPolicy> = sqlx::query_as("SELECT * FROM detection_policies")
        .fetch_all(&state.db)
        .await?;
    let default_policy = policies.first().cloned();
    let now = Utc::now();

    for pool in pools {
        if state.is_pool_syncing(&pool.id) {
            continue;
        }
        let policy = pool
            .detection_policy_id
            .as_ref()
            .and_then(|id| policies.iter().find(|p| &p.id == id).cloned())
            .or(default_policy.clone());
        let Some(policy) = policy else {
            continue;
        };
        let nodes: Vec<ProxyNode> = sqlx::query_as(
            "SELECT * FROM proxy_nodes WHERE pool_id=? AND status != 'disabled' LIMIT 500",
        )
        .bind(&pool.id)
        .fetch_all(&state.db)
        .await?;

        let mut due = Vec::new();
        for n in nodes {
            if n.status == "isolated"
                && n.isolated_until.is_some()
                && policy.on_fail == "isolate"
            {
                continue;
            }
            if should_check(&n, &policy, now) {
                due.push(n);
            }
        }
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let mut handles = Vec::new();
        for n in due.into_iter().take(40) {
            let permit = sem.clone().acquire_owned().await?;
            let st = state.clone();
            let pol = policy.clone();
            handles.push(tokio::spawn(async move {
                let _p = permit;
                let _ = check_node(&st, &n, &pol).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    }
    Ok(())
}

fn should_check(n: &ProxyNode, policy: &DetectionPolicy, now: DateTime<Utc>) -> bool {
    if let Some(next) = &n.next_check {
        if let Ok(t) = DateTime::parse_from_rfc3339(next) {
            return now >= t.with_timezone(&Utc);
        }
    }
    match &n.last_check {
        None => true,
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|t| {
                now.signed_duration_since(t.with_timezone(&Utc)).num_seconds()
                    >= policy.check_interval
            })
            .unwrap_or(true),
    }
}

async fn apply_fail(state: &AppState, node: &ProxyNode, policy: &DetectionPolicy) -> anyhow::Result<()> {
    let fails = node.fail_count + 1;
    let now = Utc::now();
    let (status, isolated_until, next_check) = if fails >= policy.max_fails {
        match policy.on_fail.as_str() {
            "isolate" => ("isolated".to_string(), None, None),
            "backoff" => {
                let wait = policy.backoff_base_seconds * (1i64 << (fails - 1).min(8));
                let next = (now + ChronoDuration::seconds(wait)).to_rfc3339();
                ("pending".to_string(), None, Some(next))
            }
            _ => {
                let until = (now + ChronoDuration::seconds(policy.isolate_seconds)).to_rfc3339();
                ("isolated".to_string(), Some(until.clone()), Some(until))
            }
        }
    } else {
        let wait = if policy.on_fail == "backoff" {
            policy.backoff_base_seconds * (1i64 << (fails - 1).min(8))
        } else {
            30
        };
        let next = (now + ChronoDuration::seconds(wait)).to_rfc3339();
        ("pending".to_string(), None, Some(next))
    };

    sqlx::query(
        "UPDATE proxy_nodes SET fail_count=?, status=?, isolated_until=?, last_check=?, next_check=? WHERE id=?",
    )
    .bind(fails)
    .bind(status)
    .bind(isolated_until)
    .bind(now_rfc3339())
    .bind(next_check)
    .bind(&node.id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn restore_expired(state: &AppState) {
    let now = now_rfc3339();
    let _ = sqlx::query(
        "UPDATE proxy_nodes SET status='pending', isolated_until=NULL WHERE status='isolated' AND isolated_until IS NOT NULL AND isolated_until < ?",
    )
    .bind(now)
    .execute(&state.db)
    .await;
}

/// 一次探测的完整结果：成败、经过上游的字节数、请求往返延迟。
/// 字节数无论如何都要带回来，因为探测失败时同样消耗了上游流量。
pub(crate) struct ProbeReport {
    pub result: anyhow::Result<String>,
    pub up: u64,
    pub down: u64,
    pub latency_ms: i64,
}

pub(crate) async fn probe(node: &ProxyNode, policy: &DetectionPolicy) -> ProbeReport {
    let relay = match CountingRelay::start(&node.host, node.port as u16).await {
        Ok(r) => r,
        Err(e) => {
            return ProbeReport {
                result: Err(e),
                up: 0,
                down: 0,
                latency_ms: 0,
            }
        }
    };
    let started = Instant::now();
    let result = probe_through(node, policy, relay.addr).await;
    // 只统计请求往返，不含中继建立与下面等待尾部字节的时间
    let latency_ms = started.elapsed().as_millis() as i64;
    // 给中继一点时间，把还在路上的尾部字节（TLS close_notify 之类）计上
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (up, down) = relay.totals();
    ProbeReport {
        result,
        up,
        down,
        latency_ms,
    }
}

async fn probe_through(
    node: &ProxyNode,
    policy: &DetectionPolicy,
    relay_addr: SocketAddr,
) -> anyhow::Result<String> {
    let proxy_url = node_to_proxy_url(node, relay_addr)?;
    let proxy = reqwest::Proxy::all(&proxy_url)?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_millis(policy.timeout_ms.max(1000) as u64))
        .danger_accept_invalid_certs(true)
        .build()?;
    let resp = client.get(&policy.check_url).send().await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("检测 URL 返回 {status}");
    }
    extract_ip(&body).ok_or_else(|| anyhow::anyhow!("无法从响应解析出口 IP"))
}

/// 把一次探测的用量记成 `kind='probe'` 的流量。上游平台照常计费，
/// 不记的话看板就会明显低于上游后台。只入库，不占用内存里最近 100 条客户端日志。
pub(crate) fn record_probe(
    state: &AppState,
    node: &ProxyNode,
    check_url: &str,
    up: u64,
    down: u64,
) {
    if up == 0 && down == 0 {
        return;
    }
    let ev = TrafficEvent {
        service_id: String::new(),
        client_ip: String::new(),
        proxy_ip: node.exit_ip.clone().unwrap_or_else(|| node.host.clone()),
        dest: dest_of(check_url),
        protocol: node.protocol.clone(),
        kind: TRAFFIC_KIND_PROBE.to_string(),
        bytes_up: up,
        bytes_down: down,
        ts: chrono::Utc::now().timestamp(),
    };
    let _ = state.traffic_tx.try_send(ev);
}

fn dest_of(check_url: &str) -> String {
    match url::Url::parse(check_url) {
        Ok(u) => match (u.host_str(), u.port_or_known_default()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => check_url.to_string(),
        },
        Err(_) => check_url.to_string(),
    }
}

/// 探测用的本地一次性 TCP 中继：reqwest 仍然照旧"走代理"，
/// 只是把代理地址指向它，由它转发到真正的上游节点并统计字节数。
/// 这样 TLS / gzip / 重定向等行为完全不变，却能拿到真实用量。
struct CountingRelay {
    addr: SocketAddr,
    up: Arc<AtomicU64>,
    down: Arc<AtomicU64>,
    task: JoinHandle<()>,
}

impl CountingRelay {
    async fn start(host: &str, port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let up = Arc::new(AtomicU64::new(0));
        let down = Arc::new(AtomicU64::new(0));
        let (relay_up, relay_down) = (up.clone(), down.clone());
        let host = host.to_string();
        let task = tokio::spawn(async move {
            let mut set = JoinSet::new();
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    break;
                };
                let (u, d) = (relay_up.clone(), relay_down.clone());
                let h = host.clone();
                set.spawn(async move {
                    if let Ok(server) = TcpStream::connect((h.as_str(), port)).await {
                        crate::proxy::chain::relay_counted(client, server, &u, &d).await;
                    }
                });
                // 回收已结束的任务，别让 JoinSet 无限长
                while set.try_join_next().is_some() {}
            }
            set.shutdown().await;
        });
        Ok(Self {
            addr,
            up,
            down,
            task,
        })
    }

    fn totals(&self) -> (u64, u64) {
        (
            self.up.load(Ordering::Relaxed),
            self.down.load(Ordering::Relaxed),
        )
    }
}

impl Drop for CountingRelay {
    fn drop(&mut self) {
        // 中继任务随 JoinSet 一起结束，不会泄漏
        self.task.abort();
    }
}

fn node_to_proxy_url(node: &ProxyNode, addr: SocketAddr) -> anyhow::Result<String> {
    let scheme = match node.protocol.as_str() {
        "socks5h" => "socks5h",
        "socks5" => "socks5",
        _ => "http",
    };
    let auth = match (&node.username, &node.password) {
        (Some(u), Some(p)) if !u.is_empty() => {
            format!("{}:{}@", urlencoding(u), urlencoding(p))
        }
        (Some(u), _) if !u.is_empty() => format!("{}@", urlencoding(u)),
        _ => String::new(),
    };
    Ok(format!("{scheme}://{auth}{}:{}", addr.ip(), addr.port()))
}

fn urlencoding(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn extract_ip(body: &str) -> Option<String> {
    let t = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        for k in ["ip", "origin", "query", "origin_ip"] {
            if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
                let ip = s.split(',').next()?.trim();
                if ip.parse::<std::net::IpAddr>().is_ok() {
                    return Some(ip.to_string());
                }
            }
        }
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\b(\d{1,3}(?:\.\d{1,3}){3})\b").unwrap());
    re.captures(t)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
}
