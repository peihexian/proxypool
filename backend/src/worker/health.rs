use crate::db::now_rfc3339;
use crate::models::{DetectionPolicy, NodePool, ProxyNode};
use crate::state::{AppState, TrafficEvent, TRAFFIC_KIND_PROBE};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{interval, timeout};

/// 同一时刻向上游发起的探测连接数。住宅网关普遍按"并发会话数"限流，
/// 8 并发正好踩线：一轮集中探测就会把整池打成集体超时。
const PROBE_CONCURRENCY: usize = 3;
/// 每池每轮最多探测多少个节点。剩下的下一轮继续，避免一轮占满整个 tick。
const MAX_PER_ROUND: usize = 40;
/// 只测"连不连得上上游"，3 秒足够；把"连不上"和"连上了但慢"分开。
const CONNECT_TIMEOUT_MS: u64 = 3000;
/// 样本太小不做"系统性故障"推断。
const SYSTEMIC_MIN_ATTEMPTS: usize = 4;
/// 响应体上限。超过这个长度基本是网关/目标返回的错误页，不是出口 IP。
const MAX_BODY_BYTES: usize = 8 * 1024;
/// 探测抖动的上限（毫秒）。所有节点都是同一批插入、同一批到期，
/// 不打散就会整池同时打上游。
const JITTER_MS: u64 = 1500;
/// 主检测目标不可达时整体切换的备用目标。只在"本机直连主目标失败"时启用，
/// 不会给每个节点都多跑一轮请求。
const FALLBACK_CHECK_URLS: &[&str] = &[
    "http://icanhazip.com",
    "http://ifconfig.me/ip",
    "http://api64.ipify.org",
];

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

/// 失败原因分类。只有 [`FailKind::Transport`] 才算"这个节点坏了"，
/// 其余两类是账号/目标侧的全局问题，隔离单个节点毫无意义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailKind {
    /// 上游代理本身的问题：连不上、超时、隧道被断开、上游连不到目标（坏出口）。
    Transport,
    /// 账号/白名单/网关策略：407、403、429、CONNECT 不被接受。
    Policy,
    /// 检测目标侧：目标返回非 2xx、响应体不是 IP、被 CDN 拦截。
    Target,
}

#[derive(Debug)]
pub(crate) struct ProbeFailure {
    pub kind: FailKind,
    pub msg: String,
    /// 换另一种 HTTP 代理请求形式（绝对 URI / CONNECT 隧道）可能成功。
    pub mode_mismatch: bool,
}

impl ProbeFailure {
    pub(crate) fn transport(msg: impl Into<String>) -> Self {
        Self {
            kind: FailKind::Transport,
            msg: msg.into(),
            mode_mismatch: false,
        }
    }

    fn policy(msg: impl Into<String>) -> Self {
        Self {
            kind: FailKind::Policy,
            msg: msg.into(),
            mode_mismatch: false,
        }
    }

    fn target(msg: impl Into<String>) -> Self {
        Self {
            kind: FailKind::Target,
            msg: msg.into(),
            mode_mismatch: false,
        }
    }

    fn mismatch(msg: impl Into<String>) -> Self {
        Self {
            kind: FailKind::Policy,
            msg: msg.into(),
            mode_mismatch: true,
        }
    }

