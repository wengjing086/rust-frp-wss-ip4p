# 放在 CDN / WAF 后面：完整教程与原理

> 适用版本：**0.8.1 及以后**。以阿里云 ESA 为例，Cloudflare、自建 nginx / Caddy
> 反代同理。0.8.0 及更早只认固定路径 `GET /~!frp` 且不会回答普通 HTTP 请求，
> 放在 CDN 后面无法工作。
>
> 配套文档：[WSS.md](WSS.md)（传输层原理）、[IP4P.md](IP4P.md)（无公网 IP 时拨号）、
> [TROUBLESHOOTING.md](TROUBLESHOOTING.md)（按症状速查）。

---

## 0. 先记住三个结论

1. **路径无关。** 本项目 frps 判定的是「是不是一条合法的 WebSocket 升级请求」，
   **不看请求路径**。把客户端 `websocket.path` 改成任意「正常业务路径」都**不会**
   解决任何问题，服务端也不需要跟着配路径。凡是以「改个路径绕开 WAF」为思路的
   配置都是无效的，不要照做。
2. **CDN 返回 403，绝大多数是握手请求缺少 `Origin` 头**，不是路径被拦。
3. **CDN 返回 520 / 521，是源站不会说 HTTP**（健康检查拿不到响应，或根本连不上），
   需要 0.8.1+ 的 frps，以及正确的回源协议 / 端口 / 监听地址。

这三条决定了后面所有配置：真正要做的是把 **Origin 头**、**源站 HTTP 响应**、
**回源链路** 三件事弄对，而不是折腾路径。

---

## 1. 拓扑与数据走向

```text
  frpc（内网）                CDN 边缘                    frps（源站）
  ┌──────────┐             ┌──────────────┐            ┌──────────────┐
  │ wss 拨号 │──TLS(1)────▶│ 终结 TLS      │──HTTP(2)──▶│ bindPort 7000│
  │          │             │ 转发 WS 升级  │            │ 两级嗅探     │
  │ frp v1   │◀──WS 帧─────│ 转发 WS 帧    │◀──WS 帧───│ yamux 控制面 │
  └──────────┘             └──────────────┘            └──────────────┘
                                   │
                                   └── 周期性 GET / 健康检查（独立请求）
```

* **TLS(1)**：客户端到 CDN 边缘，用 CDN 的证书，域名是加速域名。
* **HTTP(2)**：CDN 到源站的回源，默认按 HTTP 明文转发到 `bindPort`；
  若选 HTTPS 回源，则源站要配证书，frps 会在 TLS 解密后再判一次 HTTP。
* 隧道里跑的仍是标准 frp v1 控制面与数据面，CDN 只做「把 WebSocket 帧搬过去」。

---

## 2. 原理：CDN 到底对源站做了什么

CDN 是 HTTP 世界的产品，它对源站有三类独立行为，任何一类没被满足都会失败：

| 行为 | 具体请求 | 期望响应 | 不满足时的现象 |
|---|---|---|---|
| 健康检查 | 周期性 `GET /`（或 `/health` 等） | 2xx | 源站被标记不健康，对外 `520` / `521` / `403` |
| 转发握手 | `GET <任意路径> HTTP/1.1` + `Upgrade: websocket` + `Connection: Upgrade` | `101 Switching Protocols` | 客户端看到 `403` / `404` / `502` |
| 分流寻址 | 按 HTTP `Host` 与 TLS `SNI` 选站点 | 命中站点配置 | `403` / `502`，请求根本不到源站 |

### 2.1 健康检查为什么会把老 frps 判死

老 frps 的 `bindPort` 只会说两种东西：明文 frp 字节流，或 TLS 字节流。
CDN 发来 `GET / HTTP/1.1` 时，它要么把这串 ASCII 当成 frp 消息类型解析失败后断开，
要么拿去当 TLS ClientHello 协商失败后断开 —— **总之不会回一个合法 HTTP 响应**。
CDN 收不到响应头，记 `error=http_response_incomplete`，对外给 `520`。

### 2.2 WebSocket 升级为什么要专门转发

升级请求本质是普通 HTTP 请求，只是多了两个头：

```http
GET /~!frp HTTP/1.1
Host: frps.example.com
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: ...
Sec-WebSocket-Version: 13
Origin: http://frps.example.com
```

CDN 必须把 `Upgrade` / `Connection` 原样透传到源站，并允许 WebSocket 长连接
（不能当普通请求缓存或超时断开）。源站必须回 `101` 并切换到 WebSocket 帧协议。
**路径只是请求行的一部分，服务端判定时不参与匹配。**

