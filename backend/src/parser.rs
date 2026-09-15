use crate::models::ParsedProxy;
use serde_json::Value;

pub fn parse_proxies(
    text: &str,
    data_format: &str,
    encoding_format: &str,
    format_config: Option<&Value>,
    default_protocol: &str,
) -> Vec<ParsedProxy> {
    match data_format {
        "json" => parse_json(text, encoding_format, format_config, default_protocol),
        _ => parse_txt(text, encoding_format, default_protocol),
    }
}

pub fn parse_txt(text: &str, encoding_format: &str, default_protocol: &str) -> Vec<ParsedProxy> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
        .filter_map(|line| parse_line(line, encoding_format, default_protocol))
        .collect()
}

pub fn parse_line(line: &str, encoding_format: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let line = line.trim().trim_matches(|c| c == '"' || c == ',' || c == ';');
    if line.is_empty() {
        return None;
    }
    match encoding_format {
        "host:port" => parse_host_port(line, default_protocol),
        "user:pass@host:port" => parse_userinfo_at(line, default_protocol),
        "host:port:user:pass" => parse_colon4(line, default_protocol),
        "scheme://user:pass@host:port" | "url" => parse_url_style(line, default_protocol),
        "host:port@user:pass" => parse_host_at_user(line, default_protocol),
        "user:pass:host:port" => parse_user_first(line, default_protocol),
        _ => parse_auto(line, default_protocol),
    }
}

fn parse_auto(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    if looks_like_url(line) {
        if let Some(p) = parse_url_style(line, default_protocol) {
            return Some(p);
        }
    }
    if let Some(at) = line.rfind('@') {
        let left = &line[..at];
        let right = &line[at + 1..];
        if host_port_ok(right) && left.contains(':') {
            return parse_userinfo_at(line, default_protocol);
        }
        if host_port_ok(left) && right.contains(':') {
            return parse_host_at_user(line, default_protocol);
        }
    }
    let parts: Vec<&str> = line.split(':').collect();
    match parts.len() {
        2 => parse_host_port(line, default_protocol),
        4 => {
            if parts[1].parse::<u16>().is_ok() {
                parse_colon4(line, default_protocol)
            } else if parts[3].parse::<u16>().is_ok() {
                parse_user_first(line, default_protocol)
            } else {
                None
            }
        }
        5 if parts[1].parse::<u16>().is_ok() => {
            let proto = normalize_protocol(parts[4], default_protocol);
            parse_colon4(&parts[..4].join(":"), &proto).map(|mut p| {
                p.protocol = proto;
                p
            })
        }
        _ => None,
    }
}

fn looks_like_url(line: &str) -> bool {
    ["socks5h://", "socks5://", "socks://", "https://", "http://"]
        .iter()
        .any(|s| line.len() > s.len() && line[..s.len()].eq_ignore_ascii_case(s))
}

fn parse_url_style(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let raw = line.to_string();
    let url = url::Url::parse(line).ok()?;
    let protocol = normalize_protocol(url.scheme(), default_protocol);
    let host = url.host_str()?.to_string();
    let port = url.port().or_else(|| match url.scheme() {
        "http" | "https" => Some(80),
        "socks5" | "socks" | "socks5h" => Some(1080),
        _ => None,
    })?;
    let username = if url.username().is_empty() {
        None
    } else {
        Some(percent_decode(url.username()))
    };
    let password = url.password().map(percent_decode);
    Some(ParsedProxy {
        protocol,
        host,
        port,
        username,
        password,
        raw,
    })
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

fn parse_host_port(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let (host, port) = split_host_port(line)?;
    Some(ParsedProxy {
        protocol: normalize_protocol("", default_protocol),
        host,
        port,
        username: None,
        password: None,
        raw: line.to_string(),
    })
}

fn parse_userinfo_at(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let at = line.rfind('@')?;
    let (userpass, hostport) = line.split_at(at);
    let hostport = &hostport[1..];
    let (username, password) = split_user_pass(userpass)?;
    let (host, port) = split_host_port(hostport)?;
    Some(ParsedProxy {
        protocol: normalize_protocol("", default_protocol),
        host,
        port,
        username: Some(username),
        password: Some(password),
        raw: line.to_string(),
    })
}

fn parse_host_at_user(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let at = line.rfind('@')?;
    let (hostport, userpass) = line.split_at(at);
    let userpass = &userpass[1..];
    let (host, port) = split_host_port(hostport)?;
    let (username, password) = split_user_pass(userpass)?;
    Some(ParsedProxy {
        protocol: normalize_protocol("", default_protocol),
        host,
        port,
        username: Some(username),
        password: Some(password),
        raw: line.to_string(),
    })
}

fn parse_colon4(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() != 4 {
        return None;
    }
    let host = parts[0].trim().to_string();
    let port: u16 = parts[1].trim().parse().ok()?;
    Some(ParsedProxy {
        protocol: normalize_protocol("", default_protocol),
        host,
        port,
        username: Some(parts[2].trim().to_string()),
        password: Some(parts[3].trim().to_string()),
        raw: line.to_string(),
    })
}

fn parse_user_first(line: &str, default_protocol: &str) -> Option<ParsedProxy> {
    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() < 4 {
        return None;
    }
    let port: u16 = parts[parts.len() - 1].trim().parse().ok()?;
    let host = parts[parts.len() - 2].trim().to_string();
    Some(ParsedProxy {
        protocol: normalize_protocol("", default_protocol),
        host,
        port,
        username: Some(parts[0].trim().to_string()),
        password: Some(parts[1..parts.len() - 2].join(":")),
        raw: line.to_string(),
    })
}

fn split_user_pass(s: &str) -> Option<(String, String)> {
    let idx = s.find(':')?;
    Some((s[..idx].to_string(), s[idx + 1..].to_string()))
}

fn split_host_port(s: &str) -> Option<(String, u16)> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = rest[..end].to_string();
        let port_part = rest[end + 1..].strip_prefix(':')?;
        let port = port_part.parse().ok()?;
        return Some((host, port));
    }
    let idx = s.rfind(':')?;
    let host = s[..idx].trim().to_string();
    let port = s[idx + 1..].trim().parse().ok()?;
    if host.is_empty() {
        return None;
    }
    Some((host, port))
}