    /// 只有这一类会累加 fail_count、并最终导致隔离。
    pub fn counts_against_node(&self) -> bool {
        self.kind == FailKind::Transport
    }
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

/// 一次探测的完整结果：成败、经过上游的字节数、请求往返延迟。
/// 字节数无论如何都要带回来，因为探测失败时同样消耗了上游流量。
pub(crate) struct ProbeReport {
    pub result: Result<String, ProbeFailure>,
    pub up: u64,
    pub down: u64,
    pub latency_ms: i64,
}

impl ProbeReport {
    fn failed(fail: ProbeFailure) -> Self {
        Self {
            result: Err(fail),
            up: 0,
            down: 0,
            latency_ms: 0,
        }
    }
}

/// 单节点检测：走一轮"只有一个节点"的批次，所以会走完整的分类与落库逻辑，
/// 但样本数不足，不会触发池级熔断。
pub async fn check_node(
    state: &AppState,
    node: &ProxyNode,
    policy: &DetectionPolicy,
) -> anyhow::Result<()> {
    check_batch(state, std::slice::from_ref(node), policy).await;
    Ok(())
}

async fn check_due(state: &AppState) -> anyhow::Result<()> {
    let pools: Vec<NodePool> = sqlx::query_as("SELECT * FROM node_pools WHERE enabled=1")
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
        // 手动隔离的节点由运维控制，自动巡检不碰。
        let nodes: Vec<ProxyNode> = sqlx::query_as(
            "SELECT * FROM proxy_nodes WHERE pool_id=? AND status != 'disabled' AND manual_isolated = 0 LIMIT 500",
        )
        .bind(&pool.id)
        .fetch_all(&state.db)
        .await?;

        let due: Vec<ProxyNode> = nodes
            .into_iter()
            // 永久隔离（on_fail='isolate'）的节点不再自动探测，交给人工恢复。
            .filter(|n| !(policy.on_fail == "isolate" && n.status == "isolated"))
            .filter(|n| should_check(n, &policy, now))
            .take(MAX_PER_ROUND)
            .collect();
        if due.is_empty() {
            continue;
        }
        check_batch(state, &due, &policy).await;
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

/// 一批节点的检测：并发探测 → 逐条落库 → 本轮汇总 → 决定是否真的隔离。
///
/// 池级熔断是这里的关键：如果一轮里超过一半的尝试都失败，或者检测目标本机
/// 都连不上，那说明是系统性故障（上游抖动、目标挂了、账号被限流），
/// 这一轮只降级记录，不写隔离。避免"上游抖 1 分钟 → 整池关 10 分钟"。
pub(crate) async fn check_batch(
    state: &AppState,
    nodes: &[ProxyNode],
    policy: &DetectionPolicy,
) -> (usize, usize) {
    if nodes.is_empty() {
        return (0, 0);
    }
    let check_url = effective_check_url(policy).await;
    let sem = Arc::new(Semaphore::new(PROBE_CONCURRENCY));
    let mut set = JoinSet::new();

    for n in nodes {
        let permit_sem = sem.clone();
        let st = state.clone();
        let pol = policy.clone();
        let node = n.clone();
        let url = check_url.clone();
        set.spawn(async move {
            let _permit = match permit_sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return None,
            };
            let jitter = jitter_ms(&node.id, JITTER_MS);
            if jitter > 0 {
                tokio::time::sleep(Duration::from_millis(jitter)).await;
            }
            let report = probe(&node, &pol, &url).await;
            // 探测无论成败都已经在上游产生了流量，先入账再处理结果。
            record_probe(&st, &node, &url, report.up, report.down);
            Some((node, report))
        });
    }

    let mut attempts = 0usize;
    let mut node_faults = 0usize;
    let mut soft_faults = 0usize;
    let mut candidates: Vec<(String, i64, String)> = Vec::new();

    while let Some(joined) = set.join_next().await {
        let Ok(Some((node, report))) = joined else {
            continue;
        };
        attempts += 1;
        match report.result {
            Ok(exit_ip) => apply_success(state, &node, &exit_ip, report.latency_ms).await,
            Err(fail) if fail.counts_against_node() => {
                node_faults += 1;
                tracing::debug!("node {} check fail: {}", node.id, fail.msg);
                if let Some(fails) = apply_fail(state, &node, policy, &fail).await {
                    candidates.push((node.id.clone(), fails, fail.msg));
                }
            }
            Err(fail) => {
                soft_faults += 1;
                tracing::debug!("node {} check soft-fail: {}", node.id, fail.msg);
                apply_soft_fail(state, &node, policy, &fail).await;
            }
        }
    }

    if attempts == 0 {
        return (0, 0);
    }
    if candidates.is_empty() {
        if node_faults + soft_faults > 0 {
            tracing::info!(
                "本轮探测 {attempts} 个：上游失败 {node_faults}，策略/目标问题 {soft_faults}，无需隔离"
            );
        }
        return (attempts, node_faults);
    }

    // 熔断判定：
    // - 样本够大且失败超过一半：上游整体在抖（限流/故障）。
    // - 有 4 次以上尝试且**全部**失败：目标挂了或本机出口有问题。
    // - 检测目标本机都不可达时，失败过半就不下结论（分不清是目标还是节点）。
    // 小池子（<8 次尝试）只认"全失败"，否则池子里仅有的坏节点会被放过。
    let target_down = !url_reachable(&check_url).await;
    let half_failed = node_faults * 2 >= attempts;
    let big_sample = attempts >= 8 && half_failed;
    let all_failed = attempts >= SYSTEMIC_MIN_ATTEMPTS && node_faults == attempts;
    if big_sample || all_failed || (target_down && half_failed) {
        let reason = if all_failed {
            "本轮全部失败"
        } else if big_sample {
            "本轮失败率超过一半"
        } else {
            "检测目标本机不可达且失败过半"
        };
        for (id, fails, msg) in &candidates {
            cap_strike(state, id, *fails, policy, msg).await;
        }
        tracing::warn!(
            "{reason}（{attempts} 次尝试失败 {node_faults} 次），判定为系统性问题，本轮不隔离 {} 个节点",
            candidates.len()
        );
    } else {
        for (id, _fails, msg) in candidates {
            isolate(state, &id, policy, &msg).await;
        }
    }
    (attempts, node_faults)
}

/// 成功：清空失败计数、清掉错误与手动隔离标记、刷新出口地理信息。
async fn apply_success(state: &AppState, node: &ProxyNode, exit_ip: &str, latency_ms: i64) {
    let geo = state.geoip.lookup_str(exit_ip).unwrap_or_default();
    let _ = sqlx::query(
        r#"UPDATE proxy_nodes SET
            status='active', fail_count=0, isolated_until=NULL, manual_isolated=0,
            latency_ms=?, last_check=?, next_check=NULL, exit_ip=?, last_error=NULL,
            country=?, country_code=?, asn=?, asn_org=?, is_residential=?
           WHERE id=?"#,
    )
    .bind(latency_ms)
    .bind(now_rfc3339())
    .bind(exit_ip)
    .bind(&geo.country)
    .bind(&geo.country_code)
    .bind(geo.asn)
    .bind(&geo.asn_org)
    .bind(if geo.is_residential { 1 } else { 0 })
    .bind(&node.id)
    .execute(&state.db)
    .await;
}

/// 上游失败：累计 strike，先降为 pending，等本轮汇总决定是否隔离。
/// 返回 Some(fails) 表示"够格隔离"，由批次的熔断逻辑裁决。
async fn apply_fail(
    state: &AppState,
    node: &ProxyNode,
    policy: &DetectionPolicy,
    fail: &ProbeFailure,
) -> Option<i64> {
    let fails = node.fail_count + 1;
    let max_fails = policy.max_fails.max(1);
    let terminal = fails >= max_fails && policy.on_fail != "backoff";
    let next = now_plus(retry_delay_secs(policy, fails));

    let _ = sqlx::query(
        "UPDATE proxy_nodes SET fail_count=?, status='pending', isolated_until=NULL, last_check=?, next_check=?, last_error=? WHERE id=?",
    )
    .bind(fails)
    .bind(now_rfc3339())
    .bind(&next)
    .bind(&fail.msg)
    .bind(&node.id)
    .execute(&state.db)
    .await;

    if terminal {
        Some(fails)
    } else {
        None
    }
}

/// 策略/目标类失败：不计 strike、不动状态（fail-open），只记录原因并按正常节奏重测。
/// 这类失败隔离节点没有意义——错的不是节点。
async fn apply_soft_fail(
    state: &AppState,
    node: &ProxyNode,
    policy: &DetectionPolicy,
    fail: &ProbeFailure,
) {
    // 没有拿到关于节点本身的结论，就按正常检测节奏重试，别额外加压上游。
    let next = now_plus(policy.check_interval.clamp(60, 1800));
    let _ = sqlx::query(
        "UPDATE proxy_nodes SET status=?, last_check=?, next_check=?, last_error=? WHERE id=?",
    )
    .bind(&node.status)
    .bind(now_rfc3339())
    .bind(&next)
    .bind(&fail.msg)
    .bind(&node.id)
    .execute(&state.db)
    .await;
}

/// 熔断生效时把刚累加的 strike 撤回去，避免下一轮"一次失败就隔离"。
async fn cap_strike(state: &AppState, id: &str, fails: i64, policy: &DetectionPolicy, msg: &str) {
    let cap = (policy.max_fails.max(1) - 1).max(0);
    let next = now_plus(retry_delay_secs(policy, 1).max(60));
    let note = format!("{msg}（本轮判定为系统性问题，未隔离）");
    let _ = sqlx::query(
        "UPDATE proxy_nodes SET status='pending', fail_count=?, next_check=?, last_error=? WHERE id=?",
    )
    .bind(fails.min(cap))
    .bind(&next)
    .bind(&note)
    .bind(id)
    .execute(&state.db)
    .await;
}

async fn isolate(state: &AppState, id: &str, policy: &DetectionPolicy, msg: &str) {
    let (until, next) = if policy.on_fail == "isolate" {
        (None, None)
    } else {
        let u = now_plus(policy.isolate_seconds.max(60));
        (Some(u.clone()), Some(u))
    };
    let _ = sqlx::query(
        "UPDATE proxy_nodes SET status='isolated', isolated_until=?, next_check=?, last_check=?, last_error=? WHERE id=?",
    )
    .bind(until)
    .bind(next)
    .bind(now_rfc3339())
    .bind(msg)
    .bind(id)
    .execute(&state.db)
    .await;
}

/// 临时隔离到期：恢复成待检测，并且**清零失败计数**。
/// 不清零的话节点回来后只要再失败一次就立刻又被关 10 分钟，看起来像永久隔离。
async fn restore_expired(state: &AppState) {
    let now = now_rfc3339();
    let _ = sqlx::query(
        "UPDATE proxy_nodes SET status='pending', isolated_until=NULL, fail_count=0, next_check=NULL
         WHERE status='isolated' AND manual_isolated=0 AND isolated_until IS NOT NULL AND isolated_until < ?",
    )
    .bind(now)
    .execute(&state.db)
    .await;
}

// ---------------------------------------------------------------------------
// 探测实现
// ---------------------------------------------------------------------------

/// 检测目标不可达时整体切到备用目标（带 60 秒缓存，避免每轮都试探）。
pub(crate) async fn effective_check_url(policy: &DetectionPolicy) -> String {
    if url_reachable(&policy.check_url).await {
        return policy.check_url.clone();
    }
    for alt in FALLBACK_CHECK_URLS {
        if url_reachable(alt).await {
            tracing::warn!(
                "检测 URL {} 本机不可达，本轮改用备用目标 {}",
                policy.check_url,
                alt
            );
            return (*alt).to_string();
        }
    }
    policy.check_url.clone()
}

static TARGET_CACHE: OnceLock<Mutex<std::collections::HashMap<String, (Instant, bool)>>> =
    OnceLock::new();

async fn url_reachable(url: &str) -> bool {
    let cache = TARGET_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    {
        let g = cache.lock().await;
        if let Some((at, ok)) = g.get(url) {
            if at.elapsed() < Duration::from_secs(60) {
                return *ok;
            }
        }
    }
    let ok = direct_reachable(url).await;
    let mut g = cache.lock().await;
    if g.len() > 16 {
        g.clear();
    }
    g.insert(url.to_string(), (Instant::now(), ok));
    ok
}

/// 不经任何代理、直接从本机测一次检测目标。
///
/// 这里只判断"网络层能不能到达"（拿到任何 HTTP 响应都算到达），
/// 因为要区分的是"目标整体挂了"和"这个节点坏了"。目标返回 403 之类的，
/// 由节点分类逻辑按 Target 处理，不该影响可达性判断。
async fn direct_reachable(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .no_proxy()
        .build()
    else {
        return false;
    };
    client.get(url).send().await.is_ok()
}

/// 探测一个节点。明文 HTTP 目标自己走一条连接（精确计费 + 精确分类），
/// HTTPS 目标保留原来的 reqwest 通道。
pub(crate) async fn probe(
    node: &ProxyNode,
    policy: &DetectionPolicy,
    check_url: &str,
) -> ProbeReport {
    let Some(target) = Target::parse(check_url) else {
        return ProbeReport::failed(ProbeFailure::target(format!("检测 URL 无效: {check_url}")));
    };
    if target.tls {
        return probe_tls(node, policy, check_url).await;
    }
    let started = Instant::now();
    let budget = Duration::from_millis(policy.timeout_ms.max(1000) as u64);
    let (result, up, down) = probe_plain(node, &target, budget).await;
    ProbeReport {
        result,
        up,
        down,
        latency_ms: started.elapsed().as_millis() as i64,
    }
}

#[derive(Debug, Clone)]
struct Target {
    host: String,
    port: u16,
    path: String,
    tls: bool,
    absolute: String,
}

impl Target {
    fn parse(url: &str) -> Option<Self> {
        let u = url::Url::parse(url).ok()?;
        let host = u.host_str()?.to_string();
        let tls = u.scheme().eq_ignore_ascii_case("https");
        let port = u.port_or_known_default()?;
        let mut path = u.path().to_string();
        if path.is_empty() {
            path.push('/');
        }
        if let Some(q) = u.query() {
            path.push('?');
            path.push_str(q);
        }
        let absolute = format!(
            "{}://{}{}",
            if tls { "https" } else { "http" },
            authority(&host, port, tls),
            path
        );
        Some(Self {
            host,
            port,
            path,
            tls,
            absolute,
        })
    }

