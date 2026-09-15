use crate::db::now_rfc3339;
use crate::models::{DetectionPolicy, NodePool, ParsedProxy, ProxyNode};
use crate::parser::{decode_charset, parse_proxies};
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
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
    let n = replace_after_probe(state, pool, &parsed, &policy).await?;
    sqlx::query("UPDATE node_pools SET last_sync=?, updated_at=? WHERE id=?")
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(&pool.id)
        .execute(&state.db)
        .await?;
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
    upsert_nodes(state, pool, &parsed, replace).await
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
    Fail,
}

/// Keep serving current nodes while new subscription IPs are probed. Only after
/// probes finish are newcomers inserted; stale IPs are deleted last, and sticky
/// sessions bound to removed nodes are dropped so the next CONNECT rotates.
async fn replace_after_probe(
    state: &AppState,
    pool: &NodePool,
    parsed: &[ParsedProxy],
    policy: &DetectionPolicy,
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

    let probed = probe_newcomers(pool, policy, newcomers).await;
    let now = now_rfc3339();
    let isolate_until = (Utc::now()
        + chrono::Duration::seconds(policy.isolate_seconds.max(60)))
    .to_rfc3339();

    for (p, outcome) in probed {
        insert_probed_node(state, pool, &p, outcome, &now, &isolate_until).await?;
    }

    let stale: Vec<ProxyNode> = existing
        .into_iter()
        .filter(|n| {
            !keep.contains(&node_key(
                &n.protocol,
                &n.host,
                n.port,
                n.username.as_deref().unwrap_or(""),
            ))
        })
        .collect();
    let stale_ids: HashSet<String> = stale.iter().map(|n| n.id.clone()).collect();
    for n in &stale {
        sqlx::query("DELETE FROM proxy_nodes WHERE id=?")
            .bind(&n.id)
            .execute(&state.db)
            .await?;
    }
    state.drop_sticky_for_nodes(&stale_ids);
    if !stale_ids.is_empty() {
        tracing::info!(
            "pool {} dropped {} stale nodes and rebound sticky sessions",
            pool.name,
            stale_ids.len()
        );
    }
    Ok(parsed.len())
}

async fn probe_newcomers(
    pool: &NodePool,
    policy: &DetectionPolicy,
    newcomers: Vec<ParsedProxy>,
) -> Vec<(ParsedProxy, ProbeOutcome)> {
    if newcomers.is_empty() {
        return Vec::new();
    }
    let sem = Arc::new(Semaphore::new(8));
    let mut handles = Vec::new();
    for p in newcomers {
        let permit = match sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
        };
        let pol = policy.clone();
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            let cand = candidate_from_parsed(&pool, &p);
            let start = Instant::now();
            let outcome = match crate::worker::health::probe(&cand, &pol).await {
                Ok(exit_ip) => ProbeOutcome::Ok {
                    latency_ms: start.elapsed().as_millis() as i64,
                    exit_ip,
                },
                Err(e) => {
                    tracing::debug!("subscribe probe {}:{} fail: {e}", p.host, p.port);
                    ProbeOutcome::Fail
                }
            };
            (p, outcome)
        }));
    }
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        match h.await {
            Ok(v) => out.push(v),
            Err(e) => tracing::warn!("subscribe probe task: {e}"),
        }
    }
    out
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
        created_at: String::new(),
    }
}

async fn insert_probed_node(
    state: &AppState,
    pool: &NodePool,
    p: &ParsedProxy,
    outcome: ProbeOutcome,
    now: &str,
    isolate_until: &str,
) -> anyhow::Result<()> {
    let username = p.username.clone().unwrap_or_default();
    let id = uuid::Uuid::new_v4().to_string();
    let (status, fail_count, isolated_until, latency, exit_ip) = match &outcome {
        ProbeOutcome::Ok { latency_ms, exit_ip } => (
            "active",
            0i64,
            None,
            Some(*latency_ms),
            Some(exit_ip.as_str()),
        ),
        ProbeOutcome::Fail => ("isolated", 1i64, Some(isolate_until), None, None),
    };
    let geo = match &outcome {
        ProbeOutcome::Ok { exit_ip, .. } => state.geoip.lookup_str(exit_ip).unwrap_or_default(),
        ProbeOutcome::Fail => state.geoip.lookup_str(&p.host).unwrap_or_default(),
    };
    sqlx::query(
        r#"INSERT INTO proxy_nodes
        (id,pool_id,protocol,host,port,username,password,raw,exit_ip,country,country_code,asn,asn_org,is_residential,latency_ms,fail_count,status,isolated_until,last_check,created_at)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
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
    .bind(exit_ip)
    .bind(&geo.country)
    .bind(&geo.country_code)
    .bind(geo.asn)
    .bind(&geo.asn_org)
    .bind(if geo.is_residential { 1 } else { 0 })
    .bind(latency)
    .bind(fail_count)
    .bind(status)
    .bind(isolated_until)
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await?;
    Ok(())
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