### 2.3 `Host` / `SNI` 决定请求落到哪个站点

`serverAddr` 写成 IP（或用 IP4P 域名解析成 IP）时，TLS SNI 与 HTTP Host 会变成 IP，
CDN 找不到对应站点。所以客户端必须显式给 `transport.tls.serverName = "加速域名"`，
本项目客户端的 Host 回落顺序是：

```text
websocket.host  →  tls.serverName  →  serverAddr
```

---

## 3. 原理：本项目 frps 的「两级嗅探」

`server/src/serve.rs` 的 `handle_conn()` 对新连接做三层判定，这也是 WSS 能
**和原生 frp 共用同一个 `bindPort`** 的原因：

```text
读取首字节（1 byte，带超时）
  │
  ├─ 第 1 层：明文 HTTP 判定（probe_http）
  │    请求行含 " HTTP/1." ?
  │      是 + Upgrade: websocket  → ws::accept() → 明文 WebSocket → WS 内的 frp
  │      是 + 普通 HTTP           → 健康检查回 200（/、/health*、/status），其它 404
  │      否                       → PrefixedStream 把读过的字节无损回放
  │
  ├─ 第 2 层：TLS 协商
  │    0x16 标准 ClientHello / 0x17 frp 自定义首字节 / 其它明文透传
  │
  └─ 第 3 层：在 TLS 解密后的流上 **再判一次 HTTP**（本项目新增）
       仍是 WS 升级 → WSS 升级 → frp 控制面
       不是         → 字节无损回放 → frp over TLS / 原生 TCP
```

关键点：

* **判据与路径无关**：代码只检查 `first_line.contains(" HTTP/1.")` 与
  `Upgrade: websocket` 头，没有对请求路径做任何比较。因此客户端写什么路径都行，
  CDN 做路径改写也不会破坏握手。
* **健康检查走第 1 层**，所以 HTTP 回源时 `GET /` 能拿到 `200 ok`；
  **HTTPS 回源走第 3 层**，解密后同样能拿到 200，两种回源协议都支持。
* **字节无损回放**：`probe_http` 最多读到 `\r\n\r\n`（上限 16 KiB），不匹配就
  用 `PrefixedStream` 把字节塞回流最前面。非 HTTP 客户端最多多读 4 个字节，
  行为与改造前完全一致 —— 这也是「同端口同时接 tcp/tls/ws/wss」不会互相干扰的原因。
* 官方 frps 只在 **TLS 之前**按明文前缀嗅探，`wss` 的首字节是 `0x16`，会被当成
  普通 frp over TLS 丢给 yamux，所以官方方案必须前置 nginx 终结 TLS；
  本项目多做的第 3 层省掉了这一层。

> 结论：服务端**没有任何与路径相关的配置项**，`frps.toml` 里也不需要为 CDN 加东西。

---

## 4. 原理：403 的真因是 `Origin`，不是路径

实测（2026-11，阿里云 ESA，**WAF 规则全部放行**）：

```text
GET /~!frp  无 Origin                →  403 Forbidden        Server: ESA
GET /~!frp  +Origin                  →  101 Switching Protocols   ← 通了
GET /~!frp  +User-Agent（无 Origin） →  403 Forbidden
GET /~!frp  +Origin +UA +Protocol    →  101 Switching Protocols
```

云 WAF 普遍把「带 `Upgrade: websocket` 却不带 `Origin`」当作非浏览器发起的
自动化流量直接 403，响应体为空，**在控制台放行任何路径都无效**，因为请求根本
没走到路径规则那一步。

官方 Go 版 frpc 能过，是因为 `golang.org/x/net/websocket` 的 `Dial()` 必然会写
`Origin: http://<host>`。0.8.1 起本项目客户端自动携带该头（值按第 2.3 节的回落
顺序取，与 `Host` 同源），无需配置。

> **判断口诀**：同一条链路，Go 版 frpc 通、其它客户端 403 ⇒ 先怀疑缺 `Origin`。
>
> **反面结论**：既然 403 与路径无关，「把路径换成普通业务路径」对 403 没有作用；
> 若带 `Origin` 仍 403，去查 WAF 自定义规则、地域封禁、速率限制，而不是改路径。

---

## 5. 从零部署

### 5.1 服务端（frps）