    fn authority(&self) -> String {
        authority(&self.host, self.port, self.tls)
    }

    fn host_port(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn authority(host: &str, port: u16, tls: bool) -> String {
    let default_port = if tls { 443 } else { 80 };
    let h = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port {
        h
    } else {
        format!("{h}:{port}")
    }
}

async fn probe_plain(
    node: &ProxyNode,
    target: &Target,
    budget: Duration,
) -> (Result<String, ProbeFailure>, u64, u64) {
    match node.protocol.as_str() {
        "socks5" | "socks5h" => probe_socks5(node, target, budget).await,
        _ => probe_http_proxy(node, target, budget).await,
    }
}

/// HTTP 代理有两种请求形式，住宅网关往往只实现其中一种：
/// 1. 绝对 URI（`GET http://host/ HTTP/1.1`）—— 一次请求，多数网关都支持；
/// 2. CONNECT 隧道 —— 先建隧道再在隧道里发普通请求。
///
/// 先试绝对 URI（更省），只有出现"形式不兼容"信号时才换 CONNECT 重试。
/// 这样网关只支持 CONNECT 时不会再被判成节点死亡。
async fn probe_http_proxy(
    node: &ProxyNode,
    target: &Target,
    budget: Duration,
) -> (Result<String, ProbeFailure>, u64, u64) {
    let deadline = Instant::now() + budget;
    let mut up = 0u64;
    let mut down = 0u64;

    match http_proxy_attempt(node, target, Mode::Absolute, deadline, &mut up, &mut down).await {
        Ok(ip) => return (Ok(ip), up, down),
        Err(fail) if fail.mode_mismatch => {
            tracing::debug!(
                "node {}: 绝对 URI 形式失败（{}），改用 CONNECT 隧道重试",
                node.id,
                fail.msg
            );
        }
        Err(fail) => return (Err(fail), up, down),
    }

    let result = http_proxy_attempt(node, target, Mode::Tunnel, deadline, &mut up, &mut down).await;
    (result, up, down)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Absolute,
    Tunnel,
}

async fn http_proxy_attempt(
    node: &ProxyNode,
    target: &Target,
    mode: Mode,
    deadline: Instant,
    up: &mut u64,
    down: &mut u64,
) -> Result<String, ProbeFailure> {
    let addr = host_port(&node.host, node.port as u16);
    let mut stream = connect_upstream(&addr, deadline).await?;

    match mode {
        Mode::Absolute => {
            let req = absolute_request(node, target);
            write_counted(&mut stream, req.as_bytes(), deadline, up).await?;
            let (status, head, rest) = read_head(&mut stream, Vec::new(), deadline, down).await?;
            if status == 407 {
                return Err(ProbeFailure::policy("上游要求代理认证（407），账号或白名单不对"));
            }
            if matches!(status, 400 | 405 | 501) {
                return Err(ProbeFailure::mismatch(format!(
                    "上游不接受绝对 URI 请求（{status}）"
                )));
            }
            let body = read_body(&mut stream, &head, rest, deadline, down).await?;
            finish(status, &body)
        }
        Mode::Tunnel => {
            let req = connect_request(node, target);
            write_counted(&mut stream, req.as_bytes(), deadline, up).await?;
            let (code, _head, rest) = read_head(&mut stream, Vec::new(), deadline, down).await?;
            if code != 200 {
                return Err(classify_connect_code(code));
            }
            let req = origin_request(target);
            write_counted(&mut stream, req.as_bytes(), deadline, up).await?;
            let (status, head, rest) = read_head(&mut stream, rest, deadline, down).await?;
            let body = read_body(&mut stream, &head, rest, deadline, down).await?;
            finish(status, &body)
        }
    }
}

/// CONNECT 的状态码来自代理本身，含义和"目标返回的状态码"完全不同。
fn classify_connect_code(code: u16) -> ProbeFailure {
    match code {
        401 | 407 => ProbeFailure::policy(format!("上游要求代理认证（{code}），账号或白名单不对")),
        429 => ProbeFailure::policy("上游限流（429），先降低探测频率/并发"),
        // 网关拒绝这种 CONNECT（端口/方法不被允许），换回绝对 URI 也许能通。
        403 | 405 | 501 => ProbeFailure::mismatch(format!("上游拒绝该 CONNECT（{code}）")),
        // 5xx：代理活着，但它到目标不通，通常就是这个出口坏了。
        _ => ProbeFailure::transport(format!("上游 CONNECT 失败（{code}）")),
    }
}

async fn probe_socks5(
    node: &ProxyNode,
    target: &Target,
    budget: Duration,
) -> (Result<String, ProbeFailure>, u64, u64) {
    let deadline = Instant::now() + budget;
    let mut up = 0u64;
    let mut down = 0u64;
    let addr = host_port(&node.host, node.port as u16);
    let mut stream = match connect_upstream(&addr, deadline).await {
        Ok(s) => s,
        Err(e) => return (Err(e), 0, 0),
    };
    if let Err(e) = socks5_open(&mut stream, node, target, deadline, &mut up, &mut down).await {
        return (Err(e), up, down);
    }
    let req = origin_request(target);
    if let Err(e) = write_counted(&mut stream, req.as_bytes(), deadline, &mut up).await {
        return (Err(e), up, down);
    }
    let result = async {
        let (status, head, rest) = read_head(&mut stream, Vec::new(), deadline, &mut down).await?;
        let body = read_body(&mut stream, &head, rest, deadline, &mut down).await?;
        finish(status, &body)
    }
    .await;
    (result, up, down)
}

async fn socks5_open(
    stream: &mut TcpStream,
    node: &ProxyNode,
    target: &Target,
    deadline: Instant,
    up: &mut u64,
    down: &mut u64,
) -> Result<(), ProbeFailure> {
    let has_auth = node
        .username
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let greeting: &[u8] = if has_auth {
        &[0x05, 0x02, 0x00, 0x02]
    } else {
        &[0x05, 0x01, 0x00]
    };
    write_counted(stream, greeting, deadline, up).await?;
    let mut resp = [0u8; 2];
    read_exact_counted(stream, &mut resp, deadline, down)
        .await
        .map_err(|_| ProbeFailure::transport("上游 SOCKS5 握手中断"))?;
    if resp[0] != 0x05 {
        return Err(ProbeFailure::policy("上游不是 SOCKS5（版本号不对）"));
    }
    if resp[1] == 0x02 {
        let user = node.username.clone().unwrap_or_default();
        let pass = node.password.clone().unwrap_or_default();
        let (ub, pb) = (user.as_bytes(), pass.as_bytes());
        if ub.len() > 255 || pb.len() > 255 {
            return Err(ProbeFailure::policy("上游认证信息过长"));
        }
        let mut pkt = vec![0x01, ub.len() as u8];
        pkt.extend_from_slice(ub);
        pkt.push(pb.len() as u8);
        pkt.extend_from_slice(pb);
        write_counted(stream, &pkt, deadline, up).await?;
        let mut ar = [0u8; 2];
        read_exact_counted(stream, &mut ar, deadline, down).await?;
        if ar[1] != 0x00 {
            return Err(ProbeFailure::policy(
                "上游 SOCKS5 认证失败（账号/白名单不对）",
            ));
        }
    } else if resp[1] != 0x00 {
        return Err(ProbeFailure::policy("上游 SOCKS5 不接受我们的认证方式"));
    }

    let mut req = vec![0x05, 0x01, 0x00];
    match target.host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            req.push(0x01);
            req.extend_from_slice(&v4.octets());
        }
        Ok(IpAddr::V6(v6)) => {
            req.push(0x04);
            req.extend_from_slice(&v6.octets());
        }
        Err(_) => {
            let hb = target.host.as_bytes();
            if hb.len() > 255 {
                return Err(ProbeFailure::target("检测目标域名过长"));
            }
            req.push(0x03);
            req.push(hb.len() as u8);
            req.extend_from_slice(hb);
        }
    }
    req.extend_from_slice(&target.port.to_be_bytes());
    write_counted(stream, &req, deadline, up).await?;

