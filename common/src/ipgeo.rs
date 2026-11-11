//! 客户端 IP 归属地（省 / 市 / 运营商）—— **在线查询，不内置数据库**。
//!
//! # 为什么是"在线查询"
//!
//! 内置 GeoLite2 / ip2region 之类的库要带几十兆数据，而 frps 二进制
//! 本来只有几兆 —— 为了面板上一列小字塞一份 IP 库，不划算。
//! 所以这里沿用 `frp-wss-ipgeo` 的思路：**用到才查，查完缓存**。
//!
//! # 行为边界（重要）
//!
//! * **只对中国大陆 IP 追加归属地**；境外、内网、保留地址一律只显示 IP。
//!   判定方式见 [`Geo::is_china`]：`countryCode == CN`，或省份名命中
//!   34 个省级行政区之一（百度接口只回中文地址、不给国家码，只能靠省名兜底）。
//! * 查询是**异步、不阻塞**的：面板 / API 取值时命中缓存就返回，
//!   没命中就返回裸 IP 并在后台补查，下一次刷新自然就有了。
//! * 结果写 `ipgeo_cache.json`（可配置），重启不重查；坏文件当空缓存处理，
//!   不会因为一个手改坏的 JSON 让 frps 起不来。
//! * 任何一个源都查不到时缓存空结果（负缓存），避免面板轮询把上游打爆。
//!
//! # 数据源（按顺序降级）
//!
//! | 顺序 | 源 | 协议 | 语言 | 说明 |
//! |---|---|---|---|---|
//! | 1 | `opendata.baidu.com` | HTTPS | 中文 | 国内最稳，`location` 形如 `浙江省杭州市 阿里云` |
//! | 2 | `ip-api.com` | HTTP | 中文 | 带 `countryCode`，能明确判定境内/境外 |
//! | 3 | `api.ip.sb` | HTTPS | 英文 | 纯兜底，只用来判国家 |
//!
//! # 隐私
//!
//! 开启后，**被面板看到的客户端 IP** 会被发往上述第三方接口。
//! 介意就把 `ipgeo = false` 写进 `frps.toml`（或设 `NFRP_IPGEO=0`），
//! 此时一行查询都不会发出去。

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::httpc::{self, Options};

/// 缓存文件名（相对 `cache_file` 未指定时的当前目录）。
pub const CACHE_FILE: &str = "ipgeo_cache.json";

/// 单条归属地信息的有效期：7 天。运营商会变，但不会天天变。
const TTL_SECS: u64 = 7 * 24 * 3600;

/// 缓存条目上限。超过就按"最旧先丢"清理，防止长期运行内存无界增长。
const MAX_ENTRIES: usize = 4096;

/// 单个字段的最大字符数。上游返回什么都不能让它撑爆面板。
const MAX_FIELD_CHARS: usize = 32;

/// 一次查询的整体超时（每个源各自 5 秒）。
const QUERY_TIMEOUT_SECS: u64 = 5;

/// 34 个省级行政区的**简称**（用于判定"这是不是国内地址"）。
///
/// 百度接口只回中文地址、不给国家码 —— `美国加利福尼亚圣何塞` 与
/// `浙江省杭州市` 长得一样，只能靠"省名是否在白名单里"来区分。
const PROVINCES: [&str; 34] = [
    "北京", "天津", "河北", "山西", "内蒙古", "辽宁", "吉林", "黑龙江", "上海", "江苏",
    "浙江", "安徽", "福建", "江西", "山东", "河南", "湖北", "湖南", "广东", "广西",
    "海南", "重庆", "四川", "贵州", "云南", "西藏", "陕西", "甘肃", "青海", "宁夏",
    "新疆", "台湾", "香港", "澳门",
];

