//! IP4P 客户端地址解析（frpc 侧，零配置改动）。
//!
//! # IP4P 是什么
//!
//! 当 frps 位于 NAT 之后、公网 IP 与端口会动态变化时，frpc 无法用固定
//! `serverAddr` + `serverPort` 直连。IP4P 把「当前公网 IP:端口」发布到 DNS，
//! frpc 在**每次拨号前**解析它，用解析出的端口覆盖 `serverPort`。
//!
//! 解析优先级与 `wengjing086/frp-ip4p`（Go 版）保持一致：
//!
//! 1. **TXT 记录**（natmap / lucky / DDNS 常用）：`base64("IPv4:端口")`，
//!    例如 `MjAzLjAuMTEzLjc6MTgyNDY=` ⇒ `203.0.113.7:18246`；也兼容明文。
//! 2. **AAAA 记录**：IP4P 编码的 IPv6 地址（`2001:0000::/16`），
//!    端口在字节 10–11，IPv4 在字节 12–15。
//! 3. **普通模式**：无匹配记录时原样使用 `serverAddr` + `serverPort`，
//!    与官方 frp 行为完全一致。
//!
//! # 为什么不需要新配置键
//!
//! 本模块**自动生效**：`serverAddr` 是 IP 字面量时直接跳过（绝大多数部署），
//! 是域名时才尝试解析；没有任何 IP4P 记录时静默回退普通模式。
//! `dnsServer`（官方已有键）若配置，TXT / AAAA 查询会走它。
//! ⇒ 现有 `frpc.toml` / `frps.toml` 一个字都不用改。
//!
//! # 缓存
//!
//! Go 版每次拨号都查 DNS。这里加了一个进程内小缓存：
//! 命中（解析到 IP4P）缓存 10 秒（兼顾 natmap 端口变化），
//! 未命中缓存 60 秒（避免普通域名每条工作连接都发一轮 DNS 查询）。
//! 缓存只影响「多久重查一次」，不影响回退语义。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{
    GeneralPurpose, STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD,
};
use base64::Engine;
use tracing::{debug, info};

/// 命中 IP4P 后的缓存时长。端口是动态的，不能缓存太久。
const POSITIVE_TTL: Duration = Duration::from_secs(10);
/// 未命中（普通域名）的缓存时长，避免每次拨号都查询 DNS。
const NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// 解析结果。`source` 只用于日志，不参与连接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEndpoint {
    pub host: String,
    pub port: u16,
    /// `"txt"` / `"aaaa"` / `"plain"`
    pub source: &'static str,
}

impl ResolvedEndpoint {
    pub fn plain(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            source: "plain",
        }
    }
    pub fn socket_addr_string(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

struct CacheEntry {
    value: Option<ResolvedEndpoint>,
    expires_at: Instant,
}

static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 清空缓存（测试与热重载用）。
pub fn clear_cache() {
    if let Ok(mut g) = cache().lock() {
        g.clear();
    }
}

/// 把一条 DNS TXT 记录解析成 `IP:端口`。
///
/// 兼容四种 base64 变体（标准/无填充/URL-safe/URL-safe 无填充）与明文；
/// 容忍包裹的引号与空白；拒绝非 IP 主机、非法端口与越界端口。
/// 与 Go 版 `parseIP4PRecord` 行为对齐。
pub fn parse_txt_record(record: &str) -> Option<(String, u16)> {
    let v = record.trim().trim_matches('"').trim();
    if v.is_empty() {
        return None;
    }

    let mut candidates: Vec<String> = vec![v.to_string()];
    // `Engine` 带泛型方法，不能做成 trait object；四个常量都是同一个具体类型。
    let engines: [&GeneralPurpose; 4] = [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD];
    for engine in engines {
        if let Ok(decoded) = engine.decode(v.as_bytes()) {
            if let Ok(text) = String::from_utf8(decoded) {
                let t = text.trim().to_string();
                if !t.is_empty() && !candidates.contains(&t) {
                    candidates.push(t);
                }
            }
        }
    }

    for candidate in candidates {
        if let Some((host, port)) = split_ip_port(&candidate) {
            return Some((host, port));
        }
    }
    None
}

/// 只接受 **IP 字面量:端口**（`net.ParseIP` 语义），不接受主机名。
fn split_ip_port(s: &str) -> Option<(String, u16)> {
    let s = s.trim();
    let (host, port_str) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        (h, tail.strip_prefix(':')?)
    } else {
        s.rsplit_once(':')?
    };
    let host = host.trim();
    if host.parse::<IpAddr>().is_err() {
        return None;
    }
    let port: u16 = port_str.trim().parse().ok()?;
    if port == 0 {
        return None;
    }
    Some((host.to_string(), port))
}