    let mut hdr = [0u8; 4];
    read_exact_counted(stream, &mut hdr, deadline, down)
        .await
        .map_err(|_| ProbeFailure::transport("上游 SOCKS5 CONNECT 无响应"))?;
    if hdr[1] != 0x00 {
        // 认证已通过，说明代理是活的；CONNECT 失败通常是这个出口连不到目标。
        return Err(ProbeFailure::transport(format!(
            "上游 SOCKS5 CONNECT 失败, code={}",
            hdr[1]
        )));
    }
    match hdr[3] {
        0x01 => {
            let mut b = [0u8; 6];
            read_exact_counted(stream, &mut b, deadline, down).await?;
        }
        0x03 => {
            let mut l = [0u8; 1];
            read_exact_counted(stream, &mut l, deadline, down).await?;
            let mut rest = vec![0u8; l[0] as usize + 2];
            read_exact_counted(stream, &mut rest, deadline, down).await?;
        }
        0x04 => {
            let mut b = [0u8; 18];
            read_exact_counted(stream, &mut b, deadline, down).await?;
        }
        _ => return Err(ProbeFailure::transport("未知的 SOCKS5 地址类型")),
    }
    Ok(())
}

fn absolute_request(node: &ProxyNode, target: &Target) -> String {
    let mut req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: ProxyPool-Probe/1\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n",
        target.absolute,
        target.authority()
    );
    if let Some(token) = proxy_auth(node) {
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("Proxy-Connection: close\r\n\r\n");
    req
}

fn connect_request(node: &ProxyNode, target: &Target) -> String {
    let hp = target.host_port();
    let mut req = format!(
        "CONNECT {hp} HTTP/1.1\r\nHost: {hp}\r\nUser-Agent: ProxyPool-Probe/1\r\n"
    );
    if let Some(token) = proxy_auth(node) {
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");
    req
}

fn origin_request(target: &Target) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: ProxyPool-Probe/1\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        target.path,
        target.authority()
    )
}