/// 一条归属地信息。字段全部为**已清洗**的纯文本。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Geo {
    /// 国家名（中文或英文，取决于来源），可能为空。
    #[serde(default)]
    pub country: String,
    /// ISO 3166-1 alpha-2 国家码，如 `CN`；百度源没有这个字段。
    #[serde(default)]
    pub country_code: String,
    /// 省 / 直辖市 / 自治区，保留原后缀（`浙江省`）。
    #[serde(default)]
    pub region: String,
    /// 市（`杭州市`），可能为空。
    #[serde(default)]
    pub city: String,
    /// 运营商 / 云厂商（`电信` / `阿里云`），可能为空。
    #[serde(default)]
    pub isp: String,
}

impl Geo {
    /// 是否中国大陆地址。
    ///
    /// 国家码优先；没有国家码（百度源）时看省份简称是否在白名单里。
    pub fn is_china(&self) -> bool {
        if !self.country_code.is_empty() {
            return self.country_code.eq_ignore_ascii_case("CN");
        }
        let r = short_region(&self.region);
        PROVINCES.contains(&r.as_str())
    }

    /// 面板上展示的短串：`浙江 杭州 阿里云`。
    ///
    /// 境外 / 无法识别时返回**空串** —— 调用方据此只显示 IP。
    pub fn display(&self) -> String {
        if !self.is_china() {
            return String::new();
        }
        let region = short_region(&self.region);
        let city = short_city(&self.city);
        let isp = clean(&self.isp);
        let mut parts: Vec<String> = Vec::new();
        if !region.is_empty() {
            parts.push(region.clone());
        }
        // 直辖市的 region 与 city 是同一个词（`北京市北京市`），去重
        if !city.is_empty() && city != region {
            parts.push(city);
        }
        if !isp.is_empty() {
            parts.push(isp);
        }
        parts.join(" ")
    }
}

/// 缓存里的一条：展示串 + 写入时间（Unix 秒）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// 展示串；空串 = "查过了，但没有可展示的归属地"（境外/查不到）。
    d: String,
    /// 写入时间，用于跨进程的 TTL。
    t: u64,
}

#[derive(Default)]
struct State {
    map: HashMap<String, Entry>,
    /// 正在查的 IP，避免面板每 3 秒轮询就重复发一次请求。
    inflight: std::collections::HashSet<String>,
    /// 有未落盘的改动。
    dirty: bool,
    /// 上次落盘时间，用于节流。
    last_save: Option<std::time::Instant>,
}

/// 归属地查询器（带缓存）。进程内用 [`init`] / [`global`] 拿全局单例。
pub struct IpGeo {
    enabled: bool,
    cache_file: Option<PathBuf>,
    state: Mutex<State>,
}

impl IpGeo {
    /// 构造一个查询器。
    ///
    /// `cache_file` 为 `None` 时用当前目录的 [`CACHE_FILE`]；`enabled == false`
    /// 时一行请求都不会发，[`Self::annotate`] 直接返回裸 IP。
    pub fn new(enabled: bool, cache_file: Option<PathBuf>) -> Self {
        let path = if enabled {
            Some(cache_file.unwrap_or_else(|| PathBuf::from(CACHE_FILE)))
        } else {
            None
        };
        let mut state = State::default();
        if let Some(p) = &path {
            state.map = load_cache(p);
        }
        Self {
            enabled,
            cache_file: path,
            state: Mutex::new(state),
        }
    }

    /// 是否开启（关闭时所有查询都被跳过）。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 已缓存的展示串（`浙江 杭州 阿里云`）；`None` = 还没查过。
    ///
    /// 注意 `Some("")` 是**负缓存**：查过了，但没有可展示的归属地。
    pub fn cached(&self, ip: &str) -> Option<String> {
        let g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.map.get(ip).map(|e| e.d.clone())
    }

    /// 拼成 `1.2.3.4 (浙江 杭州 阿里云)`；内网 / 境外 / 未命中则是裸 IP。
    ///
    /// 未命中时会**顺手在后台发起查询**，本次调用不等待 —— 面板轮询一次
    /// 拿不到归属地是正常的，下一次刷新就有了。
    pub fn annotate(&self, ip: &str) -> String {
        if !self.enabled {
            return ip.to_string();
        }
        if is_local_or_reserved_str(ip) {
            return ip.to_string();
        }
        match self.cached(ip) {
            Some(d) if d.is_empty() => ip.to_string(),
            Some(d) => format!("{ip} ({d})"),
            None => ip.to_string(),
        }
    }