fn host_port_ok(s: &str) -> bool {
    split_host_port(s).is_some()
}

pub fn normalize_protocol(raw: &str, default_protocol: &str) -> String {
    let v = if raw.is_empty() { default_protocol } else { raw };
    match v.to_ascii_lowercase().as_str() {
        "socks5h" | "socks5-hostname" => "socks5h".into(),
        "socks5" | "socks" => "socks5".into(),
        "https" | "http" => "http".into(),
        other if !other.is_empty() => other.into(),
        _ => "http".into(),
    }
}

pub fn parse_json(
    text: &str,
    encoding_format: &str,
    format_config: Option<&Value>,
    default_protocol: &str,
) -> Vec<ParsedProxy> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return parse_txt(trimmed, encoding_format, default_protocol);
    }
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => match parse_jsonl(trimmed) {
            Ok(v) => v,
            Err(_) => return parse_txt(trimmed, encoding_format, default_protocol),
        },
    };

    let cfg = format_config.cloned().unwrap_or(Value::Null);
    let preset = cfg
        .get("json_preset")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    let json_path = cfg.get("json_path").and_then(|v| v.as_str()).unwrap_or("");
    let host_f = cfg.get("host_field").and_then(|v| v.as_str()).unwrap_or("");
    let port_f = cfg.get("port_field").and_then(|v| v.as_str()).unwrap_or("");
    let user_f = cfg.get("user_field").and_then(|v| v.as_str()).unwrap_or("");
    let pass_f = cfg.get("pass_field").and_then(|v| v.as_str()).unwrap_or("");
    let proto_f = cfg
        .get("protocol_field")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut out = Vec::new();
    match preset {
        "clash" => collect_clash(&value, default_protocol, encoding_format, &mut out),
        "string_array" => {
            if let Some(arr) = resolve_array(&value, json_path) {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        if let Some(p) = parse_line(s, encoding_format, default_protocol) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        "ip_port" => collect_objects(
            &value,
            json_path,
            "ip",
            "port",
            "user",
            "pass",
            "protocol",
            default_protocol,
            encoding_format,
            &mut out,
        ),
        "custom" => collect_objects(
            &value,
            json_path,
            if host_f.is_empty() { "host" } else { host_f },
            if port_f.is_empty() { "port" } else { port_f },
            user_f,
            pass_f,
            proto_f,
            default_protocol,
            encoding_format,
            &mut out,
        ),
        _ => {
            collect_auto(&value, json_path, default_protocol, encoding_format, &mut out);
        }
    }
    out
}

fn parse_jsonl(text: &str) -> anyhow::Result<Value> {
    let mut arr = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        arr.push(serde_json::from_str::<Value>(line)?);
    }
    Ok(Value::Array(arr))
}

fn collect_clash(
    value: &Value,
    default_protocol: &str,
    encoding_format: &str,
    out: &mut Vec<ParsedProxy>,
) {
    if let Some(arr) = value.get("proxies").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(p) = object_to_proxy(
                item,
                "server",
                "port",
                "username",
                "password",
                "type",
                default_protocol,
                encoding_format,
            ) {
                out.push(p);
            }
        }
    }
}

fn collect_auto(
    value: &Value,
    json_path: &str,
    default_protocol: &str,
    encoding_format: &str,
    out: &mut Vec<ParsedProxy>,
) {
    if let Some(arr) = value.get("proxies").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(p) = object_to_proxy(
                item,
                "server",
                "port",
                "username",
                "password",
                "type",
                default_protocol,
                encoding_format,
            ) {
                out.push(p);
            }
        }
        if !out.is_empty() {
            return;
        }
    }
    if let Some(arr) = resolve_array(value, json_path) {
        for item in arr {
            if let Some(p) = item_to_proxy(item, default_protocol, encoding_format) {
                out.push(p);
            }
        }
    } else if let Some(p) = item_to_proxy(value, default_protocol, encoding_format) {
        out.push(p);
    }
}