fn proxy_auth(node: &ProxyNode) -> Option<String> {
    let user = node.username.as_ref().filter(|s| !s.is_empty())?;
    let pass = node.password.clone().unwrap_or_default();
    Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("{user}:{pass}"),
    ))
}

struct Head {
    content_len: Option<usize>,
    chunked: bool,
}

/// 读响应头，返回（状态码、头部摘要、已经读到的多余字节）。
fn parse_head(raw: &str) -> Option<(u16, Head)> {
    let mut lines = raw.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    let proto = parts.next()?;
    if !proto.starts_with("HTTP/") {
        return None;
    }
    let code: u16 = parts.next()?.parse().ok()?;
    let mut head = Head {
        content_len: None,
        chunked: false,
    };
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        match key.as_str() {
            "content-length" => head.content_len = val.parse::<usize>().ok(),
            "transfer-encoding" => head.chunked = val.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }
    Some((code, head))
}

async fn connect_upstream(addr: &str, deadline: Instant) -> Result<TcpStream, ProbeFailure> {
    let wait = Duration::from_millis(CONNECT_TIMEOUT_MS).min(remaining(deadline)?);
    match timeout(wait, TcpStream::connect(addr)).await {
        Err(_) => Err(ProbeFailure::transport(format!("连接上游 {addr} 超时"))),
        Ok(Err(e)) => Err(ProbeFailure::transport(format!(
            "连接上游 {addr} 失败: {e}"
        ))),
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            Ok(stream)
        }
    }
}

