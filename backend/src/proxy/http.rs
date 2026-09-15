use crate::models::ServiceNode;
use crate::proxy::chain;
use crate::proxy::selector;
use crate::state::{AppState, TrafficEvent};
use anyhow::{bail, Result};
use base64::Engine;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn handle(
    state: AppState,
    svc: ServiceNode,
    mut stream: TcpStream,
    peer: std::net::SocketAddr,
) -> Result<()> {
    if svc.enable_http == 0 {
        let _ = stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .await;
        bail!("HTTP 未启用");
    }

    let (head, rest) = read_headers(&mut stream).await?;
    let (method, target, headers) = parse_request(&head)?;
    if !auth_ok(&svc, &headers) {
        stream
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"myproxy\"\r\nContent-Length: 0\r\n\r\n",
            )
            .await?;
        bail!("认证失败");
    }

    let client_ip = peer.ip().to_string();
    let node = match selector::pick_node(&state, &svc, &client_ip).await {
        Ok(n) => n,
        Err(e) => {
            let msg = e.to_string();
            let body = format!("no available node: {msg}");
            let resp = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await.ok();
            return Err(e);
        }
    };

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(&target)?;
        let dest = format!("{host}:{port}");
        let proxy_ip = selector::advertise_ip(&node);
        let upstream = chain::connect_via(&node, &host, port, true).await?;
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        let (up, down) = chain::copy_counted(stream, upstream).await;
        emit_traffic(&state, &svc.id, &client_ip, &proxy_ip, &dest, "http", up, down);
        return Ok(());
    }

    let (host, port, path) = parse_absolute_url(&target, &headers)?;
    let dest = format!("{host}:{port}");
    let proxy_ip = selector::advertise_ip(&node);
    let mut upstream = chain::connect_via(&node, &host, port, true).await?;
    let mut fwd = format!("{method} {path} HTTP/1.1\r\n");
    let mut has_host = false;
    for (k, v) in &headers {
        let lk = k.to_ascii_lowercase();
        if lk == "proxy-authorization" || lk == "proxy-connection" {
            continue;
        }
        if lk == "host" {
            has_host = true;
        }
        fwd.push_str(&format!("{k}: {v}\r\n"));
    }
    if !has_host {
        fwd.push_str(&format!("Host: {host}:{port}\r\n"));
    }
    fwd.push_str("Connection: close\r\n\r\n");
    upstream.write_all(fwd.as_bytes()).await?;
    if !rest.is_empty() {
        upstream.write_all(&rest).await?;
    }
    let (up, down) = chain::copy_counted(stream, upstream).await;
    emit_traffic(&state, &svc.id, &client_ip, &proxy_ip, &dest, "http", up, down);
    Ok(())
}

fn emit_traffic(
    state: &AppState,
    service_id: &str,
    client_ip: &str,
    proxy_ip: &str,
    dest: &str,
    protocol: &str,
    up: u64,
    down: u64,
) {
    let ev = TrafficEvent {
        service_id: service_id.to_string(),
        client_ip: client_ip.to_string(),
        proxy_ip: proxy_ip.to_string(),
        dest: dest.to_string(),
        protocol: protocol.to_string(),
        bytes_up: up,
        bytes_down: down,
        ts: chrono::Utc::now().timestamp(),
    };
    state.push_usage_log(ev.clone());
    let _ = state.traffic_tx.try_send(ev);
}

async fn read_headers(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            bail!("客户端关闭");
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            let rest = buf.split_off(pos + 4);
            return Ok((buf, rest));
        }
        if buf.len() > 64 * 1024 {
            bail!("请求头过大");
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_request(head: &[u8]) -> Result<(String, String, HashMap<String, String>)> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    if method.is_empty() || target.is_empty() {
        bail!("无效 HTTP 请求");
    }
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Ok((method, target, headers))
}

fn auth_ok(svc: &ServiceNode, headers: &HashMap<String, String>) -> bool {
    let raw = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("proxy-authorization"))
        .map(|(_, v)| v.as_str())
        .or_else(|| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .map(|(_, v)| v.as_str())
        });
    let Some(raw) = raw else {
        return false;
    };
    let Some(b64) = raw.strip_prefix("Basic ").or_else(|| raw.strip_prefix("basic ")) else {
        return false;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return false;
    };
    let Ok(pair) = String::from_utf8(bytes) else {
        return false;
    };
    let Some((u, p)) = pair.split_once(':') else {
        return false;
    };
    u == svc.username && p == svc.password
}

fn split_host_port(target: &str) -> Result<(String, u16)> {
    if let Some(rest) = target.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| anyhow::anyhow!("bad ipv6"))?;
        let host = rest[..end].to_string();
        let port = rest[end + 1..]
            .strip_prefix(':')
            .and_then(|s| s.parse().ok())
            .unwrap_or(443);
        return Ok((host, port));
    }
    if let Some((h, p)) = target.rsplit_once(':') {
        if let Ok(port) = p.parse() {
            return Ok((h.to_string(), port));
        }
    }
    Ok((target.to_string(), 443))
}

fn parse_absolute_url(
    target: &str,
    headers: &HashMap<String, String>,
) -> Result<(String, u16, String)> {
    if let Ok(url) = url::Url::parse(target) {
        let host = url.host_str().unwrap_or("").to_string();
        let port = url
            .port_or_known_default()
            .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
        let path = if url.path().is_empty() {
            "/".to_string()
        } else {
            let mut p = url.path().to_string();
            if let Some(q) = url.query() {
                p.push('?');
                p.push_str(q);
            }
            p
        };
        return Ok((host, port, path));
    }
    let host_hdr = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let fallback = format!("{host_hdr}:80");
    let hostport = if host_hdr.contains(':') {
        host_hdr
    } else {
        &fallback
    };
    let (host, port) = split_host_port(hostport)?;
    Ok((host, port, target.to_string()))
}