/// 解析 IP4P 编码的 AAAA 地址（`2001:0000::/16`）。
///
/// 布局（RFC 4291 的 6to4 风格约定，hevz/frp ip4p 分支使用）：
/// ```text
/// 字节  0  1  2  3  ...  10  11  12  13  14  15
///      20 01 00 00      hi  lo  a   b   c   d     ⇒ a.b.c.d : (hi<<8|lo)
/// ```
pub fn parse_ip4p_aaaa(ip: Ipv6Addr) -> Option<(Ipv4Addr, u16)> {
    let o = ip.octets();
    if o[0] != 0x20 || o[1] != 0x01 || o[2] != 0x00 || o[3] != 0x00 {
        return None;
    }
    let port = (u16::from(o[10]) << 8) | u16::from(o[11]);
    if port == 0 {
        return None;
    }
    Some((Ipv4Addr::new(o[12], o[13], o[14], o[15]), port))
}

/// 拨号前解析服务端地址。**这是客户端唯一需要调用的入口。**
///
/// - `host`：`serverAddr`（域名或 IP 字面量）
/// - `port`：`serverPort`
/// - `dns_server`：官方 `dnsServer` 键，空字符串表示用系统 DNS
pub async fn resolve(host: &str, port: u16, dns_server: &str) -> ResolvedEndpoint {
    let host = host.trim();
    // 快路径：IP 字面量根本不需要查 DNS，绝大多数部署走这里，零额外开销。
    if host.parse::<IpAddr>().is_ok() {
        return ResolvedEndpoint::plain(host, port);
    }
    if host.is_empty() || port == 0 {
        return ResolvedEndpoint::plain(host, port);
    }

    let key = format!("{host}|{port}|{dns_server}");
    if let Ok(g) = cache().lock() {
        if let Some(entry) = g.get(&key) {
            if entry.expires_at > Instant::now() {
                if let Some(v) = &entry.value {
                    debug!(%host, "IP4P 缓存命中");
                    return v.clone();
                }
                return ResolvedEndpoint::plain(host, port);
            }
        }
    }

    let found = lookup_uncached(host, port, dns_server).await;
    let (value, ttl) = match &found {
        Some(v) => (Some(v.clone()), POSITIVE_TTL),
        None => (None, NEGATIVE_TTL),
    };
    if let Ok(mut g) = cache().lock() {
        // 顺手清掉过期项，避免长时间运行时缓存无限增长。
        let now = Instant::now();
        g.retain(|_, e| e.expires_at > now);
        g.insert(
            key,
            CacheEntry {
                value,
                expires_at: now + ttl,
            },
        );
    }
    found.unwrap_or_else(|| ResolvedEndpoint::plain(host, port))
}