    /// 缓存未命中时后台补查（幂等，正在查的不会重复发起）。
    ///
    /// 自己拿 [`Arc`] 克隆进任务：进程退出时任务被丢弃，不会泄漏。
    pub fn request(self: &Arc<Self>, ip: &str) {
        if !self.enabled || is_local_or_reserved_str(ip) {
            return;
        }
        {
            let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if g.map.contains_key(ip) || !g.inflight.insert(ip.to_string()) {
                return;
            }
        }
        let me = Arc::clone(self);
        let ip = ip.to_string();
        // 没有 runtime 时（单元测试 / 同步上下文）直接放弃，绝 panic。
        if tokio::runtime::Handle::try_current().is_err() {
            let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
            g.inflight.remove(&ip);
            return;
        }
        tokio::spawn(async move {
            let geo = lookup(&ip).await;
            me.store(&ip, geo);
        });
    }

    /// 写入查询结果并择机落盘。
    fn store(&self, ip: &str, geo: Option<Geo>) {
        let display = geo.map(|g| g.display()).unwrap_or_default();
        let now = unix_now();
        {
            let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
            g.inflight.remove(ip);
            g.map.insert(
                ip.to_string(),
                Entry {
                    d: display,
                    t: now,
                },
            );
            if g.map.len() > MAX_ENTRIES {
                evict_oldest(&mut g.map);
            }
            g.dirty = true;
        }
        self.maybe_save();
    }

    /// 落盘节流：最快 3 秒一次，避免一次刷出几十个客户端时反复写盘。
    fn maybe_save(&self) {
        let Some(path) = &self.cache_file else {
            return;
        };
        let snapshot = {
            let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !g.dirty {
                return;
            }
            let due = g
                .last_save
                .map(|t| t.elapsed() >= Duration::from_secs(3))
                .unwrap_or(true);
            if !due {
                return;
            }
            g.dirty = false;
            g.last_save = Some(std::time::Instant::now());
            g.map.clone()
        };
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(dir);
            }
        }
        // 先写临时文件再 rename：断电 / 崩溃时不会留下半截 JSON。
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec(&snapshot).unwrap_or_else(|_| b"{}".to_vec());
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// 丢掉最旧的条目，直到回到上限以内。
fn evict_oldest(map: &mut HashMap<String, Entry>) {
    let mut keys: Vec<(String, u64)> = map.iter().map(|(k, v)| (k.clone(), v.t)).collect();
    keys.sort_by_key(|(_, t)| *t);
    let over = map.len().saturating_sub(MAX_ENTRIES);
    for (k, _) in keys.into_iter().take(over) {
        map.remove(&k);
    }
}

/// 读缓存文件；坏文件 / 不存在的文件都当空缓存（**绝不让面板起不来**）。
fn load_cache(path: &Path) -> HashMap<String, Entry> {
    let Ok(raw) = std::fs::read(path) else {
        return HashMap::new();
    };
    let Ok(map) = serde_json::from_slice::<HashMap<String, Entry>>(&raw) else {
        tracing::warn!(file = %path.display(), "归属地缓存不是合法 JSON，按空缓存处理");
        return HashMap::new();
    };
    let now = unix_now();
    map.into_iter()
        .filter(|(_, e)| now.saturating_sub(e.t) <= TTL_SECS)
        .filter(|(k, _)| k.parse::<IpAddr>().is_ok())
        .take(MAX_ENTRIES)
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------- 全局单例

static GLOBAL: OnceLock<Arc<IpGeo>> = OnceLock::new();

/// 初始化全局查询器；重复调用以第一次为准。
///
/// `enabled` 还会被环境变量 `NFRP_IPGEO=0` 强制关掉（方便容器里临时禁用，
/// 不用改配置文件）。
pub fn init(enabled: bool, cache_file: Option<PathBuf>) -> &'static Arc<IpGeo> {
    let enabled = enabled && !disabled_by_env();
    GLOBAL.get_or_init(|| Arc::new(IpGeo::new(enabled, cache_file)))
}

