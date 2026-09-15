use crate::models::ProxyNode;
use anyhow::{bail, Context, Result};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn connect_via(
    node: &ProxyNode,
    dest_host: &str,
    dest_port: u16,
    remote_dns: bool,
) -> Result<TcpStream> {
    match tokio::time::timeout(
        Duration::from_secs(30),
        connect_via_inner(node, dest_host, dest_port, remote_dns),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => bail!("连接上游代理超时"),
    }
}

/// Reach `dest` only by asking the selected upstream proxy to CONNECT.
/// This process never dials the destination itself, so a v4-only proxy
/// failing to egress IPv6 must surface as an error — never fall back to
/// the host's own IPv6.
async fn connect_via_inner(
    node: &ProxyNode,
    dest_host: &str,
    dest_port: u16,
    remote_dns: bool,
) -> Result<TcpStream> {
    let addr = host_port(&node.host, node.port as u16);
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("连接上游 {addr} 失败"))?;
    stream.set_nodelay(true).ok();

    match node.protocol.as_str() {
        "socks5" | "socks5h" => {
            let use_domain = remote_dns
                || node.protocol == "socks5h"
                || dest_host.parse::<std::net::IpAddr>().is_err();
            socks5_handshake(&mut stream, node, dest_host, dest_port, use_domain).await?;
        }
        _ => {
            http_connect(&mut stream, node, dest_host, dest_port).await?;
        }
    }
    Ok(stream)
}

async fn socks5_handshake(
    stream: &mut TcpStream,
    node: &ProxyNode,
    dest_host: &str,
    dest_port: u16,
    use_domain: bool,
) -> Result<()> {
    let has_auth = node
        .username
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_auth {
        stream.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
    } else {
        stream.write_all(&[0x05, 0x01, 0x00]).await?;
    }
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;
    if resp[0] != 0x05 {
        bail!("上游不是 SOCKS5");
    }
    if resp[1] == 0x02 {
        let user = node.username.clone().unwrap_or_default();
        let pass = node.password.clone().unwrap_or_default();
        let ub = user.as_bytes();
        let pb = pass.as_bytes();
        if ub.len() > 255 || pb.len() > 255 {
            bail!("上游认证信息过长");
        }
        let mut pkt = Vec::with_capacity(3 + ub.len() + pb.len());
        pkt.push(0x01);
        pkt.push(ub.len() as u8);
        pkt.extend_from_slice(ub);
        pkt.push(pb.len() as u8);
        pkt.extend_from_slice(pb);
        stream.write_all(&pkt).await?;
        let mut ar = [0u8; 2];
        stream.read_exact(&mut ar).await?;
        if ar[1] != 0x00 {
            bail!("上游 SOCKS5 认证失败");
        }
    } else if resp[1] != 0x00 {
        bail!("上游 SOCKS5 不支持所需认证方式");
    }

    let mut req = vec![0x05, 0x01, 0x00];
    if use_domain {
        let hb = dest_host.as_bytes();
        if hb.len() > 255 {
            bail!("目标域名过长");
        }
        req.push(0x03);
        req.push(hb.len() as u8);
        req.extend_from_slice(hb);
    } else if let Ok(ip) = dest_host.parse::<std::net::IpAddr>() {
        match ip {
            std::net::IpAddr::V4(v) => {
                req.push(0x01);
                req.extend_from_slice(&v.octets());
            }
            std::net::IpAddr::V6(v) => {
                req.push(0x04);
                req.extend_from_slice(&v.octets());
            }
        }
    } else {
        let hb = dest_host.as_bytes();
        req.push(0x03);
        req.push(hb.len() as u8);
        req.extend_from_slice(hb);
    }
    req.extend_from_slice(&dest_port.to_be_bytes());
    stream.write_all(&req).await?;

    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await?;
    if hdr[1] != 0x00 {
        bail!("上游 SOCKS5 CONNECT 失败, code={}", hdr[1]);
    }
    match hdr[3] {
        0x01 => {
            let mut b = [0u8; 6];
            stream.read_exact(&mut b).await?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut rest = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut rest).await?;
        }
        0x04 => {
            let mut b = [0u8; 18];
            stream.read_exact(&mut b).await?;
        }
        _ => bail!("未知 SOCKS5 地址类型"),
    }
    Ok(())
}

async fn http_connect(
    stream: &mut TcpStream,
    node: &ProxyNode,
    dest_host: &str,
    dest_port: u16,
) -> Result<()> {
    let hp = host_port(dest_host, dest_port);
    let mut req = format!("CONNECT {hp} HTTP/1.1\r\nHost: {hp}\r\n");
    if let (Some(u), Some(p)) = (&node.username, &node.password) {
        if !u.is_empty() {
            let token = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("{u}:{p}"),
            );
            req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
        }
    }
    req.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            bail!("上游 HTTP 代理关闭连接");
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            bail!("上游 HTTP 响应头过大");
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status = text.lines().next().unwrap_or("");
    if !status.contains(" 200") {
        bail!("上游 HTTP CONNECT 失败: {status}");
    }
    Ok(())
}

fn host_port(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Bidirectional copy until either side closes. No idle timeout, so long LLM
/// thinking / streaming responses are not cut off after the tunnel is up.
///
/// Byte counts are kept even if the peer resets after data has already been
/// copied — `tokio::io::copy` would drop the count on BrokenPipe.
pub async fn copy_counted(a: TcpStream, b: TcpStream) -> (u64, u64) {
    let (mut ar, mut aw) = a.into_split();
    let (mut br, mut bw) = b.into_split();
    let up = async {
        let n = copy_with_count(&mut ar, &mut bw).await;
        let _ = bw.shutdown().await;
        n
    };
    let down = async {
        let n = copy_with_count(&mut br, &mut aw).await;
        let _ = aw.shutdown().await;
        n
    };
    tokio::join!(up, down)
}

async fn copy_with_count<R, W>(reader: &mut R, writer: &mut W) -> u64
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let mut off = 0;
        while off < n {
            match writer.write(&buf[off..n]).await {
                Ok(0) => {
                    let _ = writer.flush().await;
                    return total;
                }
                Ok(w) => {
                    total += w as u64;
                    off += w;
                }
                Err(_) => {
                    let _ = writer.flush().await;
                    return total;
                }
            }
        }
    }
    let _ = writer.flush().await;
    total
}
