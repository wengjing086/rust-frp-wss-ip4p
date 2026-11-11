# 放在 CDN / WAF 后面（阿里云 ESA、Cloudflare 等）

> 适用版本：**0.8.1 及以后**。0.8.0 及更早只认 `GET /~!frp`，放在 CDN 后面
> 要么被 WAF 拦成 403，要么因为源站不答 HTTP 被 CDN 判成 520，无法直连。

## 一、先分清是「谁」在拒绝

看客户端日志只有一句 `服务端返回 403（期望 101）` 是不够的 —— 403 可能来自
WAF，也可能来自源站。用 `_interop/probe_cdn.py` 直接发一条 WebSocket 升级请求，
把响应头原样打出来：

```bash
python _interop/probe_cdn.py frps.example.com 443 /~!frp /ws /
```

两种典型结果：

| 现象 | 含义 | 处理 |
|---|---|---|
| `403` + `Server: ESA` / `Server: cloudflare` | **CDN 边缘拦的**，请求没到源站 | 换路径（见第二节） |
| `520` + `proxy-status: esa; error=http_response_incomplete` | 到了源站，但源站没回一个合法 HTTP 响应 | 源站换成 0.8.1+ 的 frps（见第三节） |
| `101 Switching Protocols` | 链路已通 | 只剩 frpc 的 `path` 要跟服务端一致 |

### 1.1 最隐蔽的一种 403：**少了 `Origin` 头**（0.8.1 修的就是这个）

实测（2026-11，阿里云 ESA，**WAF 规则全部放行**的前提下）：

```text
GET /~!frp  无 Origin              →  HTTP/1.1 403 Forbidden  Server: ESA
GET /~!frp  +Origin                →  HTTP/1.1 101 Switching Protocols   ← 通了
GET /~!frp  +User-Agent（无 Origin）→  HTTP/1.1 403 Forbidden
GET /~!frp  +Origin +UA +Protocol  →  HTTP/1.1 101 Switching Protocols
```

结论很反直觉：**拦的是"没有 `Origin` 的 WebSocket 升级"，不是路径**。
云 WAF 普遍把「带 `Upgrade: websocket` 却不带 `Origin`」当作非浏览器发起的
自动化流量，直接 403，且响应体为空 —— 你在控制台怎么放行路径都没用，因为
它根本没走到路径规则那一步。

官方 Go 版 frpc 能过，就是因为它底层用 `golang.org/x/net/websocket`，
`Dial()` 必然会写一条 `Origin: http://<host>`。所以：

> **判断方法**：同一条链路，Go 版 frpc 通、非 Go 客户端 403 ⇒ 九成是缺 `Origin`。
> 0.8.1 起客户端已自动携带（值取 `websocket.host` → `tls.serverName` → `serverAddr`
> 的回落结果），不必配置。

### 1.2 另一种 403 / 520：路径与回源

实测同一域名：

```text
GET /~!frp  →  HTTP/1.1 403 Forbidden     （无 Origin 时）
GET /ws     →  HTTP/1.1 520 Origin Error  proxy-status: esa; error=http_response_incomplete
GET /       →  HTTP/1.1 520 Origin Error  （同上）
```

520 说明**回源那一跳**没走通 —— 源站（老的 frps）只会说 TLS / frp 协议，
不认普通 HTTP，CDN 等不到响应就报 `http_response_incomplete`。
注意：**520 只影响健康检查和换路径，不影响默认路径的 WSS 登录** —— 实测
`/~!frp` 带 Origin 拿到 101 后，控制连接与 RDP 数据面都能正常跑。

## 二、客户端：路径不是必须改的

CDN 后面的路径由客户端决定，服务端从 0.8.1 起**不再限制路径**：

```toml
serverAddr = "frps.example.com"   # 域名或 IP 都行，反正是 CDN 的地址
serverPort = 443
auth.method = "token"
auth.token = "******"

[transport]
protocol = "wss"
heartbeatInterval = 10
heartbeatTimeout = 30

[transport.tls]
# ★ 这个域名是给 CDN 用的：TLS 的 SNI 和 HTTP 的 Host 都用它。
#   serverAddr 填成 IP（或 IP4P 域名）时，没有它 CDN 会按 Host 找不到站点，回 403/502。
serverName = "frps.example.com"
verifyServerCert = false        # 用 CDN 的证书就算 true 也能过；自签才需要 false

[transport.websocket]
# 可选。默认 `/~!frp` 在 0.8.1 + 带 Origin 的前提下通常已经能用（实测 101）。
# 只有当 WAF **确实按路径**拦了你，才改成普通路径（/ws、/api/v1/stream 都行）。
# ★ 改路径要求服务端也认这个路径：本项目 frps 认任意路径；
#   官方 Go frps 只认 `/~!frp`（需打 frp-wss-ipgeo 补丁后才认任意路径）。
# path = "/ws"
# host 留空 = 自动用上面的 serverName
```

要点：

* **Host 的回落顺序**是 `websocket.host` → `tls.serverName` → `serverAddr`。
  CDN 按 Host 分流，`serverAddr` 是 IP 时必须靠 `serverName` 补上域名。
* `protocol = "wss"` 已经包含 TLS，不用再写 `tls.enable = true`；
  想再加一层端到端加密（CDN 看不到 frp 内容）就保持 `tls.enable = true`（默认）。