/// 取全局查询器；未显式 [`init`] 时按"默认开启 + 当前目录缓存"惰性建立。
pub fn global() -> &'static Arc<IpGeo> {
    GLOBAL.get_or_init(|| Arc::new(IpGeo::new(true, None)))
}

fn disabled_by_env() -> bool {
    matches!(
        std::env::var("NFRP_IPGEO").unwrap_or_default().trim(),
        "0" | "off" | "false" | "no"
    )
}

// ---------------------------------------------------------------- 查询

/// 依次问三个源，第一个给出结果的胜出。
async fn lookup(ip: &str) -> Option<Geo> {
    let opts = Options {
        timeout_secs: QUERY_TIMEOUT_SECS,
        ..Default::default()
    };
    let ua = [("User-Agent", "nfrp/0.8 (+https://github.com/wengjing086/rust-frp-wss-ip4p)")];
    for (name, url) in [
        (
            "baidu",
            format!("https://opendata.baidu.com/api.php?query={ip}&co=&resource_id=6006&oe=utf8"),
        ),
        (
            "ip-api",
            format!("http://ip-api.com/json/{ip}?lang=zh-CN&fields=status,message,country,countryCode,regionName,city,isp"),
        ),
        ("ip.sb", format!("https://api.ip.sb/geoip/{ip}")),
    ] {
        match httpc::get(&url, &ua, &opts).await {
            Ok(resp) if resp.status == 200 => {
                let parsed = resp.json().ok().and_then(|v| match name {
                    "baidu" => parse_baidu(&v),
                    "ip-api" => parse_ipapi(&v),
                    _ => parse_ipsb(&v),
                });
                if let Some(g) = parsed {
                    return Some(g);
                }
                tracing::debug!(source = name, ip, "归属地接口有响应但没解析出结果");
            }
            Ok(resp) => tracing::debug!(source = name, ip, status = resp.status, "归属地查询异常"),
            Err(e) => tracing::debug!(source = name, ip, error = %e, "归属地查询失败"),
        }
    }
    None
}

/// 百度 open data：`{"status":"0","data":[{"location":"浙江省杭州市 阿里云"}]}`
///
/// 没有国家码，`is_china()` 靠省名白名单判定；境外地址形如
/// `美国` / `美国加利福尼亚圣何塞`，省名匹配不上，自然被过滤掉。
fn parse_baidu(v: &serde_json::Value) -> Option<Geo> {
    if v["status"].as_str() != Some("0") {
        return None;
    }
    let loc = v["data"].get(0)?.get("location")?.as_str()?;
    let loc = clean(loc);
    if loc.is_empty() {
        return None;
    }
    let mut it = loc.split_whitespace();
    let addr = it.next().unwrap_or("");
    let isp = it.collect::<Vec<_>>().join(" ");
    let (region, city) = split_region_city(addr);
    let g = Geo {
        country: String::new(),
        country_code: String::new(),
        region,
        city,
        isp,
    };
    (!g.region.is_empty()).then_some(g)
}

/// ip-api：`{"status":"success","country":"中国","countryCode":"CN",...}`
fn parse_ipapi(v: &serde_json::Value) -> Option<Geo> {
    if v["status"].as_str() != Some("success") {
        return None;
    }
    let g = Geo {
        country: clean(v["country"].as_str().unwrap_or("")),
        country_code: clean(v["countryCode"].as_str().unwrap_or("")).to_uppercase(),
        region: clean(v["regionName"].as_str().unwrap_or("")),
        city: clean(v["city"].as_str().unwrap_or("")),
        isp: clean(v["isp"].as_str().unwrap_or("")),
    };
    (!g.country_code.is_empty()).then_some(g)
}

