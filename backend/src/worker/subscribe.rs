use crate::db::now_rfc3339;
use crate::models::{DetectionPolicy, NodePool, ParsedProxy, ProxyNode};
use crate::parser::{decode_charset, parse_proxies};
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{interval, Duration};

pub async fn run(state: AppState) {
    let mut tick = interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        if let Err(e) = tick_once(&state).await {
            tracing::warn!("subscribe tick: {e}");
        }
    }
}

pub async fn sync_pool(state: &AppState, pool: &NodePool) -> anyhow::Result<usize> {
    if pool.source_type != "remote" {
        anyhow::bail!("仅远程订阅池可同步");
    }
    let url = pool
        .subscription_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("未配置订阅地址"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let resp = client.get(url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    let text = decode_charset(&bytes, &pool.charset);
    let cfg: Option<Value> = pool
        .format_config
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok());
    let parsed = parse_proxies(
        &text,
        &pool.data_format,
        &pool.encoding_format,
        cfg.as_ref(),
        &pool.default_protocol,
    );
    if parsed.is_empty() {
        anyhow::bail!("订阅内容为空或无法解析，已保留现有节点");
    }
    let policy = load_policy(state, pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("远程订阅更新需要检测策略，请先配置"))?;
    // 目标本身不可达时整体切备用目标，避免"目标挂了 → 整批新 IP 全部判死"。
    let check_url = crate::worker::health::effective_check_url(&policy).await;
    let n = replace_after_probe(state, pool, &parsed, &policy, &check_url).await?;
    sqlx::query("UPDATE node_pools SET last_sync=?, updated_at=? WHERE id=?")
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(&pool.id)
        .execute(&state.db)
        .await?;
    // 订阅里的 IP 已经变了，账号的固定出口也要立刻跟上，不用等巡检。
    if let Err(e) = crate::accounts::reconcile_all(state).await {
        tracing::warn!("reconcile accounts after pool sync: {e:#}");
    }
    Ok(n)
}

pub async fn import_local(
    state: &AppState,
    pool: &NodePool,
    text: &str,
    replace: bool,
) -> anyhow::Result<usize> {
    let cfg: Option<Value> = pool
        .format_config
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok());
    let parsed = parse_proxies(
        text,
        &pool.data_format,
        &pool.encoding_format,
        cfg.as_ref(),
        &pool.default_protocol,
    );
    let n = upsert_nodes(state, pool, &parsed, replace).await?;
    if let Err(e) = crate::accounts::reconcile_all(state).await {
        tracing::warn!("reconcile accounts after local import: {e:#}");
    }
    Ok(n)
}

async fn tick_once(state: &AppState) -> anyhow::Result<()> {
    let pools: Vec<NodePool> = sqlx::query_as(
        "SELECT * FROM node_pools WHERE enabled=1 AND source_type='remote'",
    )
    .fetch_all(&state.db)
    .await?;
    let now = Utc::now();
    for pool in pools {
        let due = match &pool.last_sync {
            None => true,
            Some(s) => DateTime::parse_from_rfc3339(s)
                .map(|t| now.signed_duration_since(t.with_timezone(&Utc)).num_seconds() >= pool.update_interval)
                .unwrap_or(true),
        };
        if due {
            match sync_pool(state, &pool).await {
                Ok(n) => tracing::info!("pool {} synced {n} nodes", pool.name),
                Err(e) => tracing::warn!("pool {} sync failed: {e}", pool.name),
            }
        }
    }
    Ok(())
}

fn node_key(protocol: &str, host: &str, port: i64, username: &str) -> String {
    format!("{protocol}|{host}|{port}|{username}")
}

fn parsed_key(p: &ParsedProxy) -> String {
    node_key(
        &p.protocol,
        &p.host,
        p.port as i64,
        p.username.as_deref().unwrap_or(""),
    )
}