fn remaining(deadline: Instant) -> Result<Duration, ProbeFailure> {
    let now = Instant::now();
    if now >= deadline {
        return Err(ProbeFailure::transport("探测总时长超时"));
    }
    Ok(deadline - now)
}

async fn write_counted(
    stream: &mut TcpStream,
    data: &[u8],
    deadline: Instant,
    up: &mut u64,
) -> Result<(), ProbeFailure> {
    timeout(remaining(deadline)?, stream.write_all(data))
        .await
        .map_err(|_| ProbeFailure::transport("向上游发送请求超时"))?
        .map_err(|e| ProbeFailure::transport(format!("向上游发送请求失败: {e}")))?;
    *up += data.len() as u64;
    Ok(())
}

async fn read_exact_counted(
    stream: &mut TcpStream,
    buf: &mut [u8],
    deadline: Instant,
    down: &mut u64,
) -> Result<(), ProbeFailure> {
    timeout(remaining(deadline)?, stream.read_exact(buf))
        .await
        .map_err(|_| ProbeFailure::transport("等待上游响应超时"))?
        .map_err(|e| ProbeFailure::transport(format!("读取上游响应失败: {e}")))?;
    *down += buf.len() as u64;
    Ok(())
}

async fn read_head(
    stream: &mut TcpStream,
    prefix: Vec<u8>,
    deadline: Instant,
    down: &mut u64,
) -> Result<(u16, Head, Vec<u8>), ProbeFailure> {
    let mut buf = prefix;
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_head_end(&buf) {
            let head_text = String::from_utf8_lossy(&buf[..pos]).to_string();
            let (code, head) = parse_head(&head_text)
                .ok_or_else(|| ProbeFailure::target("上游返回的不是 HTTP 响应"))?;
            let rest = buf[pos + 4..].to_vec();
            return Ok((code, head, rest));
        }
        if buf.len() > 16 * 1024 {
            return Err(ProbeFailure::target("响应头过大，疑似错误页"));
        }
        let n = timeout(remaining(deadline)?, stream.read(&mut chunk))
            .await
            .map_err(|_| ProbeFailure::transport("等待响应超时"))?
            .map_err(|e| ProbeFailure::transport(format!("读取响应失败: {e}")))?;
        if n == 0 {
            return Err(if buf.is_empty() {
                ProbeFailure::transport("上游没返回任何数据就关闭了连接")
            } else {
                ProbeFailure::transport("上游在响应头中途关闭了连接")
            });
        }
        *down += n as u64;
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn read_body(
    stream: &mut TcpStream,
    head: &Head,
    mut buf: Vec<u8>,
    deadline: Instant,
    down: &mut u64,
) -> Result<String, ProbeFailure> {
    // 出口 IP 的响应体一定很小；按 Content-Length 读，没有就一路读到 EOF。
    let limit = if head.chunked {
        MAX_BODY_BYTES
    } else {
        head.content_len
            .unwrap_or(MAX_BODY_BYTES)
            .min(MAX_BODY_BYTES)
    };
    // 没有 Content-Length 时对方可能不关连接：给每次读一个很短的收尾窗口，
    // 读到东西后就算超时也当读完，避免把"网关不关连接"误判成节点超时。
    let dangling = head.content_len.is_none() && !head.chunked;
    let mut chunk = [0u8; 2048];
    while buf.len() < limit {
        let wait = remaining(deadline)?;
        let wait = if dangling {
            wait.min(Duration::from_millis(600))
        } else {
            wait
        };
        match timeout(wait, stream.read(&mut chunk)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                *down += n as u64;
                buf.extend_from_slice(&chunk[..n]);
            }
            Ok(Err(e)) => {
                if buf.is_empty() {
                    return Err(ProbeFailure::transport(format!("读取响应体失败: {e}")));
                }
                break;
            }
            Err(_) => {
                if buf.is_empty() {
                    return Err(ProbeFailure::transport("读取响应体超时"));
                }
                break;
            }
        }
    }
    buf.truncate(limit);
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// 目标侧的最终判定：状态码 2xx 且能解析出 IP 才算通过。
fn finish(status: u16, body: &str) -> Result<String, ProbeFailure> {
    if !(200..300).contains(&status) {
        return Err(ProbeFailure::target(format!("检测目标返回 {status}")));
    }
    extract_ip(body).ok_or_else(|| {
        ProbeFailure::target("响应体里没有可识别的出口 IP（可能是网关/目标返回的错误页）")
    })
}

// ---------------------------------------------------------------------------
// HTTPS 目标：保留原来的 reqwest 通道（rustls 握手交给 reqwest）
// ---------------------------------------------------------------------------

async fn probe_tls(
    node: &ProxyNode,
    policy: &DetectionPolicy,
    check_url: &str,
) -> ProbeReport {
    let relay = match CountingRelay::start(&node.host, node.port as u16).await {
        Ok(r) => r,
        Err(e) => return ProbeReport::failed(ProbeFailure::transport(e.to_string())),
    };
    let started = Instant::now();
    let result = probe_through_tls(node, policy, relay.addr, check_url).await;
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

async fn probe_through_tls(
    node: &ProxyNode,
    policy: &DetectionPolicy,
    relay_addr: SocketAddr,
    check_url: &str,
) -> Result<String, ProbeFailure> {
    let proxy_url = node_to_proxy_url(node, relay_addr)
        .map_err(|e| ProbeFailure::transport(e.to_string()))?;
    let proxy = reqwest::Proxy::all(&proxy_url).map_err(|e| ProbeFailure::transport(e.to_string()))?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_millis(policy.timeout_ms.max(1000) as u64))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| ProbeFailure::transport(e.to_string()))?;
    let resp = client
        .get(check_url)
        .send()
        .await
        .map_err(|e| classify_reqwest(&e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProbeFailure::target(format!("检测 URL 返回 {status}")));
    }
    extract_ip(&body).ok_or_else(|| ProbeFailure::target("无法从响应解析出口 IP"))
}

