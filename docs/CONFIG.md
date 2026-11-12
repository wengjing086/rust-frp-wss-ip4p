# 配置模板库：按功能模块的 frps / frpc TOML 写法

> 本文按**功能模块**给出可直接复制的配置片段。顶层与常用代理字段用官方 frp 0.71
> 的 camelCase 写法（官方配置可原样喂进来）；**NFrp 扩展字段**（限流、健康检查、
> 插件、group、指标、虚拟网络等）没有 camelCase 别名，一律用 snake_case 平铺写法，
> 或用官方 `[proxies.transport]` / `[proxies.loadBalancer]` / `[proxies.healthCheck]` 子表。
> 两种写法可混用，同名时原生字段优先；全量带注释清单见 [assets/frps.toml](../assets/frps.toml) 与
> [assets/frpc.toml](../assets/frpc.toml)，也可用 `frps --gen-config frps.toml` 生成。
>
> 片段可以叠加，但同一份配置里 `[[proxies]]` 可以有多个、`[[visitors]]` 可以有多个；
> 顶层标量键不要重复定义。配置文件支持 TOML 与老式 `frps.ini` / `frpc.ini`。

---

## 0. 约定与自检

| 项目 | 说明 |
|---|---|
| 配置格式 | TOML（推荐）或 legacy INI，自动识别 |
| 两种键名 | 官方 `bindPort` / `auth.token` / `[[proxies]]` 与 NFrp `bind_port` / `token` 都接受；同名时**原生字段优先** |
| 服务端启动 | `frps -c frps.toml` |
| 客户端启动 | `frpc -c frpc.toml` |
| 追加代理定义 | `includes = ["conf.d/*.toml"]`，只取其中的 `[[proxies]]` / `[[visitors]]` |
| 线协议 | 默认 `v1`（兼容官方 0.71.0），显式 `v2` 用 `transport.wireProtocol = "v2"` |

启动后先看日志里的 `登录成功（控制通道已加密）` 和 `代理注册成功`，再测数据面。

---

## 1. 最小可用（先跑通再叠加）

**frps.toml**

```toml
bindAddr = "0.0.0.0"
bindPort = 7000

[auth]
method = "token"
token = "换成一个至少 16 位的随机强令牌"
```

**frpc.toml**

```toml
serverAddr = "你的服务端 IP 或域名"
serverPort = 7000

[auth]
method = "token"
token = "与服务端完全一致"

[[proxies]]
name = "ssh"
type = "tcp"
localIP = "127.0.0.1"
localPort = 22
remotePort = 6022
```

> 生产环境务必再加 `allowPorts`（第 8 节）与强 token；`0.0.0.0` 只想内网访问时改回环地址。

---

## 2. 八种代理类型

**frpc.toml**（可按需取其中几段，服务端不需要逐条配置）

```toml
# 1) TCP：端口转发
[[proxies]]
name = "web"
type = "tcp"
localIP = "127.0.0.1"
localPort = 8080
remotePort = 6001

# 2) UDP：DNS / 游戏服 / WireGuard
[[proxies]]
name = "dns"
type = "udp"
localIP = "127.0.0.1"
localPort = 53
remotePort = 6053

# 3) HTTP：按域名路由，不占公网端口
[[proxies]]
name = "site"
type = "http"
localIP = "127.0.0.1"
localPort = 80
customDomains = ["web.example.com"]
locations = ["/"]              # 可选：路径前缀
httpUser = "admin"              # 可选：Basic Auth
httpPwd = "change_me"
hostHeaderRewrite = "backend.internal"

# 4) HTTPS：只嗅探 SNI 原样透传，证书由内网服务提供
[[proxies]]
name = "secure"
type = "https"
localIP = "127.0.0.1"
localPort = 443
customDomains = ["secure.example.com"]

# 5) tcpmux：HTTP CONNECT 复用，多条代理共用一个端口
[[proxies]]
name = "mux"
type = "tcpmux"
multiplexer = "httpconnect"
localIP = "127.0.0.1"
localPort = 22
customDomains = ["mux.example.com"]

# 6) stcp：私密隧道，不占公网端口，靠 secretKey + user 白名单
[[proxies]]
name = "rdp"
type = "stcp"
localIP = "127.0.0.1"
localPort = 3389
secretKey = "换一个双方一致的密钥"
allowUsers = ["*"]

# 7) sudp：私密 UDP
[[proxies]]
name = "wireguard"
type = "sudp"
localIP = "127.0.0.1"
localPort = 51820
secretKey = "换一个双方一致的密钥"

# 8) xtcp：优先 P2P 打洞，打不通回退中继
[[proxies]]
name = "p2p-rdp"
type = "xtcp"
localIP = "127.0.0.1"
localPort = 3389
secretKey = "换一个双方一致的密钥"
```