```toml
# frps.toml —— 无需任何 CDN 专用配置
bindAddr = "::"          # 家宽无公网 IPv4 时必须是 "::"，见第 8 节
bindPort = 7000          # CDN 的「源站端口」填这个
auth.method = "token"
auth.token = "换成一个足够随机的强令牌"

# 可选：让面板显示真实客户端 IP（填 CDN 的回源网段）
trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12"]
```

启动后**先在源站本机自检**（必须返回 200）：

```bash
curl -i http://127.0.0.1:7000/
# HTTP/1.1 200 OK
# Content-Type: text/plain; charset=utf-8
# ok
```

### 5.2 CDN 控制台（以阿里云 ESA 为例）

| 项目 | 建议值 | 为什么 |
|---|---|---|
| 加速域名 | `frps.example.com` | 与客户端 `tls.serverName` 一致 |
| 源站地址 | 源站公网 IP 或域名 | 家宽无 IPv4 时填 IPv6 地址 |
| 源站端口 | `7000` | 与 `bindPort` 一致 |
| 回源协议 | **HTTP**（源站是明文 frps） | 选 HTTPS 需要源站配证书，见 5.4 |
| 回源 Host / SNI | 加速域名 | 与客户端 `Host` / `serverName` 同源 |
| WebSocket | **开启** | 关掉会拿不到 101 |
| 健康检查 | `GET /`，期望 2xx | frps 对该路径回 200 |
| 缓存 | 全部关闭 / 绕过 | 升级请求被缓存层拦下会失败 |
| 强制 HTTPS 跳转 | 关闭（或确保不重定向 WS 升级） | 301/302 会打断握手 |
| WAF 自定义规则 | 放行 WS 升级请求 | 见第 4 节；**不要靠改路径绕过** |

### 5.3 客户端（frpc）：最小可用配置

```toml
serverAddr = "frps.example.com"   # 加速域名；写成 IP 时必须配下面的 serverName
serverPort = 443                  # CDN 对外端口（HTTPS）
auth.method = "token"
auth.token = "和服务端一致的强令牌"

[transport]
protocol = "wss"

[transport.tls]
serverName = "frps.example.com"   # TLS SNI + HTTP Host + Origin 都取它
verifyServerCert = false          # 用 CDN 的正规证书可以设 true；自签才需 false
```

说明：

* **不要写 `[transport.websocket] path`**：默认路径 `/~!frp` 在带 `Origin` 的前提下
  实测就能拿到 101；服务端不校验路径，改它不产生任何效果。
* `protocol = "wss"` 已包含 TLS，不用再写 `tls.enable = true`。
  想让 CDN 看不到 frp 内容，可再加 `tls.enable = true`（默认即为 true），
  这是 WebSocket 帧之内的第二层加密，与 CDN 终结的外层 TLS 互不冲突。
* `Origin` 由客户端自动补，值按 `websocket.host → tls.serverName → serverAddr` 回落，
  与 `Host` 同源。

### 5.4 若坚持 HTTPS 回源

源站 `frps.toml` 增加证书，CDN 侧「回源协议」改 HTTPS：

```toml
[transport.tls]
certFile = "/etc/ssl/frps.example.com.pem"
keyFile  = "/etc/ssl/frps.example.com.key"
```

frps 会在第 2 层做 TLS 协商，再在第 3 层对解密后的流判 HTTP，所以健康检查与
WSS 升级都能工作。要注意 CDN 是否校验源站证书（自签需在 CDN 侧关闭校验）。

### 5.5 验证顺序（不要跳步）

```bash
# ① 源站自检：直连 frps 端口，普通 HTTP 必须 200
curl -i http://<源站IP>:7000/

# ② 边缘自检：向 CDN 发一条 WS 升级请求，期望 101
python _interop/probe_cdn.py frps.example.com 443 /~!frp /

# ③ 客户端
./frpc -c frpc.toml      # 出现 login to server success 即成功
```

* ① 不通过 ⇒ 源站版本不对（不是 0.8.1+）或端口/协议填错。
* ② 不通过 ⇒ 问题在 CDN 边缘（403 查 Origin/WAF，520 查回源）。
* ③ 不通过但 ② 是 101 ⇒ 只剩 token 不一致或 `tls.enable` 组合问题，
  看服务端日志里的认证结果与 `WebSocket（明文）/ WSS（TLS 内 WebSocket）控制连接已建立`。

---

## 6. 排障矩阵

