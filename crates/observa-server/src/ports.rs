use std::io;

use observa_shared::{ObservaError, Result};
use tokio::process::Command;

#[derive(Debug, serde::Serialize)]
pub struct PortRow {
    pub protocol: String,
    pub local_addr: String,
    pub local_port: u16,
    pub process: String,
}

pub async fn open_ports() -> Result<Vec<PortRow>> {
    match ss_ports().await {
        Ok(rows) if !rows.is_empty() => return Ok(rows),
        _ => {}
    }
    proc_net_ports().await
}

async fn ss_ports() -> Result<Vec<PortRow>> {
    let output = Command::new("ss")
        .args(["-tlnp", "-H"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await?;

    if !output.status.success() {
        return Err(ObservaError::Io(io::Error::other(format!(
            "ss exited {}",
            output.status
        ))));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let proto = parts[0].to_lowercase();
        let local = parts[3];
        let (addr, port) = parse_addr_port(local);
        let process = parts.last().map(|s| s.to_string()).unwrap_or_default();
        rows.push(PortRow {
            protocol: proto,
            local_addr: addr,
            local_port: port,
            process,
        });
    }
    Ok(rows)
}

async fn proc_net_ports() -> Result<Vec<PortRow>> {
    let mut rows = Vec::new();
    rows.extend(parse_proc_net_file("/proc/net/tcp", "tcp").await?);
    rows.extend(parse_proc_net_file("/proc/net/udp", "udp").await?);
    Ok(rows)
}

async fn parse_proc_net_file(path: &str, proto: &str) -> Result<Vec<PortRow>> {
    let content = tokio::fs::read_to_string(path).await?;
    let mut rows = Vec::new();
    for line in content.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let state_hex = parts[5];
        let state = u32::from_str_radix(state_hex, 16).unwrap_or(0);
        if proto == "tcp" && state != 10 {
            // 0x0A = TCP_LISTEN
            continue;
        }
        let local = parts[1];
        let (_, port) = parse_addr_port(local);
        rows.push(PortRow {
            protocol: proto.to_string(),
            local_addr: "0.0.0.0".to_string(),
            local_port: port,
            process: String::new(),
        });
    }
    Ok(rows)
}

fn parse_addr_port(addr: &str) -> (String, u16) {
    if let Some(idx) = addr.rfind(':') {
        let host = &addr[..idx];
        let port_hex = &addr[idx + 1..];
        let port = u16::from_str_radix(port_hex, 16).unwrap_or(0);
        return (host.to_string(), port);
    }
    (addr.to_string(), 0)
}