fn collect_objects(
    value: &Value,
    json_path: &str,
    host_f: &str,
    port_f: &str,
    user_f: &str,
    pass_f: &str,
    proto_f: &str,
    default_protocol: &str,
    encoding_format: &str,
    out: &mut Vec<ParsedProxy>,
) {
    if let Some(arr) = resolve_array(value, json_path) {
        for item in arr {
            if let Some(p) = object_to_proxy(
                item,
                host_f,
                port_f,
                user_f,
                pass_f,
                proto_f,
                default_protocol,
                encoding_format,
            ) {
                out.push(p);
            }
        }
    }
}

fn resolve_array<'a>(value: &'a Value, path: &str) -> Option<&'a Vec<Value>> {
    if !path.is_empty() {
        let mut cur = value;
        for key in path.split('.').filter(|s| !s.is_empty()) {
            cur = cur.get(key)?;
        }
        return cur.as_array();
    }
    if let Some(arr) = value.as_array() {
        return Some(arr);
    }
    for key in ["data", "list", "proxies", "nodes", "result", "items", "rows"] {
        if let Some(arr) = value.get(key).and_then(|v| v.as_array()) {
            return Some(arr);
        }
    }
    None
}

fn item_to_proxy(item: &Value, default_protocol: &str, encoding_format: &str) -> Option<ParsedProxy> {
    if let Some(s) = item.as_str() {
        return parse_line(s, encoding_format, default_protocol);
    }
    object_to_proxy(
        item,
        "host",
        "port",
        "username",
        "password",
        "protocol",
        default_protocol,
        encoding_format,
    )
}

fn object_to_proxy(
    item: &Value,
    host_field: &str,
    port_field: &str,
    user_field: &str,
    pass_field: &str,
    protocol_field: &str,
    default_protocol: &str,
    encoding_format: &str,
) -> Option<ParsedProxy> {
    let obj = item.as_object()?;
    if let Some(raw) = first_str(obj, &["raw", "line", "proxy", "uri", "url"]) {
        if let Some(p) = parse_line(&raw, encoding_format, default_protocol) {
            return Some(p);
        }
    }
    let host = get_str(obj, host_field)
        .or_else(|| first_str(obj, &["host", "ip", "server", "addr", "address", "hostname"]))?;
    let port = get_port(obj, port_field).or_else(|| {
        ["port", "portNum", "proxy_port"]
            .iter()
            .find_map(|k| get_port(obj, k))
    })?;
    let username = get_str(obj, user_field)
        .or_else(|| first_str(obj, &["username", "user", "userName", "auth_user", "login"]));
    let password = get_str(obj, pass_field)
        .or_else(|| first_str(obj, &["password", "pass", "passwd", "auth_pass", "pwd"]));
    let protocol_raw = get_str(obj, protocol_field)
        .or_else(|| first_str(obj, &["protocol", "type", "scheme", "proto"]));
    Some(ParsedProxy {
        protocol: normalize_protocol(protocol_raw.as_deref().unwrap_or(""), default_protocol),
        host,
        port,
        username,
        password,
        raw: item.to_string(),
    })
}

fn get_str(obj: &serde_json::Map<String, Value>, field: &str) -> Option<String> {
    if field.is_empty() {
        return None;
    }
    match obj.get(field)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn get_port(obj: &serde_json::Map<String, Value>, field: &str) -> Option<u16> {
    if field.is_empty() {
        return None;
    }
    match obj.get(field)? {
        Value::Number(n) => n.as_u64().and_then(|v| u16::try_from(v).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn first_str(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| get_str(obj, k)).filter(|s| !s.is_empty())
}

pub fn decode_charset(bytes: &[u8], charset: &str) -> String {
    let enc = match charset.to_ascii_lowercase().as_str() {
        "gbk" | "gb2312" | "gb18030" => encoding_rs::GBK,
        "big5" => encoding_rs::BIG5,
        "latin1" | "iso-8859-1" => encoding_rs::WINDOWS_1252,
        _ => encoding_rs::UTF_8,
    };
    enc.decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_various() {
        let a = parse_line("1.2.3.4:8080", "auto", "http").unwrap();
        assert_eq!((a.host.as_str(), a.port), ("1.2.3.4", 8080));
        let b = parse_line("user:pass@1.2.3.4:8080", "auto", "http").unwrap();
        assert_eq!(b.username.as_deref(), Some("user"));
        let c = parse_line("socks5://u:p@1.2.3.4:1080", "auto", "http").unwrap();
        assert_eq!(c.protocol, "socks5");
        let d = parse_line("1.2.3.4:8080@user:pass", "auto", "http").unwrap();
        assert_eq!(d.password.as_deref(), Some("pass"));
    }
}