| 现象 | 判据 | 原因 | 处理 |
|---|---|---|---|
| `403` + `Server: ESA` / `cloudflare` | 响应来自边缘，源站日志无记录 | 多数是握手缺 `Origin`；其次 WAF 自定义规则 / 地域封禁 | 确认客户端是 0.8.1+；用 `probe_cdn.py` 看响应头；查 WAF 规则。**不要改路径** |
| `404` | 源站回了 HTTP 但路径未命中健康检查白名单 | 请求被判为普通 HTTP（通常是 `Upgrade` 头被反代剥掉） | 检查 CDN / nginx 是否透传 `Upgrade` 与 `Connection` |
| `520` + `error=http_response_incomplete` | 到了源站但没拿到合法 HTTP 响应 | 源站是旧版 frps，或回源协议填成 HTTPS 而源站是明文 | 升到 0.8.1+；回源协议选 HTTP；用 `curl -i` 直连源站验证 |
| `521 Origin Down` | 连不上源站端口 | 源站没监听 / 防火墙 / **只监听 IPv4 而 CDN 走 IPv6 回源** | 看第 8 节，`bindAddr = "::"` |
| `101` 但 frpc 登录失败 | 握手成功，认证阶段失败 | token 不一致，或内外层 TLS 组合不对 | 服务端日志会明确写认证失败；对齐 `auth.token` |
| 能用但面板 IP 全是 CDN 地址 | 面板显示回源 IP | 没配 `trusted_proxies` | 见第 7 节 |
| 连接偶发断开 | 长时间空闲后被断 | CDN 空闲超时 / 缓存层介入 | 关缓存、调长 WebSocket 空闲超时、配 `heartbeatInterval` |

`_interop/probe_cdn.py` 会把每条路径的响应头原样打印，是区分「边缘拒」与「源站挂」
最快的工具：看 `Server` 头和 `proxy-status`，再对照服务端日志有没有这条请求。

---

## 7. 真实客户端 IP 与安全

```toml
# frps.toml —— 只采信这些地址发来的 X-Forwarded-For
trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12"]
```

* 默认空列表 = **永远不采信**。否则任何人都能伪造 `X-Forwarded-For` 自称
  `127.0.0.1`，直接绕过 IP 黑名单。
* 填 CDN 的回源网段（阿里云 ESA 的地址段见其文档；自建 nginx 同机填 `127.0.0.1/32`）。
* CDN 只负责把流量送到源站，**认证仍然是 frp 的 token**：
  用强 token、能开 `tls.force = true` 就开、开面板必须设 `webServer.user` / `password`。

---

## 8. 家宽无公网 IPv4：`bindAddr` 必须写 `::`

真实案例（2026-11）：源站是家宽 + 移动宽带，没有公网 IPv4，CDN 回源走
**IPv6 直连源站 7000 端口**（ESA 边缘节点连 `[IPv6]:7000`）。Go 版 frps 的
`net.Listen` 默认双栈（`[::]:7000` 同时收 IPv4/IPv6），而 Rust 版默认
`bindAddr = "0.0.0.0"` **只监听 IPv4** —— CDN 回源全部 `connection_refused`，
面板报 `521 Origin Down`，但局域网内直连一切正常，极具迷惑性。

判别：看服务端启动日志，若只写 `监听 0.0.0.0:7000` 即中招；Go 版时代能看到
`read tcp [2409:…]:7000->[240d:c010:…]:xxxx`（`240d:c010` 是 ESA 回源段）。

修复：

```toml
bindAddr = "::"      # Linux 默认双栈，IPv4 也能进
```

iStoreOS / OpenWrt 的 luci-app-frps：UCI 里设 `frps.main.bindAddr='::'` 后重启服务
即可（生成器白名单里有 `bindAddr`，会写进生成的 TOML）。

---

## 9. 常见误区

| 误区 | 事实 |
|---|---|
| 把 `websocket.path` 改成「正常业务路径」能绕开 WAF | **无效**。403 是缺 `Origin`，服务端也不校验路径。0.8.1 起客户端已自动带该头 |
| 要让服务端也认客户端的路径 | 不需要。判定只看请求行与 `Upgrade` 头，服务端没有路径配置项 |
| 520 一定要修好才能用 | 若 CDN 因健康检查判源站不健康，可能整站不可用，建议还是升到 0.8.1+ 让它回 200 |
| 开了 CDN 就不用 token 了 | CDN 不做 frp 认证，token 仍是唯一边界 |
| CDN 证书 = 端到端加密 | CDN 会终结外层 TLS；`tls.enable = true` 提供的是 WS 帧内的第二层加密 |
| `serverAddr` 写 IP 更稳妥 | 写 IP 会导致 SNI/Host 变成 IP，CDN 找不到站点，必须补 `tls.serverName` |