async fn load_policy(state: &AppState, pool: &NodePool) -> anyhow::Result<Option<DetectionPolicy>> {
    if let Some(id) = &pool.detection_policy_id {
        Ok(sqlx::query_as("SELECT * FROM detection_policies WHERE id=?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?)
    } else {
        Ok(sqlx::query_as("SELECT * FROM detection_policies LIMIT 1")
            .fetch_optional(&state.db)
            .await?)
    }
}

enum ProbeOutcome {
    Ok { latency_ms: i64, exit_ip: String },
    Failed(crate::worker::health::ProbeFailure),
}

/// 订阅刷新时的探测并发。批量涌入几百个 IP 时，8 并发足以把住宅网关打到限流，
/// 进而让整批新 IP 一起超时、一起被判死。
const SYNC_CONCURRENCY: usize = 3;

/// Overlapping-generation refresh:
/// - Old nodes stay selectable for the whole round (the live pool is not emptied).
/// - Each new IP is inserted as soon as its probe finishes, so traffic can move
///   onto working newcomers without waiting for the slowest timeout.
/// - IPs missing from the new list are deleted only after the round, and only
///   if the pool would still have at least one active node.
async fn replace_after_probe(
    state: &AppState,
    pool: &NodePool,
    parsed: &[ParsedProxy],
    policy: &DetectionPolicy,
    check_url: &str,
) -> anyhow::Result<usize> {
    let existing: Vec<ProxyNode> =
        sqlx::query_as("SELECT * FROM proxy_nodes WHERE pool_id=?")
            .bind(&pool.id)
            .fetch_all(&state.db)
            .await?;
    let mut existing_by_key = std::collections::HashMap::new();
    for n in &existing {
        existing_by_key.insert(
            node_key(
                &n.protocol,
                &n.host,
                n.port,
                n.username.as_deref().unwrap_or(""),
            ),
            n.clone(),
        );
    }

    let keep: HashSet<String> = parsed.iter().map(parsed_key).collect();
    let mut newcomers = Vec::new();
    for p in parsed {
        if existing_by_key.contains_key(&parsed_key(p)) {
            sqlx::query("UPDATE proxy_nodes SET password=?, raw=? WHERE id=?")
                .bind(&p.password)
                .bind(&p.raw)
                .bind(&existing_by_key[&parsed_key(p)].id)
                .execute(&state.db)
                .await?;
        } else {
            newcomers.push(p.clone());
        }
    }

    let stale: Vec<ProxyNode> = existing
        .iter()
        .filter(|n| {
            !keep.contains(&node_key(
                &n.protocol,
                &n.host,
                n.port,
                n.username.as_deref().unwrap_or(""),
            ))
        })
        .cloned()
        .collect();
    let keep_active = existing
        .iter()
        .filter(|n| {
            n.status == "active"
                && keep.contains(&node_key(
                    &n.protocol,
                    &n.host,
                    n.port,
                    n.username.as_deref().unwrap_or(""),
                ))
        })
        .count();
    let stale_ids: HashSet<String> = stale.iter().map(|n| n.id.clone()).collect();
    let overlap = parsed.len().saturating_sub(newcomers.len());
    tracing::info!(
        "pool {} refresh: overlap {}, probe {}, retire {}",
        pool.name,
        overlap,
        newcomers.len(),
        stale.len()
    );

    let _sync_guard = state.enter_pool_sync(&pool.id);
    let admitted_ok =
        admit_newcomers(state, pool, policy, newcomers, &stale_ids, check_url).await?;

    let remaining_active = keep_active + admitted_ok;
    if remaining_active == 0 && !stale.is_empty() {
        tracing::warn!(
            "pool {} new subscription has no working nodes, keeping {} previous IPs",
            pool.name,
            stale.len()
        );
        return Ok(parsed.len());
    }

    for n in &stale {
        sqlx::query("DELETE FROM proxy_nodes WHERE id=?")
            .bind(&n.id)
            .execute(&state.db)
            .await?;
    }
    state.drop_sticky_for_nodes(&stale_ids);
    if !stale_ids.is_empty() {
        tracing::info!(
            "pool {} dropped {} stale nodes and rebound sticky sessions (admitted {admitted_ok} new active)",
            pool.name,
            stale_ids.len()
        );
    }
    Ok(parsed.len())
}

async fn admit_newcomers(
    state: &AppState,
    pool: &NodePool,
    policy: &DetectionPolicy,
    newcomers: Vec<ParsedProxy>,
    stale_ids: &HashSet<String>,
    check_url: &str,
) -> anyhow::Result<usize> {
    if newcomers.is_empty() {
        return Ok(0);
    }
    let sem = Arc::new(Semaphore::new(SYNC_CONCURRENCY));
    let mut set = JoinSet::new();
    for p in newcomers {
        let permit_sem = sem.clone();
        let pol = policy.clone();
        let pool = pool.clone();
        let st = state.clone();
        let url = check_url.to_string();
        set.spawn(async move {
            let _permit = match permit_sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    return (
                        p,
                        ProbeOutcome::Failed(crate::worker::health::ProbeFailure::transport(
                            "探测任务被取消",
                        )),
                    )
                }
            };
            // 和巡检一样打散：几百个新 IP 同时开测会把上游网关打到限流。
            let id = format!("{}:{}", p.host, p.port);
            let jitter = crate::worker::health::jitter_ms(&id, 1500);
            if jitter > 0 {
                tokio::time::sleep(Duration::from_millis(jitter)).await;
            }
            let cand = candidate_from_parsed(&pool, &p);
            let report = crate::worker::health::probe(&cand, &pol, &url).await;
            // 订阅刷新时的探测同样走上游，一样要入账
            crate::worker::health::record_probe(&st, &cand, &url, report.up, report.down);
            let outcome = match report.result {
                Ok(exit_ip) => ProbeOutcome::Ok {
                    latency_ms: report.latency_ms,
                    exit_ip,
                },
                Err(fail) => {
                    tracing::debug!("subscribe probe {}:{} fail: {}", p.host, p.port, fail.msg);
                    ProbeOutcome::Failed(fail)
                }
            };
            (p, outcome)
        });
    }

    let batch_now = now_rfc3339();
    let mut admitted_ok = 0usize;
    let mut attempts = 0usize;
    let mut node_faults = 0usize;
    let mut soft_faults = 0usize;
    let mut short_excluded: Vec<String> = Vec::new();

    while let Some(joined) = set.join_next().await {
        let (p, outcome) = match joined {
            Ok(item) => item,
            Err(e) => {
                tracing::warn!("subscribe probe task: {e}");
                continue;
            }
        };
        attempts += 1;
        // 只有"上游不可达"这类失败才把节点暂时挡在轮换外；
        // 账号/目标问题不代表节点坏了，先按待检测入库。
        let exclude = match &outcome {
            ProbeOutcome::Ok { .. } => false,
            ProbeOutcome::Failed(fail) => {
                if fail.counts_against_node() {
                    node_faults += 1;
                    true
                } else {
                    soft_faults += 1;
                    false
                }
            }
        };
        match insert_probed_node(state, pool, policy, &p, &outcome, &batch_now, exclude).await {
            Ok(id) => {
                if matches!(outcome, ProbeOutcome::Ok { .. }) {
                    admitted_ok += 1;
                    // Stop pinning clients to IPs that are leaving the
                    // subscription once a verified replacement exists.
                    if admitted_ok == 1 {
                        state.drop_sticky_for_nodes(stale_ids);
                    }
                } else if exclude {
                    short_excluded.push(id);
                }
            }
            Err(e) => tracing::warn!("insert probed {}:{}: {e}", p.host, p.port),
        }
    }

    // 与巡检同一套熔断：一整批都失败说明是上游/目标的系统性问题，
    // 不要把整份新订阅都挡在门外。原来这里是一次探测不过就直接写 isolated。
    let systemic = attempts >= 4 && node_faults * 2 >= attempts;
    if systemic {
        for id in &short_excluded {
            let _ = sqlx::query("UPDATE proxy_nodes SET isolated_until=NULL WHERE id=?")
                .bind(id)
                .execute(&state.db)
                .await;
        }
        tracing::warn!(
            "订阅探测 {attempts} 个新节点，上游失败 {node_faults} 个（策略/目标问题 {soft_faults} 个），判定为系统性问题：不隔离，稍后自动重测"
        );
    } else if attempts > 0 {
        tracing::info!(
            "订阅探测完成：通过 {admitted_ok} / 尝试 {attempts}（上游失败 {node_faults}，策略或目标 {soft_faults}）"
        );
    }
    Ok(admitted_ok)
}