**frps.toml**（按用到的类型开启对应端口）

```toml
vhostHTTPPort = 8080             # http 类型需要
vhostHTTPSPort = 8443            # https 类型需要
tcpmuxHTTPConnectPort = 7004     # tcpmux 类型需要
p2p_port = 7002                   # xtcp 想尝试真 P2P 才需要（UDP）
subdomainHost = "example.com"    # 可选：客户端只写 subdomain 时
```

> 跨实现（Rust ↔ 官方 Go frp）时 `stcp` 四种组合全通，`xtcp` 跨实现不通，
> 详见 [README 互通矩阵](../README.md#与官方-go-版-frp-的互通性)。

---

## 3. 访客端（stcp / xtcp / sudp 的访问者）

**frpc.toml**（运行在「访问者」那台机器）

```toml
user = "alice"                   # 用于匹配提供者的 allowUsers

[[visitors]]
name = "rdp-visitor"
type = "stcp"                    # 或 "xtcp" / "sudp"
serverName = "rdp"               # 提供者注册的代理名
serverUser = "alice"             # 提供者所属客户端的 user
secretKey = "与提供者一致"
bindAddr = "127.0.0.1"
bindPort = 13389                 # 本机 13389 → 提供者的 3389
```

> `allowUsers` 比对的是访问者配置顶层的 `user`，不是 `[[visitors]]` 的 `name`。
> 跨版本 P2P 请优先用 `stcp`；`xtcp` 跨实现不互通。

---

## 4. 传输层（六选一，两端协议要一致）

**frpc.toml**（按需选一种）

```toml
[transport]
protocol = "tcp"                 # 默认；可选 tcp / tls / websocket / wss / quic / kcp
tcpMux = true                    # yamux 多路复用，减少连接数
poolCount = 1
heartbeatInterval = 10
heartbeatTimeout = 30
```

```toml
# TLS：外层加密（服务端自动识别，无需额外开端口）
[transport]
protocol = "tcp"
[transport.tls]
enable = true
serverName = "frps.example.com"
trustedCaFile = "/etc/ssl/ca.pem"    # 自有 CA 时才需要
verifyServerCert = true              # NFrp 扩展：默认不校验，见 README 安全章节
```

```toml
# WSS：TLS 内的 WebSocket，本项目 frps 同端口直连，无需 nginx
[transport]
protocol = "wss"
[transport.tls]
serverName = "frps.example.com"
```

```toml
# QUIC / KCP：需要服务端显式开 UDP 端口
[transport]
protocol = "quic"                # 或 "kcp"
```

**frps.toml**

```toml
quicBindPort = 7101              # 可选：QUIC 独立 UDP 端口（顶层）
kcpBindPort = 7001               # 可选：KCP 独立 UDP 端口（顶层）

[transport]
protocol = "quic"                # 与客户端一致；不配则默认 tcp
[transport.tls]
force = false                    # true = 强制客户端必须用 TLS
certFile = "/etc/ssl/frps.crt"   # HTTPS 回源 / WSS 需要自定义证书时
keyFile = "/etc/ssl/frps.key"
```

> `wss` 与 `tls` 都可以在**同一个 `bindPort`** 上，服务端按连接内容自动判定；
> 官方 frps 不支持 wss 直连（需前置 nginx）。KCP 是精简移植，跨实现请用 tcp/tls/websocket/quic。

---

## 5. 认证

```toml
# —— 两端都用 token（最简单）——
[auth]
method = "token"
token = "强令牌"
```

```toml
# —— OIDC：服务端 ——
[auth]
method = "oidc"
[auth.oidc]
issuer = "https://keycloak.example.com/realms/myrealm"
audience = "nfrp"
# trustedCaFile = "/etc/ssl/private-ca.pem"   # 自签 IdP 才需要
```

```toml
# —— OIDC：客户端 ——
[auth]
method = "oidc"
[auth.oidc]
clientID = "nfrp-client"
clientSecret = "从 IdP 拿到的密钥"
tokenEndpointURL = "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/token"
audience = "nfrp"
```

```toml
# —— token 放在文件 / 由外部程序提供（两端都支持）——
[auth.tokenSource]
type = "file"                    # 或 "exec"
[auth.tokenSource.file]
path = "/etc/nfrp/token"
# [auth.tokenSource.exec]
# command = "/usr/local/bin/get-token"
# args = ["--format", "raw"]
```

> `exec` 会以本进程身份执行外部程序，能改配置的人等于能执行任意命令；
> 只读文件请用 `type = "file"`。OIDC 模式下凭证是 access token 原样，不做 md5 派生。

---

## 6. 数据面加密与压缩

```toml
# frpc.toml：客户端级（对所有代理生效）
[transport]
useEncryption = true
useCompression = true
```

```toml
# frpc.toml：也可以只给某条代理开（写在 [[proxies]] 内）
[[proxies]]
name = "encrypted"
type = "tcp"
localIP = "127.0.0.1"
localPort = 3389
remotePort = 63389
transport = { useEncryption = true, useCompression = true }
```

> 加密是**逐跳**的：`visitor ↔ frps` 用 `secretKey` 派生，`frps ↔ provider` 用 `token` 派生，
> 两侧密钥不同（0.8.4 修的就是这里）。`stcp` 跨实现 + 加密已实测全通。

---

## 7. 面板、指标与本地管理

```toml
# frps.toml：内嵌官方 frp 面板 + Prometheus
enablePrometheus = true          # 顶层字段：/metrics 默认关闭，显式开启

[webServer]
addr = "0.0.0.0"
port = 7500
user = "admin"                   # 不配则面板不鉴权，且对外监听时会拒绝启动
password = "change_me"
```

```toml
# frpc.toml：本地管理界面（能看状态、能动态增删代理）
[webServer]
addr = "127.0.0.1"               # 建议只听回环
port = 7400
user = "admin"
password = "change_me"
```

```toml
# frpc.toml：面板里临时加的代理落盘，重启自动恢复
[store]
path = "/var/lib/nfrp/store.json"
```

> 服务端面板写接口（加代理 / 删代理 / 踢客户端）需要 Basic Auth，
> 且动态管理只对 NFrp frpc 生效。客户端写接口还要求 `X-Nfrp-Client: 1` 头。

---

## 8. 资源上限、端口管控与权限（frps）

```toml
# 端口白名单：不配的话，任何拿到 token 的客户端都能申请 22 / 3306
allowPorts = ["20000-40000"]
maxPortsPerClient = 20

# 连接 / 客户端上限（0 = 不限；v0.8.7 起前三项有保守默认值）
max_total_conns = 5000
max_conns_per_client = 200
max_pending_per_client = 64
max_clients = 0
max_proxies_per_client = 0

# 心跳与连接池
[transport]
heartbeatTimeout = 90
maxPoolCount = 0                 # 0 = 不限（官方默认 5）
```

```toml
# 顶层键必须写在任何表头之前，否则会被 TOML 归到上一个表里
defaultRole = ""                 # 无角色匹配时套用的角色
denyUnknown = false              # 无角色匹配时是否直接拒绝登录

# IP 白 / 黑名单：认证之前执行，deny 优先
[acl]
allow = ["10.0.0.0/8", "192.168.0.0/16"]
deny = ["10.1.2.3"]

# 角色：按顺序匹配，最具体的写前面
[[roles]]
name = "ops"
users = ["alice", "bob"]
allowProxyTypes = ["tcp", "http"]
portRange = "20000-29999"
allowManage = true
allowVisitors = true
maxProxies = 20
```

```toml
# 审计日志（默认关）
[audit]
enable = true
path = "/var/log/nfrp/audit.jsonl"
maxEntries = 1000
```

> 家宽无公网 IPv4、CDN 走 IPv6 回源时，`bindAddr` 必须写 `"::"`，否则 CDN 报 521，见 [CDN.md](CDN.md)。

---

## 9. 日志、热重载、拆分配置

```toml
# 两端通用
[log]
level = "info"                   # error / warn / info / debug / trace
to = "/var/log/nfrps.log"        # 配了就写文件（stdout 不再有日志），按天轮转
maxDays = 3
```

```toml
# frps.toml：监视配置文件 mtime（log_level 与面板密码热生效）
hot_reload = true
```

```toml
# frpc.toml：批量追加代理定义，只取 [[proxies]] / [[visitors]]
includes = ["conf.d/*.toml"]
```

```toml
# frpc.toml：断线行为
loginFailExit = true             # 首次被明确拒绝（token 错等）才退出；连不上会持续重连
reconnectInterval = 10
```

---

## 10. IP4P 与 CDN 回源

```toml
# frpc.toml：无需任何新增键，serverAddr 填发布 IP4P 记录的域名即可
serverAddr = "your.ip4p.example.com"
# serverPort 可省略：TXT / AAAA 解析出的端口会自动覆盖
dnsServer = "223.5.5.5"          # 可选：指定 DNS 服务器
```

```toml
# frps.toml：CDN 回源时采信 X-Forwarded-For（默认空 = 永不采信）
trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12"]
ipgeo = true                     # 面板显示国内归属地；可关或 NFRP_IPGEO=0
```

```toml
# frpc.toml：放在 CDN / WAF 后面（详见 CDN.md）
serverAddr = "frps.example.com"
serverPort = 443
[transport]
protocol = "wss"
[transport.tls]
serverName = "frps.example.com"
verifyServerCert = false
```

> **不需要配置 `websocket.path`**：服务端按合法 WS 升级请求判定，不比较路径；
> 403 的真因是缺 `Origin`（客户端已自动带），完整原理与排障见 [CDN.md](CDN.md)。

---

## 11. 插件（frpc 变成正向代理 / 静态站点）

```toml
[[proxies]]
name = "outbound"
type = "tcp"
remotePort = 6200
plugin = "http_proxy"            # 也支持 socks5
plugin_user = "u"
plugin_passwd = "p"

[[proxies]]
name = "static"
type = "tcp"
remotePort = 6201
plugin = "static_file"
plugin_local_path = "/var/www/html"
plugin_strip_prefix = "/prefix"

[[proxies]]
name = "tls-front"
type = "tcp"
remotePort = 6202
plugin = "https2http"            # https 进、http 出
localIP = "127.0.0.1"
localPort = 80
plugin_crt_path = "/etc/ssl/site.crt"
plugin_key_path = "/etc/ssl/site.key"
```

> 官方 9 种插件全部支持：`http_proxy` / `socks5` / `static_file` /
> `unix_domain_socket` / `http2http` / `http2https` / `https2http` / `https2https` / `tls2raw`。

---

## 12. group 负载均衡、健康检查、限流、PROXY 协议

```toml
# frpc.toml：同 group 共享一个 remotePort
[[proxies]]
name = "web-a"
type = "tcp"
localIP = "127.0.0.1"
localPort = 8080
remotePort = 6100
group = "web"
group_key = "shared_secret"
health_check_type = "http"       # tcp / http；不健康会被摘除
health_check_url = "/healthz"
health_check_interval_s = 10
health_check_timeout_s = 3
health_check_max_failed = 3
bandwidth_limit = "1MB"          # 支持 KB / KiB 后缀
bandwidth_limit_mode = "server"
proxyProtocolVersion = "v2"      # 让后端拿到真实客户端 IP，需后端支持解析
```

```toml
# frps.toml：调度策略（NFrp 扩展）
lb_policy = "roundrobin"          # 或 "leastconn"
```

```toml
# frpc.toml：附加元数据整表透传，第三方平台常靠它认隧道（不要删）
[metadatas]
token = "平台下发的隧道凭证"
```

---

## 13. VirtualNet 与 P2P 打洞

```toml
# frps.toml
vnet_port = 7003
[vnet]
network = "100.64.0.0"
subnet = "100.64.0.0/24"
mtu = 1400
```

```toml
# frpc.toml —— 顶层键写在表头之前
p2pPort = 7002                   # xtcp 打洞（两端都要配）
p2pEnable = true
xtcp_transport = "quic"          # 或 "kcp"（弱网更稳，延迟略高）

[virtualNet]
serverPort = 7003                # 必须与服务端的 vnet_port 一致
address = ""                     # 留空 + autoAssign 时由服务端分配
autoAssign = true
mtu = 1400
```

> TUN 设备：Linux 用 `/dev/net/tun`（需 root / CAP_NET_ADMIN），Windows 用
> Wintun（需管理员 + `wintun.dll`），macOS 未实现会明确报错。

---

## 14. 完整带注释模板与生成命令

| 内容 | 位置 |
|---|---|
| 服务端全量注释模板 | [assets/frps.toml](../assets/frps.toml) |
| 客户端全量注释模板 | [assets/frpc.toml](../assets/frpc.toml) |
| 命令行生成 | `frps --gen-config frps.toml` / `frpc --gen-config frpc.toml` |
| CDN 原理与部署 | [CDN.md](CDN.md) |
| WSS 传输层原理 | [WSS.md](WSS.md) |
| IP4P 记录格式 | [IP4P.md](IP4P.md) |
| 按症状排障 | [TROUBLESHOOTING.md](TROUBLESHOOTING.md) |