/// ip.sb：`{"country":"China","country_code":"CN","region":"Zhejiang",...}`
fn parse_ipsb(v: &serde_json::Value) -> Option<Geo> {
    let g = Geo {
        country: clean(v["country"].as_str().unwrap_or("")),
        country_code: clean(v["country_code"].as_str().unwrap_or("")).to_uppercase(),
        region: clean(v["region"].as_str().unwrap_or("")),
        city: clean(v["city"].as_str().unwrap_or("")),
        isp: clean(v["isp"].as_str().or(v["organization"].as_str()).unwrap_or("")),
    };
    (!g.country_code.is_empty()).then_some(g)
}

// ---------------------------------------------------------------- 文本处理

/// 只保留"看得懂、塞得进表格"的字符：ASCII 字母数字、汉字、空格、`-`、`.`、`·`。
///
/// 上游返回的内容会进 HTML —— 面板那边已经做了转义，这里再削一层，
/// 顺便把超长串截断，避免有人用畸形 ISP 名把面板撑变形。
fn clean(s: &str) -> String {
    let filtered: String = s
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric()
                || *c == '-'
                || *c == '.'
                || *c == ' '
                || *c == '·'
                || ('\u{4e00}'..='\u{9fff}').contains(c)
        })
        .collect();
    let squeezed = filtered.split_whitespace().collect::<Vec<_>>().join(" ");
    squeezed.chars().take(MAX_FIELD_CHARS).collect()
}

/// `浙江省` → `浙江`，`广西壮族自治区` → `广西`，`北京市` → `北京`。
fn short_region(s: &str) -> String {
    let s = clean(s);
    for suf in ["特别行政区", "自治区", "省", "市"] {
        if let Some(base) = s.strip_suffix(suf) {
            // 自治区要再削掉民族名：广西壮族 / 宁夏回族 / 新疆维吾尔
            let base = ["壮族", "回族", "维吾尔"]
                .iter()
                .find_map(|e| base.strip_suffix(e))
                .unwrap_or(base);
            return base.to_string();
        }
    }
    s
}

/// `杭州市` → `杭州`。
fn short_city(s: &str) -> String {
    let s = clean(s);
    s.strip_suffix('市').unwrap_or(&s).to_string()
}

/// 把 `浙江省杭州市` 切成 `(浙江省, 杭州市)`。
///
/// 处理四种写法：`浙江省杭州市`、`北京市`（省市同名）、
/// `内蒙古自治区呼和浩特市`、`新疆维吾尔自治区乌鲁木齐市`。
fn split_region_city(addr: &str) -> (String, String) {
    let addr = clean(addr);
    if addr.is_empty() {
        return (String::new(), String::new());
    }
    let mut cut: Option<usize> = None;
    for marker in ["特别行政区", "自治区", "省", "市"] {
        if let Some(pos) = addr.find(marker) {
            let end = pos + marker.len();
            if cut.map(|c| end < c).unwrap_or(true) {
                cut = Some(end);
            }
        }
    }
    match cut {
        Some(end) if end < addr.len() => (
            addr[..end].to_string(),
            addr[end..].trim_start_matches(' ').to_string(),
        ),
        _ => (addr.clone(), addr),
    }
}

/// 内网 / 保留 / 不可路由地址：这些不发查询（也查不出东西）。
fn is_local_or_reserved_str(ip: &str) -> bool {
    match ip.parse::<IpAddr>() {
        Ok(a) => is_local_or_reserved(&a),
        // 解析不了（比如带 zone 的 `fe80::1%eth0`）也当本地处理，宁可不查
        Err(_) => true,
    }
}