fn candidate_from_parsed(pool: &NodePool, p: &ParsedProxy) -> ProxyNode {
    ProxyNode {
        id: String::new(),
        pool_id: pool.id.clone(),
        protocol: p.protocol.clone(),
        host: p.host.clone(),
        port: p.port as i64,
        username: p.username.clone(),
        password: p.password.clone(),
        raw: Some(p.raw.clone()),
        exit_ip: None,
        country: None,
        country_code: None,
        asn: None,
        asn_org: None,
        is_residential: 0,
        latency_ms: None,
        fail_count: 0,
        status: "pending".into(),
        isolated_until: None,
        last_check: None,
        last_used: None,
        next_check: None,
        last_error: None,
        manual_isolated: 0,
        created_at: String::new(),
    }
}

async fn insert_probed_node(
    state: &AppState,
    pool: &NodePool,
    policy: &DetectionPolicy,
    p: &ParsedProxy,
    outcome: &ProbeOutcome,
    now: &str,
    exclude: bool,
) -> anyhow::Result<String> {
    let username = p.username.clone().unwrap_or_default();
    let id = uuid::Uuid::new_v4().to_string();
    let retry_at = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
    let (status, fail_count, isolated_until, latency, exit_ip, last_error) = match outcome {
        ProbeOutcome::Ok { latency_ms, exit_ip } => (
            "active",
            0i64,
            None,
            Some(*latency_ms),
            Some(exit_ip.clone()),
            None,
        ),
        ProbeOutcome::Failed(fail) => {
            // 只是"暂时别把它放进轮换"，最多 2 分钟；30 秒后就会重测。
            let until = if exclude {
                Some(
                    (Utc::now()
                        + chrono::Duration::seconds(policy.isolate_seconds.clamp(60, 120)))
                    .to_rfc3339(),
                )
            } else {
                None
            };
            (
                "pending",
                if exclude { 1 } else { 0 },
                until,
                None,
                None,
                Some(fail.msg.clone()),
            )
        }
    };
    let geo = match outcome {
        ProbeOutcome::Ok { exit_ip, .. } => state.geoip.lookup_str(exit_ip).unwrap_or_default(),
        ProbeOutcome::Failed(_) => state.geoip.lookup_str(&p.host).unwrap_or_default(),
    };
    sqlx::query(
        r#"INSERT INTO proxy_nodes
        (id,pool_id,protocol,host,port,username,password,raw,exit_ip,country,country_code,asn,asn_org,is_residential,latency_ms,fail_count,status,isolated_until,last_check,next_check,last_error,created_at)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
    )
    .bind(&id)
    .bind(&pool.id)
    .bind(&p.protocol)
    .bind(&p.host)
    .bind(p.port as i64)
    .bind(if username.is_empty() {
        None
    } else {
        Some(username.as_str())
    })
    .bind(&p.password)
    .bind(&p.raw)
    .bind(exit_ip.as_deref())
    .bind(&geo.country)
    .bind(&geo.country_code)
    .bind(geo.asn)
    .bind(&geo.asn_org)
    .bind(if geo.is_residential { 1 } else { 0 })
    .bind(latency)
    .bind(fail_count)
    .bind(status)
    .bind(&isolated_until)
    .bind(now)
    .bind(if matches!(outcome, ProbeOutcome::Ok { .. }) {
        None
    } else {
        Some(retry_at.as_str())
    })
    .bind(last_error.as_deref())
    .bind(now)
    .execute(&state.db)
    .await?;
    Ok(id)
}

