//! 官方 frps（v0.72）面板兼容 API。
//!
//! # 为什么独立成模块
//!
//! 官方 Vue 面板（`_frp_src/web/frps`，vite 构建）按**官方 JSON 契约**取数：
//!
//! * v1 端点直接返回对象本体，错误是 `{"code":404,"msg":"..."}`；
//! * v2 端点一律包 `{code:200,msg:"success",data:...}` 信封，分页在
//!   `data: {total,page,pageSize,items}`；
//! * 我们自己的 `api_v2` 信封（`{api,data,page}`）与它**不兼容**，
//!   所以这里按官方契约重写一版，并在 dashboard 路由里**优先接管**
//!   冲突路径（`/api/v2/clients`、`/api/v2/proxies`）。
//!
//! # 字段出处
//!
//! 所有字段名与官方 `server/http/model/types.go` / `model/v2.go` 一一对应，
//! 流量方向语义见 [`crate::stats`] 的模块注释。

use std::sync::Arc;

use crate::{
    registry::Registry,
    stats::{ProxyStat, RESERVE_DAYS},
};
use nfrp_common::config::ServerConfig;

/// 与官方 `version.Full()` 对应的位置（前端展示在系统信息里）。
const VERSION: &str = concat!("nfrp v", env!("CARGO_PKG_VERSION"));

pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    fn json(status: u16, body: String) -> Self {
        Self { status, body }
    }
}

/// v1 风格错误：`{"code":..,"msg":".."}`。
fn v1_error(status: u16, msg: &str) -> Response {
    Response::json(
        status,
        serde_json::json!({ "code": status, "msg": msg }).to_string(),
    )
}

/// v2 成功信封：`{"code":200,"msg":"success","data":...}`。
fn v2_ok(data: serde_json::Value) -> Response {
    Response::json(
        200,
        serde_json::json!({ "code": 200, "msg": "success", "data": data }).to_string(),
    )
}

/// v2 错误信封：`{"code":..,"msg":"..","data":null}`。
fn v2_error(status: u16, msg: &str) -> Response {
    Response::json(
        status,
        serde_json::json!({ "code": status, "msg": msg, "data": null }).to_string(),
    )
}

