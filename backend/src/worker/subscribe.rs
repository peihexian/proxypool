use crate::db::now_rfc3339;
use crate::models::{NodePool, ParsedProxy};
use crate::parser::{decode_charset, parse_proxies};
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
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
    let n = upsert_nodes(state, pool, &parsed, true).await?;
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