/// 内网 / 保留 / 不可路由地址判定。
pub fn is_local_or_reserved(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                // 100.64.0.0/10 运营商级 NAT
                || (o[0] == 100 && (64..128).contains(&o[1]))
                // 198.18.0.0/15 基准测试网段
                || (o[0] == 198 && (18..20).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local_or_reserved(&IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 唯一本地地址
                || (s[0] & 0xfe00) == 0xfc00
                // fe80::/10 链路本地
                || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 百度结果解析出省市运营商() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"status":"0","data":[{"location":"浙江省杭州市 阿里云","origip":"223.5.5.5"}]}"#,
        )
        .unwrap();
        let g = parse_baidu(&v).expect("应解析出结果");
        assert_eq!(g.region, "浙江省");
        assert_eq!(g.city, "杭州市");
        assert_eq!(g.isp, "阿里云");
        assert_eq!(g.display(), "浙江 杭州 阿里云");
        assert!(g.is_china());
    }

    #[test]
    fn 百度直辖市的省市同名不重复() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"status":"0","data":[{"location":"北京市北京市 联通"}]}"#,
        )
        .unwrap();
        let g = parse_baidu(&v).unwrap();
        assert_eq!(g.display(), "北京 联通", "直辖市不该出现两次");
    }

    #[test]
    fn 百度境外地址被过滤掉() {
        for loc in ["美国", "美国加利福尼亚圣何塞", "澳大利亚"] {
            let v: serde_json::Value = serde_json::json!({
                "status": "0",
                "data": [{ "location": loc }],
            });
            let g = parse_baidu(&v);
            if let Some(g) = g {
                assert!(
                    g.display().is_empty(),
                    "境外地址不该显示归属地，实际得到 {:?}",
                    g.display()
                );
            }
        }
    }

    #[test]
    fn 百度失败状态返回_none() {
        let v: serde_json::Value = serde_json::json!({"status":"1","data":[]});
        assert!(parse_baidu(&v).is_none());
    }

    #[test]
    fn ip_api_国内与国外() {
        let cn: serde_json::Value = serde_json::json!({
            "status":"success","country":"中国","countryCode":"CN",
            "regionName":"浙江","city":"杭州","isp":"Hangzhou Alibaba Advertising Co"
        });
        let g = parse_ipapi(&cn).unwrap();
        assert!(g.is_china());
        assert_eq!(g.display(), "浙江 杭州 Hangzhou Alibaba Advertising Co");

        let us: serde_json::Value = serde_json::json!({
            "status":"success","country":"United States","countryCode":"US",
            "regionName":"California","city":"San Jose","isp":"Google"
        });
        let g = parse_ipapi(&us).unwrap();
        assert!(!g.is_china());
        assert_eq!(g.display(), "");
    }

    #[test]
    fn ip_sb_解析() {
        let v: serde_json::Value = serde_json::json!({
            "country":"China","country_code":"CN","region":"Zhejiang",
            "city":"Hangzhou","isp":"Alibaba"
        });
        let g = parse_ipsb(&v).unwrap();
        assert!(g.is_china());
        assert_eq!(g.display(), "Zhejiang Hangzhou Alibaba");
    }

    #[test]
    fn 省名规范化() {
        assert_eq!(short_region("浙江省"), "浙江");
        assert_eq!(short_region("北京市"), "北京");
        assert_eq!(short_region("内蒙古自治区"), "内蒙古");
        assert_eq!(short_region("广西壮族自治区"), "广西");
        assert_eq!(short_region("宁夏回族自治区"), "宁夏");
        assert_eq!(short_region("新疆维吾尔自治区"), "新疆");
        assert_eq!(short_region("香港特别行政区"), "香港");
        assert_eq!(short_region("浙江"), "浙江");
    }

    #[test]
    fn 地址切分() {
        assert_eq!(
            split_region_city("浙江省杭州市"),
            ("浙江省".to_string(), "杭州市".to_string())
        );
        assert_eq!(
            split_region_city("内蒙古自治区呼和浩特市"),
            ("内蒙古自治区".to_string(), "呼和浩特市".to_string())
        );
        assert_eq!(
            split_region_city("北京市"),
            ("北京市".to_string(), "北京市".to_string())
        );
        assert_eq!(
            split_region_city("新疆维吾尔自治区乌鲁木齐市"),
            ("新疆维吾尔自治区".to_string(), "乌鲁木齐市".to_string())
        );
    }

    #[test]
    fn 内网与保留地址不查询() {
        for ip in [
            "127.0.0.1",
            "192.168.1.10",
            "10.0.0.5",
            "172.16.3.4",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.0.1",
        ] {
            let a: IpAddr = ip.parse().unwrap();
            assert!(is_local_or_reserved(&a), "{ip} 应被视为内网/保留地址");
        }
        for ip in ["223.5.5.5", "8.8.8.8", "2408:8000::1"] {
            let a: IpAddr = ip.parse().unwrap();
            assert!(!is_local_or_reserved(&a), "{ip} 应被视为公网地址");
        }
    }

    #[test]
    fn 关闭时只返回裸_ip() {
        let g = IpGeo::new(false, None);
        assert!(!g.enabled());
        assert_eq!(g.annotate("223.5.5.5"), "223.5.5.5");
    }

    #[test]
    fn 命中缓存时拼接归属地() {
        let g = IpGeo::new(true, None);
        g.store("223.5.5.5", Some(Geo {
            country_code: "CN".into(),
            region: "浙江省".into(),
            city: "杭州市".into(),
            isp: "阿里云".into(),
            ..Default::default()
        }));
        assert_eq!(g.annotate("223.5.5.5"), "223.5.5.5 (浙江 杭州 阿里云)");
        // 负缓存：查过但没结果 → 只显示 IP
        g.store("8.8.8.8", None);
        assert_eq!(g.annotate("8.8.8.8"), "8.8.8.8");
    }

    #[test]
    fn 内网地址直接返回裸_ip_不等查询() {
        let g = IpGeo::new(true, None);
        assert_eq!(g.annotate("192.168.1.2"), "192.168.1.2");
        assert!(g.cached("192.168.1.2").is_none(), "不该为内网地址建缓存");
    }

    #[test]
    fn 缓存文件能读回并忽略过期条目() {
        let dir = std::env::temp_dir().join(format!("nfrp-ipgeo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("cache.json");
        let now = unix_now();
        let body = serde_json::json!({
            "223.5.5.5": { "d": "浙江 杭州 阿里云", "t": now },
            "1.1.1.1": { "d": "旧数据", "t": now.saturating_sub(TTL_SECS + 60) },
            "不是IP": { "d": "脏数据", "t": now },
        });
        std::fs::write(&f, serde_json::to_vec(&body).unwrap()).unwrap();

        let g = IpGeo::new(true, Some(f.clone()));
        assert_eq!(g.cached("223.5.5.5").as_deref(), Some("浙江 杭州 阿里云"));
        assert!(g.cached("1.1.1.1").is_none(), "过期条目应被丢弃");
        assert!(g.cached("不是IP").is_none(), "非 IP 键应被丢弃");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 缓存文件损坏不影响启动() {
        let dir = std::env::temp_dir().join(format!("nfrp-ipgeo-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("cache.json");
        std::fs::write(&f, b"{ this is not json").unwrap();
        let g = IpGeo::new(true, Some(f));
        assert!(g.cached("223.5.5.5").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 清洗会削掉控制字符与超长串() {
        assert_eq!(clean("阿里\u{7f}云\u{1}"), "阿里云");
        assert_eq!(clean("  a   b  "), "a b");
        let long = "浙".repeat(100);
        assert_eq!(clean(&long).chars().count(), MAX_FIELD_CHARS);
    }

    /// **真实联网**校验：默认不跑，避免 CI 依赖外网。
    ///
    /// ```bash
    /// cargo test -p nfrp-common --lib ipgeo -- --ignored --nocapture
    /// ```
    ///
    /// 这条用例的意义：解析逻辑有 fixture 撑着，但"接口地址还通不通、
    /// TLS 根证书能不能验过"只有真跑一次才知道。
    #[tokio::test]
    #[ignore = "需要外网：验证三个归属地接口当前可用"]
    async fn 真实查询_阿里_dns_应能拿到浙江杭州() {
        let g = lookup("223.5.5.5").await.expect("三个源都失败了");
        println!("查询结果：{:?} → {:?}", g, g.display());
        assert!(g.is_china(), "223.5.5.5 应判定为国内地址");
        assert!(!g.display().is_empty(), "国内地址应能拼出归属地");
    }
}