async fn upsert_nodes(
    state: &AppState,
    pool: &NodePool,
    parsed: &[ParsedProxy],
    replace: bool,
) -> anyhow::Result<usize> {
    let mut keep: HashSet<String> = HashSet::new();
    let now = now_rfc3339();
    for p in parsed {
        let username = p.username.clone().unwrap_or_default();
        let key = format!("{}|{}|{}|{}", p.protocol, p.host, p.port, username);
        keep.insert(key);
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM proxy_nodes WHERE pool_id=? AND protocol=? AND host=? AND port=? AND IFNULL(username,'')=?",
        )
        .bind(&pool.id)
        .bind(&p.protocol)
        .bind(&p.host)
        .bind(p.port as i64)
        .bind(&username)
        .fetch_optional(&state.db)
        .await?;

        let geo = state.geoip.lookup_str(&p.host);
        if let Some(id) = existing {
            sqlx::query(
                "UPDATE proxy_nodes SET password=?, raw=? WHERE id=?",
            )
            .bind(&p.password)
            .bind(&p.raw)
            .bind(&id.0)
            .execute(&state.db)
            .await?;
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                r#"INSERT INTO proxy_nodes
                (id,pool_id,protocol,host,port,username,password,raw,country,country_code,asn,asn_org,is_residential,status,created_at)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,'pending',?)"#,
            )
            .bind(&id)
            .bind(&pool.id)
            .bind(&p.protocol)
            .bind(&p.host)
            .bind(p.port as i64)
            .bind(if username.is_empty() { None } else { Some(username.as_str()) })
            .bind(&p.password)
            .bind(&p.raw)
            .bind(geo.as_ref().and_then(|g| g.country.clone()))
            .bind(geo.as_ref().and_then(|g| g.country_code.clone()))
            .bind(geo.as_ref().and_then(|g| g.asn))
            .bind(geo.as_ref().and_then(|g| g.asn_org.clone()))
            .bind(geo.as_ref().map(|g| if g.is_residential { 1 } else { 0 }).unwrap_or(0))
            .bind(&now)
            .execute(&state.db)
            .await?;
        }
    }

    if replace {
        let all: Vec<crate::models::ProxyNode> =
            sqlx::query_as("SELECT * FROM proxy_nodes WHERE pool_id=?")
                .bind(&pool.id)
                .fetch_all(&state.db)
                .await?;
        for n in all {
            let k = format!(
                "{}|{}|{}|{}",
                n.protocol,
                n.host,
                n.port,
                n.username.clone().unwrap_or_default()
            );
            if !keep.contains(&k) {
                let _ = sqlx::query("DELETE FROM proxy_nodes WHERE id=?")
                    .bind(&n.id)
                    .execute(&state.db)
                    .await;
            }
        }
    }
    Ok(parsed.len())
}