fn classify_reqwest(e: &reqwest::Error) -> ProbeFailure {
    if e.is_timeout() {
        ProbeFailure::transport(format!("探测超时: {e}"))
    } else if e.is_connect() || e.is_request() || e.is_body() {
        ProbeFailure::transport(format!("上游连接失败: {e}"))
    } else {
        ProbeFailure::target(format!("请求失败: {e}"))
    }
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

/// 解析出口 IP。宁缺毋滥：只接受"整串就是一个 IP"、JSON 里的 ip 字段、
/// 或者逐行中整行就是 IP 的情况。以前用正则在 HTML 错误页里随便抓一个 IPv4，
/// 会把网关错误页里的地址当成出口 IP，污染去重和地理库。
fn extract_ip(body: &str) -> Option<String> {
    let t = body.trim();
    if t.is_empty() || t.len() > MAX_BODY_BYTES {
        return None;
    }
    if let Some(ip) = parse_ip_token(t) {
        return Some(ip);
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        for k in ["ip", "origin", "query", "origin_ip", "addr", "address"] {
            if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
                for part in s.split(',') {
                    if let Some(ip) = parse_ip_token(part) {
                        return Some(ip);
                    }
                }
            }
        }
    }
    // chunked 传输时数据行就是纯 IP；也兼容末尾带空行的纯文本
    for line in t.lines().take(64) {
        if let Some(ip) = parse_ip_token(line) {
            return Some(ip);
        }
    }
    None
}