fn secs_to_str(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let day = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02}:{:02}",
        crate::stats::day_to_date(day),
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `allow_ports` 的官方字符串形式（"1000-2000,3000"）。
fn allow_ports_str(cfg: &ServerConfig) -> String {
    cfg.allow_ports
        .iter()
        .map(|r| {
            if r.start == r.end {
                r.start.to_string()
            } else {
                format!("{}-{}", r.start, r.end)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// `/api/serverinfo` 与 `/api/v2/system/info` 共用的状态汇总。
struct ServerStatus {
    total_in: u64,
    total_out: u64,
    cur_conns: i64,
    client_counts: i64,
    type_counts: std::collections::HashMap<String, i64>,
}

fn server_status(registry: &Registry) -> ServerStatus {
    let (total_in, total_out) = registry.stats.total_today_in_out();
    ServerStatus {
        total_in: total_in as i64 as u64,
        total_out: total_out as i64 as u64,
        cur_conns: registry.stats.total_cur_conns(),
        client_counts: registry.sessions.online_count(),
        type_counts: registry.stats.type_counts(),
    }
}

/// 与官方 `ServerInfoResp`（model/types.go）逐字段对齐。
fn serverinfo_json(registry: &Registry, cfg: &ServerConfig) -> serde_json::Value {
    let st = server_status(registry);
    serde_json::json!({
        "version": VERSION,
        "bindPort": cfg.bind_port,
        "vhostHTTPPort": cfg.vhost_http_port.unwrap_or(0),
        "vhostHTTPSPort": cfg.vhost_https_port.unwrap_or(0),
        "tcpmuxHTTPConnectPort": cfg.tcpmux_http_connect_port.unwrap_or(0),
        "kcpBindPort": cfg.kcp_bind_port.unwrap_or(0),
        "quicBindPort": cfg.quic_bind_port.unwrap_or(0),
        "subdomainHost": cfg.subdomain_host,
        "maxPoolCount": cfg.max_pool_count as i64,
        "maxPortsPerClient": cfg.max_ports_per_client,
        "heartbeatTimeout": cfg.heartbeat_timeout as i64,
        "allowPortsStr": allow_ports_str(cfg),
        "tlsForce": cfg.tls_force,
        "totalTrafficIn": st.total_in as i64,
        "totalTrafficOut": st.total_out as i64,
        "curConns": st.cur_conns,
        "clientCounts": st.client_counts,
        "proxyTypeCount": st.type_counts,
    })
}

/// 与官方 `ClientInfoResp` 逐字段对齐。
fn client_info_json(m: &crate::stats::SessionMeta) -> serde_json::Value {
    serde_json::json!({
        "key": m.client_id,
        "user": m.user,
        "clientID": m.client_id,
        "runID": m.run_id,
        "version": m.version,
        "wireProtocol": "",
        "hostname": m.hostname,
        "clientIP": m.ip,
        "firstConnectedAt": m.first_connected,
        "lastConnectedAt": m.last_connected,
        "disconnectedAt": if m.disconnected > 0 { m.disconnected } else { 0 },
        "online": m.online,
    })
}

fn client_conns_and_proxies(registry: &Registry, client_id: &str) -> (i64, i64) {
    let mut conns = 0i64;
    let mut proxies = 0i64;
    for p in registry.stats.all() {
        if p.client_id == client_id {
            proxies += 1;
            conns += p.cur_conns();
        }
    }
    (conns, proxies)
}

/// 与官方 `ProxyStatsInfo` 逐字段对齐（v1 列表项）。
fn proxy_stats_json(p: &ProxyStat) -> serde_json::Value {
    let (tin, tout) = p.today_in_out();
    serde_json::json!({
        // ★ v0.8.5：`name` / `type` 必须在**顶层**。
        //   官方 `ProxyInfoResp` 就是这两个字段在最外层，`conf` 只是原始配置
        //   —— 之前只把 name/type 留在 conf 里，按官方结构写的第三方面板 /
        //   脚本读 `item.name` 会拿到 undefined，只能靠翻 conf 才能用。
        "name": p.name,
        "type": p.proxy_type,
        "conf": p.conf,
        "user": p.user,
        "clientID": p.client_id,
        "todayTrafficIn": tin as i64,
        "todayTrafficOut": tout as i64,
        "curConns": p.cur_conns(),
        "lastStartTime": secs_to_str(p.last_start()),
        "lastCloseTime": secs_to_str(p.last_close()),
        "status": if p.is_online() { "online" } else { "offline" },
    })
}

/// 与官方 `V2ProxyResp` 对齐（spec 按类型装块）。
fn proxy_v2_json(p: &ProxyStat) -> serde_json::Value {
    let (tin, tout) = p.today_in_out();
    let base = serde_json::json!({
        "transport": {
            "useEncryption": p.conf.get("useEncryption").and_then(|v| v.as_bool()).unwrap_or(false),
            "useCompression": p.conf.get("useCompression").and_then(|v| v.as_bool()).unwrap_or(false),
            "bandwidthLimit": "",
            "bandwidthLimitMode": "bandwidth",
        },
        "loadBalancer": { "group": "" },
    });
    let block = match p.proxy_type.as_str() {
        "tcp" | "udp" => {
            let mut b = base.as_object().cloned().unwrap_or_default();
            b.insert(
                "remotePort".into(),
                p.conf.get("remotePort").cloned().unwrap_or(serde_json::json!(0)),
            );
            serde_json::json!({ p.proxy_type.clone(): b })
        }
        "http" | "https" | "tcpmux" => {
            let key = p.proxy_type.as_str();
            let mut b = base.as_object().cloned().unwrap_or_default();
            if let Some(d) = p.conf.get("customDomains") {
                b.insert("customDomains".into(), d.clone());
            }
            if let Some(d) = p.conf.get("subdomain") {
                b.insert("subdomain".into(), d.clone());
            }
            if let Some(d) = p.conf.get("locations") {
                b.insert("locations".into(), d.clone());
            }
            if let Some(d) = p.conf.get("hostHeaderRewrite") {
                b.insert("hostHeaderRewrite".into(), d.clone());
            }
            if let Some(d) = p.conf.get("multiplexer") {
                b.insert("multiplexer".into(), d.clone());
            }
            serde_json::json!({ key: b })
        }
        other => serde_json::json!({ other: base }),
    };
    // 官方 V2ProxySpec 是带 `type` 判别字段的联合类型：
    // `{"type":"stcp","stcp":{...}}`。前端 `getActiveProxySpec` 先按
    // `spec.type` 分派，缺了 `type` 会直接落进 assertNever 抛
    // "Unsupported proxy spec"。
    let block = {
        let mut obj = block.as_object().cloned().unwrap_or_default();
        obj.insert("type".into(), serde_json::json!(p.proxy_type));
        serde_json::Value::Object(obj)
    };
    serde_json::json!({
        "name": p.name,
        "user": p.user,
        "clientID": p.client_id,
        "spec": block,
        "status": {
            "phase": if p.is_online() { "online" } else { "offline" },
            "todayTrafficIn": tin as i64,
            "todayTrafficOut": tout as i64,
            "curConns": p.cur_conns(),
            "lastStartAt": p.last_start(),
            "lastCloseAt": p.last_close(),
        },
    })
}

/// 与官方 `GetProxyTrafficResp`（v1）对齐：最近 7 天按天流量。
fn traffic_v1_json(p: &ProxyStat) -> serde_json::Value {
    let (tin, tout) = p.last_days_in_out(RESERVE_DAYS);
    serde_json::json!({
        "name": p.name,
        "trafficIn": tin.into_iter().map(|v| v as i64).collect::<Vec<_>>(),
        "trafficOut": tout.into_iter().map(|v| v as i64).collect::<Vec<_>>(),
    })
}

/// 与官方 `V2ProxyTrafficResp` 对齐：`history` 每天一个点。
fn traffic_v2_json(p: &ProxyStat) -> serde_json::Value {
    let (tin, tout) = p.last_days_in_out(RESERVE_DAYS);
    let today = crate::stats::today_day_pub();
    let history: Vec<serde_json::Value> = (0..RESERVE_DAYS)
        .map(|i| {
            let day = today - (RESERVE_DAYS as i64 - 1) + i as i64;
            serde_json::json!({
                "date": crate::stats::day_to_date(day),
                "trafficIn": tin[i] as i64,
                "trafficOut": tout[i] as i64,
            })
        })
        .collect();
    serde_json::json!({
        "name": p.name,
        "unit": "bytes",
        "granularity": "day",
        "history": history,
    })
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

fn parse_page(query: &str) -> (usize, usize) {
    let page = query_param(query, "page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let size = query_param(query, "pageSize")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    (page, size)
}

fn paginate<T>(items: Vec<T>, page: usize, size: usize) -> (Vec<T>, usize, usize, usize) {
    let total = items.len();
    let start = (page - 1) * size;
    let slice: Vec<T> = items.into_iter().skip(start).take(size).collect();
    (slice, total, page, size)
}

/// 官方面板路由。返回 `None` 表示不归本模块管（落到 v1 老路径 / api_v2）。
pub async fn route(
    registry: &Arc<Registry>,
    cfg: &Arc<ServerConfig>,
    method: &str,
    path: &str,
    query: &str,
) -> Option<Response> {
    let stats = &registry.stats;

    // ---- v1：对象本体 ----
    match (method, path) {
        ("GET", "/api/serverinfo") => {
            return Some(Response::json(
                200,
                serverinfo_json(registry, cfg).to_string(),
            ));
        }
        ("DELETE", "/api/proxies") => {
            if query_param(query, "status").as_deref() != Some("offline") {
                return Some(v1_error(400, "status only support offline"));
            }
            let (cleared, total) = stats.clear_offline();
            return Some(Response::json(
                200,
                serde_json::json!({ "code": 200, "msg": "success", "cleared": cleared, "total": total })
                    .to_string(),
            ));
        }
        ("GET", "/api/clients") => {
            let mut items: Vec<serde_json::Value> = registry
                .sessions
                .list()
                .iter()
                .map(client_info_json)
                .collect();
            items.sort_by(|a, b| {
                let ka = a["key"].as_str().unwrap_or("");
                let kb = b["key"].as_str().unwrap_or("");
                ka.cmp(kb)
            });
            return Some(Response::json(200, serde_json::Value::Array(items).to_string()));
        }
        _ => {}
    }

    // /api/proxy/{type} 与 /api/proxy/{type}/{name}
    if let Some(rest) = path.strip_prefix("/api/proxy/") {
        if rest.is_empty() {
            return None;
        }
        let (ptype, pname) = match rest.split_once('/') {
            Some((t, n)) => (t, Some(n)),
            None => (rest, None),
        };
        if method != "GET" {
            return Some(v1_error(405, "method not allowed"));
        }
        let known = [
            "tcp", "udp", "http", "https", "tcpmux", "stcp", "xtcp", "sudp",
        ];
        if !known.contains(&ptype) {
            return Some(v1_error(404, "no proxy info found"));
        }
        let mut items: Vec<Arc<ProxyStat>> = stats.by_type(ptype);
        items.sort_by(|a, b| a.name.cmp(&b.name));
        return match pname {
            None => Some(Response::json(
                200,
                serde_json::json!({ "proxies": items.iter().map(|p| proxy_stats_json(p)).collect::<Vec<_>>() })
                    .to_string(),
            )),
            Some(name) => {
                let p = items.into_iter().find(|p| p.name == name);
                match p {
                    Some(p) => Some(Response::json(
                        200,
                        proxy_stats_json(&p).to_string(),
                    )),
                    None => Some(v1_error(404, "no proxy info found")),
                }
            }
        };
    }

    // /api/proxies/{name}（v1 详情）
    if let Some(name) = path.strip_prefix("/api/proxies/") {
        if method != "GET" {
            return Some(v1_error(405, "method not allowed"));
        }
        return Some(match stats.get(name) {
            Some(p) => Response::json(200, proxy_stats_json(&p).to_string()),
            None => v1_error(404, "no proxy info found"),
        });
    }

    // /api/traffic/{name}
    if let Some(name) = path.strip_prefix("/api/traffic/") {
        if method != "GET" {
            return Some(v1_error(405, "method not allowed"));
        }
        return Some(match stats.get(name) {
            Some(p) => Response::json(200, traffic_v1_json(&p).to_string()),
            None => v1_error(404, "no proxy info found"),
        });
    }

    // ---- v2：官方信封 ----
    match (method, path) {
        ("GET", "/api/v2/system/info") => {
            let full = serverinfo_json(registry, cfg);
            return Some(v2_ok(serde_json::json!({
                "version": full["version"],
                "config": {
                    "bindPort": full["bindPort"],
                    "vhostHTTPPort": full["vhostHTTPPort"],
                    "vhostHTTPSPort": full["vhostHTTPSPort"],
                    "tcpmuxHTTPConnectPort": full["tcpmuxHTTPConnectPort"],
                    "kcpBindPort": full["kcpBindPort"],
                    "quicBindPort": full["quicBindPort"],
                    "subdomainHost": full["subdomainHost"],
                    "maxPoolCount": full["maxPoolCount"],
                    "maxPortsPerClient": full["maxPortsPerClient"],
                    "heartbeatTimeout": full["heartbeatTimeout"],
                    "allowPortsStr": full["allowPortsStr"],
                    "tlsForce": full["tlsForce"],
                },
                "status": {
                    "totalTrafficIn": full["totalTrafficIn"],
                    "totalTrafficOut": full["totalTrafficOut"],
                    "curConns": full["curConns"],
                    "clientCounts": full["clientCounts"],
                    "proxyTypeCount": full["proxyTypeCount"],
                },
            })));
        }
        ("POST", "/api/v2/system/prune") => {
            if query_param(query, "type").as_deref() != Some("offline_proxies") {
                return Some(v2_error(400, "type only support offline_proxies"));
            }
            let (cleared, total) = stats.clear_offline();
            return Some(v2_ok(serde_json::json!({
                "type": "offline_proxies",
                "cleared": cleared,
                "total": total,
            })));
        }
        ("GET", "/api/v2/clients") => {
            let status_filter = query_param(query, "status").unwrap_or_default();
            let q = query_param(query, "q").unwrap_or_default();
            let user_f = query_param(query, "user").unwrap_or_default();
            let cid_f = query_param(query, "clientID").unwrap_or_default();
            let rid_f = query_param(query, "runId").unwrap_or_default();
            let mut items: Vec<serde_json::Value> = registry
                .sessions
                .list()
                .iter()
                .filter(|m| match status_filter.as_str() {
                    "" | "all" => true,
                    "online" => m.online,
                    "offline" => !m.online,
                    _ => true,
                })
                .filter(|m| user_f.is_empty() || m.user == user_f)
                .filter(|m| cid_f.is_empty() || m.client_id == cid_f)
                .filter(|m| rid_f.is_empty() || m.run_id == rid_f)
                .filter(|m| {
                    q.is_empty()
                        || m.client_id.contains(&q)
                        || m.user.contains(&q)
                        || m.hostname.contains(&q)
                        || m.ip.contains(&q)
                })
                .map(client_info_json)
                .collect();
            items.sort_by(|a, b| {
                let ka = a["key"].as_str().unwrap_or("");
                let kb = b["key"].as_str().unwrap_or("");
                ka.cmp(kb)
            });
            let (page, size) = parse_page(query);
            let (chunk, total, page, size) = paginate(items, page, size);
            return Some(v2_ok(serde_json::json!({
                "total": total,
                "page": page,
                "pageSize": size,
                "items": chunk,
            })));
        }
        ("GET", "/api/v2/proxies") => {
            let status_filter = query_param(query, "status").unwrap_or_default();
            let q = query_param(query, "q").unwrap_or_default();
            let type_f = query_param(query, "type").unwrap_or_default();
            let user_f = query_param(query, "user").unwrap_or_default();
            let cid_f = query_param(query, "clientID").unwrap_or_default();
            let mut items: Vec<serde_json::Value> = stats
                .all()
                .iter()
                .filter(|p| match status_filter.as_str() {
                    "" | "all" => true,
                    "online" => p.is_online(),
                    "offline" => !p.is_online(),
                    _ => true,
                })
                .filter(|p| type_f.is_empty() || p.proxy_type == type_f)
                .filter(|p| user_f.is_empty() || p.user == user_f)
                .filter(|p| cid_f.is_empty() || p.client_id == cid_f)
                .filter(|p| q.is_empty() || p.name.contains(&q))
                .map(|p| proxy_v2_json(p))
                .collect();
            items.sort_by(|a, b| {
                let ka = a["name"].as_str().unwrap_or("");
                let kb = b["name"].as_str().unwrap_or("");
                ka.cmp(kb)
            });
            let (page, size) = parse_page(query);
            let (chunk, total, page, size) = paginate(items, page, size);
            return Some(v2_ok(serde_json::json!({
                "total": total,
                "page": page,
                "pageSize": size,
                "items": chunk,
            })));
        }
        _ => {}
    }

    // /api/v2/clients/{key}
    if let Some(key) = path.strip_prefix("/api/v2/clients/") {
        // ★ v0.8.5：写接口（`/clients/kick`）不归这里管，交回自有 api_v2。
        //
        // 之前这里对任何非 GET 都直接回 405 —— 而 `POST /api/v2/clients/kick`
        // 的 path 同样以 `/api/v2/clients/` 开头，于是"踢客户端"被这条
        // 前缀规则整个挡死（返回 `{"code":405,"msg":"method not allowed"}`，
        // 前端只看到"操作失败"）。返回 `None` 让后面的 api_v2 接着处理。
        if method != "GET" {
            return None;
        }
        let key = percent_decode(key);
        return Some(match registry.sessions.get(&key) {
            Some(m) => {
                let (conns, proxies) = client_conns_and_proxies(registry, &m.client_id);
                let mut body = client_info_json(&m);
                body["status"] = serde_json::json!({
                    "phase": if m.online { "online" } else { "offline" },
                    "curConns": conns,
                    "proxyCount": proxies,
                });
                v2_ok(body)
            }
            None => v2_error(404, "client not found"),
        });
    }

    // /api/v2/proxies/{name}/traffic 与 /api/v2/proxies/{name}
    if let Some(rest) = path.strip_prefix("/api/v2/proxies/") {
        // ★ v0.8.5：同上 —— `/proxies/add` 与 `/proxies/remove` 也是这个前缀，
        // 必须放行给 api_v2 的写接口，否则面板的"动态加/删代理"全都 405。
        if method != "GET" {
            return None;
        }
        let (name, traffic) = match rest.strip_suffix("/traffic") {
            Some(n) => (percent_decode(n), true),
            None => (percent_decode(rest), false),
        };
        return Some(match stats.get(&name) {
            Some(p) if traffic => v2_ok(traffic_v2_json(&p)),
            Some(p) if !traffic => v2_ok(proxy_v2_json(&p)),
            _ => v2_error(404, "no proxy info found"),
        });
    }

    None
}

/// 最小 percent 解码（`%XX`）；面板传来的 key/name 可能带特殊字符。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::ProxyStats;

    /// 官方面板 `getActiveProxySpec` 按 `spec.type` 分派；漏掉这个判别字段
    /// 会让前端抛 "Unsupported proxy spec"。回归保护。
    #[test]
    fn v2_proxy_spec_has_type_discriminator() {
        let stats = ProxyStats::default();
        stats.open(
            "win_rdp_stcp_xnj",
            "stcp",
            "xnj",
            "c1",
            serde_json::json!({"useEncryption": false, "useCompression": false}),
        );
        let p = stats.get("win_rdp_stcp_xnj").unwrap();
        let body = proxy_v2_json(&p);
        let spec = &body["spec"];
        assert_eq!(spec["type"], "stcp", "spec 必须带 type 判别字段：{spec}");
        assert!(
            spec["stcp"].is_object() && spec["stcp"]["transport"].is_object(),
            "spec 必须带同名的类型块：{spec}"
        );
        assert_eq!(spec["stcp"]["loadBalancer"]["group"], "");

        // tcp 类型的 remotePort 也应落在自己的块里。
        stats.open("rdp", "tcp", "u", "c2", serde_json::json!({"remotePort": 6010}));
        let body = proxy_v2_json(&stats.get("rdp").unwrap());
        let spec = &body["spec"];
        assert_eq!(spec["type"], "tcp");
        assert_eq!(spec["tcp"]["remotePort"], 6010);
    }

    /// v0.8.5 回归：`/api/v2/proxies/{x}` 与 `/api/v2/clients/{x}` 的前缀规则
    /// **不能吞掉写接口**。
    ///
    /// 原实现里这两个分支对任何非 GET 都返回 405，而 `/api/v2/proxies/add`、
    /// `/api/v2/proxies/remove`、`/api/v2/clients/kick` 的路径恰好都以它们为前缀
    /// ⇒ 面板的"动态加代理 / 删代理 / 踢客户端"全部失效（前端只看到 405）。
    ///
    /// 修法：非 GET 时返回 `None`，把请求交回 `api_v2`。这个测试锁住"必须返回
    /// None"，否则又会被改回 405。
    #[tokio::test]
    async fn 写接口路径不被单条查询前缀吞掉() {
        let reg = Arc::new(Registry::unlimited());
        let cfg = Arc::new(ServerConfig::default());

        for path in [
            "/api/v2/proxies/add",
            "/api/v2/proxies/remove",
            "/api/v2/clients/kick",
        ] {
            let r = route(&reg, &cfg, "POST", path, "").await;
            assert!(
                r.is_none(),
                "{path} 的 POST 必须交回 api_v2 处理，不能被 official_api 回 405"
            );
        }

        // GET 单条查询仍归这里，别改坏了。
        let r = route(&reg, &cfg, "GET", "/api/v2/proxies/add", "").await;
        assert!(r.is_some(), "GET 查询仍应由 official_api 接管");
    }
}