* `~!` 之外的路径都行，但**两边不必配置一致** —— 服务端认"合法 WS 升级请求"，
  不认具体路径。所以 CDN 上做路径改写（比如把 `/ws` 改写成 `/`）也不会坏。

## 三、服务端：0.8.1 起才会"说 HTTP"

CDN 是 HTTP 世界的产品，回源时它做两件事：

1. **健康检查**：周期性发 `GET /`，期望 2xx/3xx，否则把源站标记为不健康（对外 520/403）；
2. **转发**：把客户端的 WS 升级请求当普通 HTTP 请求转给源站。

老 frps 两件事都答不上来（它只会说 TLS 和 frp 协议），所以必然 520。
0.8.1 的 `probe_http()` 改成：

```text
首字节 → 是不是 HTTP？（方法前缀 + " HTTP/1." 请求行）
  ├─ 是 + Upgrade: websocket  → ws::accept() → WS 内的 frp（路径不限）
  ├─ 是 + 普通请求            → 回 200（/、/health*、/status）或 404，然后关连接
  └─ 不是                     → 字节无损回放 → 原来的 TLS / frp 分流（完全不变）
```

TLS 解密之后**再判一次**，所以 CDN 用 HTTP 回源还是 HTTPS 回源都能工作。

服务端不需要任何新配置，照常用 `bindPort`：

```toml
bind_addr = "0.0.0.0"
bind_port = 7000          # ← ESA 的"源站端口"填这个
token = "******"
```

## 四、CDN 控制台要改的几项（以阿里云 ESA 为例）

1. **源站协议 = HTTP**（不是 HTTPS）。源站是 frps 的明文端口，
   CDN 发来的 `GET /ws` 才能被 `probe_http()` 认出来。
   如果坚持 HTTPS 回源，就要给 frps 配 `tls_cert_file` / `tls_key_file`，
   此时 CDN 与源站之间是加密的，0.8.1 同样支持（TLS 后再判 HTTP）。
2. **源站端口 = frps 的 `bind_port`**（示例 7000）。
3. **开启 WebSocket**（ESA 默认开启；若站点规则里关了要打开）。
4. **加速域名不要开"强制 HTTPS 跳转"之外的路径改写**；如需改写，
   客户端 `path` 与服务端已解耦，改写不影响握手。
5. 如果套餐带 **WAF 自定义规则**，把 `/ws` 加进放行名单更稳。

## 五、验证链路

```bash
# 1. 源站自检：直连 frps 端口发一条健康检查，必须 200
curl -i http://<源站IP>:7000/

# 2. CDN 自检：期望 101（或至少不再是 403/520）
python _interop/probe_cdn.py frps.example.com 443 /ws

# 3. 客户端
./frpc -c frpc.toml        # 日志出现 "login to server success" 即成功
```

如果第 2 步是 101、第 3 步还失败，几乎只剩两种原因：

* 客户端 `path` 与服务端/改写规则对不上 → 看服务端日志里有没有
  `WebSocket（明文）控制连接已建立` / `WSS（TLS 内 WebSocket）控制连接已建立`；
* token 不一致 → 服务端日志会明确写认证失败。

## 六、拿不到真实客户端 IP 时

CDN 回源时对端是 CDN 的地址。要让面板 / IP 白黑名单看到**真实客户端 IP**：

```toml
# frps.toml —— 只采信这些地址发来的 X-Forwarded-For
trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12"]
```

* 默认空列表 = **永远不采信**（否则任何人都能伪造 `X-Forwarded-For` 自称
  `127.0.0.1`，直接绕过 IP 黑名单）；
* 填 CDN 的回源网段（阿里云 ESA 的回源地址段见其文档；自建 nginx 同机填
  `127.0.0.1/32`）。

## 七、安全提醒

CDN 只负责把流量送到源站，**认证仍然是 frp 的 token**。所以：

* 不要把 `token` 设成弱口令；
* 能开 `tls_force = true` 就开（要求客户端 `tls.enable = true`）；
* 开了面板就一定要设 `dashboard_user` / `dashboard_pwd`。

另外，CDN 侧若开了"缓存"，请把 `/ws` 这类路径设为**不缓存**，否则升级请求
可能被缓存层挡下来。

## 八、家宽无公网 IPv4：bindAddr 必须写 "::"

真实案例（2026-11）：源站是家宽 + 移动宽带，没有公网 IPv4，CDN 回源走
**IPv6 直连源站 7000 端口**（ESA 边缘节点连 `[IPv6]:7000`）。Go 版 frps 的
`net.Listen` 默认双栈（`[::]:7000` 同时收 IPv4/IPv6），而 Rust 版默认
`bindAddr = "0.0.0.0"` **只监听 IPv4**——于是 CDN 回源全部
`connection_refused`，面板报 `521 Origin Down`，但局域网内直连一切正常，
极具迷惑性。

判别方法：看服务端日志，Go 版时代能看到
`read tcp [2409:…]:7000->[240d:c010:…]:xxxx`（240d:c010 是 ESA 回源）；
Rust 版启动日志若只写 `监听 0.0.0.0:7000` 即中招。

修复：frps.toml 里把监听地址改成 IPv6 通配（Linux 默认双栈，IPv4 也能进）：

```toml
bindAddr = "::"
```

iStoreOS / OpenWrt 的 luci-app-frps：UCI 里 `frps.main.bindAddr='::'` 后
重启服务即可（生成器白名单里有 `bindAddr`，会写进生成的 TOML）。