fn parse_ip_token(s: &str) -> Option<String> {
    let t = s
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '[' || c == ']' || c == ',' || c == '{' || c == '}');
    if t.is_empty() || t.len() > 64 {
        return None;
    }
    t.parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn host_port(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn now_plus(secs: i64) -> String {
    (Utc::now() + ChronoDuration::seconds(secs.max(0))).to_rfc3339()
}

/// 失败后的重试间隔：`backoff` 策略指数增长，其余策略按次数线性增长，
/// 再叠 ±25% 抖动。以前是不看任何配置的固定 30 秒，三次失败 60 秒就隔离。
fn retry_delay_secs(policy: &DetectionPolicy, fails: i64) -> i64 {
    let base = policy.backoff_base_seconds.max(10);
    let raw = if policy.on_fail == "backoff" {
        base.saturating_mul(1i64 << (fails.clamp(1, 8) - 1))
    } else {
        base.saturating_mul(fails.clamp(1, 3))
    };
    let span = (raw / 4).max(1) as u64;
    let jittered = raw - span as i64 + (pseudo_rand() % (span * 2 + 1)) as i64;
    jittered.max(10)
}

fn pseudo_rand() -> u64 {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

pub(crate) fn jitter_ms(id: &str, span: u64) -> u64 {
    if span == 0 {
        return 0;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= pseudo_rand();
    h % span
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

/// HTTPS 探测用的本地一次性 TCP 中继：reqwest 仍然照旧"走代理"，
/// 只是把代理地址指向它，由它转发到真正的上游节点并统计字节数。
/// 这样 TLS / gzip / 重定向等行为完全不变，却能拿到真实用量。
struct CountingRelay {
    addr: SocketAddr,
    up: Arc<std::sync::atomic::AtomicU64>,
    down: Arc<std::sync::atomic::AtomicU64>,
    task: JoinHandle<()>,
}

impl CountingRelay {
    async fn start(host: &str, port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let up = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let down = Arc::new(std::sync::atomic::AtomicU64::new(0));
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
            self.up.load(std::sync::atomic::Ordering::Relaxed),
            self.down.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

impl Drop for CountingRelay {
    fn drop(&mut self) {
        // 中继任务随 JoinSet 一起结束，不会泄漏
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> ProxyNode {
        ProxyNode {
            id: "n1".into(),
            pool_id: "p1".into(),
            protocol: "http".into(),
            host: "127.0.0.1".into(),
            port: 1,
            username: Some("user".into()),
            password: Some("pass".into()),
            raw: None,
            exit_ip: None,
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

    fn policy() -> DetectionPolicy {
        DetectionPolicy {
            id: "pol".into(),
            name: "test".into(),
            check_interval: 300,
            check_url: "http://api.ipify.org/".into(),
            timeout_ms: 5000,
            on_fail: "temp_isolate".into(),
            isolate_seconds: 600,
            backoff_base_seconds: 30,
            max_fails: 3,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[derive(Clone, Copy)]
    enum Behavior {
        /// 支持绝对 URI，返回纯 IP
        AbsoluteOk,
        /// 只支持 CONNECT，绝对 URI 回 405
        ConnectOnly,
        /// 要求代理认证
        AuthRequired,
        /// 返回一个 HTML 错误页（里面还有别的 IP）
        HtmlErrorPage,
        /// 200 但没有 Content-Length，而且不关连接
        NoLengthHang,
    }

    /// 一个假的上游 HTTP 代理，行为可控，用来验证探测的请求格式与分类。
    async fn spawn_proxy(behavior: Behavior) -> SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = match sock.read(&mut buf).await {
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Behavior::NoLengthHang = behavior {
                        // 没有 Content-Length，而且写完不关连接
                        sock.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n203.0.113.7")
                            .await
                            .ok();
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        return;
                    }
                    if head.starts_with("CONNECT ") {
                        if let Behavior::AuthRequired = behavior {
                            sock.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n")
                                .await
                                .ok();
                            return;
                        }
                        sock.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                            .await
                            .ok();
                        // 隧道里再读一次 GET
                        let mut more = vec![0u8; 8192];
                        let _ = sock.read(&mut more).await;
                        let resp = reply(behavior);
                        sock.write_all(resp.as_bytes()).await.ok();
                        return;
                    }
                    let resp = match behavior {
                        Behavior::AbsoluteOk
                        | Behavior::HtmlErrorPage
                        | Behavior::NoLengthHang => reply(behavior),
                        Behavior::ConnectOnly => {
                            "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n".to_string()
                        }
                        Behavior::AuthRequired => {
                            "HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n"
                                .to_string()
                        }
                    };
                    sock.write_all(resp.as_bytes()).await.ok();
                });
            }
        });
        addr
    }

    fn reply(behavior: Behavior) -> String {
        let body = match behavior {
            Behavior::HtmlErrorPage => {
                "<html><body>blocked by gateway 10.11.12.13</body></html>".to_string()
            }
            _ => "203.0.113.7".to_string(),
        };
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    async fn probe_against(behavior: Behavior) -> ProbeReport {
        let addr = spawn_proxy(behavior).await;
        let mut n = node();
        n.port = addr.port() as i64;
        probe(&n, &policy(), "http://api.ipify.org/").await
    }

    #[tokio::test]
    async fn absolute_uri_returns_exit_ip_and_counts_bytes() {
        let r = probe_against(Behavior::AbsoluteOk).await;
        assert_eq!(r.result.expect("should succeed"), "203.0.113.7");
        assert!(r.up > 0, "upstream bytes must be accounted");
        assert!(r.down > 0, "downstream bytes must be accounted");
    }

    #[tokio::test]
    async fn connect_only_gateway_falls_back_to_tunnel() {
        let r = probe_against(Behavior::ConnectOnly).await;
        assert_eq!(r.result.expect("CONNECT fallback should succeed"), "203.0.113.7");
    }

    #[tokio::test]
    async fn auth_required_is_policy_not_node_fault() {
        let r = probe_against(Behavior::AuthRequired).await;
        let fail = r.result.expect_err("407 must fail");
        assert_eq!(fail.kind, FailKind::Policy);
        assert!(!fail.counts_against_node(), "auth errors must not isolate the node");
    }

    #[tokio::test]
    async fn html_error_page_is_not_an_exit_ip() {
        let r = probe_against(Behavior::HtmlErrorPage).await;
        let fail = r.result.expect_err("an HTML page must not parse as an exit IP");
        assert_eq!(fail.kind, FailKind::Target);
        assert!(!fail.counts_against_node());
    }

    #[tokio::test]
    async fn missing_content_length_does_not_look_like_a_timeout() {
        let r = probe_against(Behavior::NoLengthHang).await;
        assert_eq!(
            r.result.expect("body without Content-Length must still parse"),
            "203.0.113.7"
        );
    }

    #[tokio::test]
    async fn dead_port_is_a_node_fault() {
        let mut n = node();
        // 绑定后立刻释放，保证端口没人监听
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        n.port = l.local_addr().unwrap().port() as i64;
        drop(l);
        let r = probe(&n, &policy(), "http://api.ipify.org/").await;
        let fail = r.result.expect_err("closed port must fail");
        assert_eq!(fail.kind, FailKind::Transport);
        assert!(fail.counts_against_node());
    }
}