async fn lookup_uncached(host: &str, port: u16, dns_server: &str) -> Option<ResolvedEndpoint> {
    let servers = crate::dns::dns_servers_for(dns_server);
    let fallback = crate::dns::public_dns_servers();

    // 1) TXT 记录
    let txt = match crate::dns::lookup_txt(&servers, host).await {
        Ok(v) => v,
        Err(_) => crate::dns::lookup_txt(&fallback, host)
            .await
            .unwrap_or_default(),
    };
    for record in &txt {
        if let Some((ip, p)) = parse_txt_record(record) {
            info!(%host, resolved = %format!("{ip}:{p}"), "ip4p: 由 TXT 记录解析出服务端地址");
            return Some(ResolvedEndpoint {
                host: ip,
                port: p,
                source: "txt",
            });
        }
    }

    // 2) AAAA 记录（自定义/系统 DNS + 系统 resolver）
    let aaaa = match crate::dns::lookup_aaaa(&servers, host).await {
        Ok(v) => v,
        Err(_) => crate::dns::lookup_aaaa(&fallback, host)
            .await
            .unwrap_or_default(),
    };
    let mut candidates = aaaa;
    if let Ok(addrs) = tokio::net::lookup_host((host, port)).await {
        for sa in addrs {
            if let IpAddr::V6(v6) = sa.ip() {
                if !candidates.contains(&v6) {
                    candidates.push(v6);
                }
            }
        }
    }
    for ip in candidates {
        if let Some((v4, p)) = parse_ip4p_aaaa(ip) {
            let ip = v4.to_string();
            info!(%host, resolved = %format!("{ip}:{p}"), "ip4p: 由 IP4P AAAA 记录解析出服务端地址");
            return Some(ResolvedEndpoint {
                host: ip,
                port: p,
                source: "aaaa",
            });
        }
    }

    // 3) 普通模式
    debug!(%host, %port, "ip4p: 未发现 IP4P 记录，按普通模式连接");
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn txt_base64_标准编码() {
        // base64("203.0.113.7:18246")
        let got = parse_txt_record("MjAzLjAuMTEzLjc6MTgyNDY=").unwrap();
        assert_eq!(got, ("203.0.113.7".to_string(), 18246));
    }

    #[test]
    fn txt_明文_与_引号空白() {
        assert_eq!(
            parse_txt_record("  \"203.0.113.7:18246\"  ").unwrap(),
            ("203.0.113.7".to_string(), 18246)
        );
        assert_eq!(
            parse_txt_record("203.0.113.7:18246").unwrap(),
            ("203.0.113.7".to_string(), 18246)
        );
    }

    #[test]
    fn txt_无填充_与_url_safe() {
        // 造一个 base64 里含 '+' 或 '/' 的值，验证 URL-safe 变体
        let raw = "252.253.254.255:65535";
        let enc = URL_SAFE.encode(raw.as_bytes());
        assert_eq!(
            parse_txt_record(&enc).unwrap(),
            ("252.253.254.255".to_string(), 65535)
        );
        let enc_no_pad = URL_SAFE_NO_PAD.encode(raw.as_bytes());
        assert_eq!(
            parse_txt_record(&enc_no_pad).unwrap(),
            ("252.253.254.255".to_string(), 65535)
        );
    }

    #[test]
    fn txt_拒绝非法值() {
        for bad in [
            "",
            "   ",
            "not-an-address",
            "example.com:7000",
            "203.0.113.7:0",
            "203.0.113.7:70000",
            "203.0.113.7",
            "203.0.113.7:abc",
        ] {
            assert!(parse_txt_record(bad).is_none(), "不该接受 {bad:?}");
        }
    }

    #[test]
    fn aaaa_ip4p_解码() {
        // 203.0.113.7 : 18246 (0x4746)
        let mut o = [0u8; 16];
        o[0] = 0x20;
        o[1] = 0x01;
        o[10] = 0x47;
        o[11] = 0x46;
        o[12..16].copy_from_slice(&[203, 0, 113, 7]);
        let ip = Ipv6Addr::from(o);
        let (v4, port) = parse_ip4p_aaaa(ip).unwrap();
        assert_eq!(v4, Ipv4Addr::new(203, 0, 113, 7));
        assert_eq!(port, 18246);
    }

    #[test]
    fn aaaa_非_ip4p_前缀要拒绝() {
        let ip = Ipv6Addr::from_str("2001:db8::1").unwrap();
        assert!(parse_ip4p_aaaa(ip).is_none());
        let ip = Ipv6Addr::from_str("::1").unwrap();
        assert!(parse_ip4p_aaaa(ip).is_none());
    }

    #[tokio::test]
    async fn ip_字面量不走_dns_且原样返回() {
        clear_cache();
        let r = resolve("127.0.0.1", 7000, "").await;
        assert_eq!(r, ResolvedEndpoint::plain("127.0.0.1", 7000));
        assert_eq!(r.socket_addr_string(), "127.0.0.1:7000");
    }

    /// 真起一个本地假 DNS 服务器，验证「TXT 命中 ⇒ 端口覆盖 serverPort」这条
    /// 完整链路（DNS 报文 → TXT 解析 → base64 解码 → 端点替换）。
    #[tokio::test]
    async fn resolve_由_txt_记录解析端点并覆盖端口() {
        clear_cache();
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dns = sock.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let (n, peer) = sock.recv_from(&mut buf).await.unwrap();
            let req = &buf[..n];
            let mut resp = Vec::new();
            resp.extend_from_slice(&req[0..2]); // 回显事务 ID
            resp.extend_from_slice(&[0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0]); // QR/RD/RA, QD=1, AN=1
                                                                           // 原样回显问题段：逐个 label 走到 0，再 +4 字节 QTYPE/QCLASS
            let mut p = 12usize;
            while p < req.len() && req[p] != 0 {
                p += 1 + req[p] as usize;
            }
            p += 5;
            resp.extend_from_slice(&req[12..p]);

            let value = b"MjAzLjAuMTEzLjc6MTgyNDY="; // 203.0.113.7:18246
            resp.extend_from_slice(&[0xC0, 0x0C]); // 名字压缩指针
            resp.extend_from_slice(&[0, 16, 0, 1, 0, 0, 0, 60]); // TYPE=TXT, CLASS=IN, TTL
            resp.extend_from_slice(&[0, (value.len() + 1) as u8]); // RDLEN
            resp.push(value.len() as u8); // TXT 字符串长度
            resp.extend_from_slice(value);
            sock.send_to(&resp, peer).await.unwrap();
        });

        let r = resolve("ip4p.example.com", 7000, &dns.to_string()).await;
        handle.await.unwrap();
        assert_eq!(r.host, "203.0.113.7");
        assert_eq!(r.port, 18246, "TXT 里的端口必须覆盖 serverPort");
        assert_eq!(r.source, "txt");
        assert_eq!(r.socket_addr_string(), "203.0.113.7:18246");
    }
}
