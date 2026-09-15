use crate::models::ServiceNode;
use crate::proxy::chain;
use crate::proxy::selector;
use crate::state::{AppState, TrafficEvent};
use anyhow::{bail, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn handle(
    state: AppState,
    svc: ServiceNode,
    mut stream: TcpStream,
    peer: std::net::SocketAddr,
) -> Result<()> {
    if svc.enable_socks5 == 0 && svc.enable_socks5h == 0 {
        stream.write_all(&[0x05, 0xFF]).await.ok();
        bail!("SOCKS5 未启用");
    }

    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;
    if hdr[0] != 0x05 {
        bail!("非 SOCKS5");
    }
    let mut methods = vec![0u8; hdr[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0x02) {
        stream.write_all(&[0x05, 0xFF]).await?;
        bail!("客户端未提供用户名密码认证");
    }
    stream.write_all(&[0x05, 0x02]).await?;

    let mut ver = [0u8; 2];
    stream.read_exact(&mut ver).await?;
    if ver[0] != 0x01 {
        bail!("无效认证版本");
    }
    let ulen = ver[1] as usize;
    let mut user = vec![0u8; ulen];
    stream.read_exact(&mut user).await?;
    let mut plen = [0u8; 1];
    stream.read_exact(&mut plen).await?;
    let mut pass = vec![0u8; plen[0] as usize];
    stream.read_exact(&mut pass).await?;
    let username = String::from_utf8_lossy(&user);
    let password = String::from_utf8_lossy(&pass);
    if username != svc.username || password != svc.password {
        stream.write_all(&[0x01, 0x01]).await?;
        bail!("SOCKS5 认证失败");
    }
    stream.write_all(&[0x01, 0x00]).await?;

    let mut req = [0u8; 4];
    stream.read_exact(&mut req).await?;
    if req[0] != 0x05 || req[1] != 0x01 {
        reply_fail(&mut stream, 0x07).await?;
        bail!("仅支持 CONNECT");
    }
    let (host, port, is_domain) = read_addr(&mut stream, req[3]).await?;

    if is_domain && svc.enable_socks5h == 0 && svc.enable_socks5 == 1 {
        // socks5: resolve locally then connect via node as IP if possible
    }
    if is_domain && svc.enable_socks5h == 0 && svc.enable_socks5 == 0 {
        reply_fail(&mut stream, 0x02).await?;
        bail!("socks5h 未启用");
    }

    let client_ip = peer.ip().to_string();
    let node = match selector::pick_node(&state, &svc, &client_ip).await {
        Ok(n) => n,
        Err(_) => {
            reply_fail(&mut stream, 0x04).await?;
            bail!("无可用节点");
        }
    };

    let remote_dns = is_domain && svc.enable_socks5h != 0;
    let upstream = match chain::connect_via(&node, &host, port, remote_dns).await {
        Ok(s) => s,
        Err(e) => {
            reply_fail(&mut stream, 0x05).await?;
            return Err(e);
        }
    };

    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    let (up, down) = chain::copy_counted(stream, upstream).await;
    let _ = state.traffic_tx.try_send(TrafficEvent {
        service_id: svc.id,
        client_ip,
        bytes_up: up,
        bytes_down: down,
    });
    Ok(())
}

async fn read_addr(stream: &mut TcpStream, atyp: u8) -> Result<(String, u16, bool)> {
    match atyp {
        0x01 => {
            let mut b = [0u8; 6];
            stream.read_exact(&mut b).await?;
            let ip = std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]);
            let port = u16::from_be_bytes([b[4], b[5]]);
            Ok((ip.to_string(), port, false))
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            stream.read_exact(&mut name).await?;
            let mut pb = [0u8; 2];
            stream.read_exact(&mut pb).await?;
            let port = u16::from_be_bytes(pb);
            Ok((String::from_utf8_lossy(&name).into_owned(), port, true))
        }
        0x04 => {
            let mut b = [0u8; 18];
            stream.read_exact(&mut b).await?;
            let mut oct = [0u8; 16];
            oct.copy_from_slice(&b[..16]);
            let ip = std::net::Ipv6Addr::from(oct);
            let port = u16::from_be_bytes([b[16], b[17]]);
            Ok((ip.to_string(), port, false))
        }
        _ => bail!("未知地址类型"),
    }
}

async fn reply_fail(stream: &mut TcpStream, code: u8) -> Result<()> {
    stream
        .write_all(&[0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    Ok(())
}
