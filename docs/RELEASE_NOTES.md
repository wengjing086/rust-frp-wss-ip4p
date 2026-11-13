## v0.8.9 — 对齐官方 0.71.1/0.72.0 中的重连类修复

**线协议没变，0.8.8 / 0.8.9 可以混用。**

### 先说版本基线为什么不动

官方 frp 已发到 v0.72.0，但 **v0.71.1 与 v0.72.0 都没有改动线协议或配置格式**，
只有功能与 bug 修复。本项目的 `FRP_WIRE_VERSION` 仍是 `0.71.0`，它出现在
`Login.version` / `LoginResp.version`、`frpc -v` 与面板 `/api/v2/version`，
第三方控制面板会解析它决定下发 INI 还是 TOML。把它刷成 `0.72.0` 等于对外
宣称「已用官方 0.72.0 二进制实测互通」，而 README 的互通矩阵实测基准是
0.71.0 —— 那是虚假声明，所以刻意保持不动。

逐条核对两版 release notes 后的结论：健康检查连续失败计数、VirtualNet
重连路由与退避、CONNECT 早数据丢失、限流下工作连接残留、控制连接写失败
即重连，本项目此前均已独立修复并带回归测试；Go yamux 忙循环、log 前缀
数据竞争、NTLM 代理 challenge、DST 日流量统计属 Go 特有或本实现免疫，不适用。
本版只移植下面三条仍有价值的修复。三个新功能（系统 CA 验证、`frpc reload`
证书热刷新、独立 `metricsServer`）未纳入本版。

### 1. 控制连接写加超时（WSS/CDN 反压不再挂死重连）

WSS / CDN 链路上对端不读时，底层 `poll_write` 会一直返回 `Pending`。此前
`send_msg` 没有写超时，会**永久**占住会话主循环 —— 心跳发不出去、重连被
无限推迟，表现之一就是「刚连上很卡、流量大了才流畅」以及断线后迟迟不恢复。
这是官方 v0.72.0 修的「WebSocket 写阻塞拖延重连」的同款症状。

- `common/src/frp/conn.rs`：整段写入（含 flush）加 10 秒 `WRITE_TIMEOUT`。
  `write_frame` 同走 `write_bytes`，一处覆盖全部写路径（含 NLink 扩展帧、
  v2 握手帧）；`shutdown()` 的 flush 同样加超时，服务端拒绝 visitor 时不会被卡住。
- `client/src/web.rs`：管理界面的写失败不再被吞成业务回执。新增
  `LiveError::{Business, Fatal}`，写超时 / 读错 / 对端关闭归为 Fatal，经 `?`
  冒泡出 `run_session` 立即触发重连；同名、被服务端拒绝等仍是业务失败。

### 2. 重连名单取进程级活表

注册循环与 `ResumeCommit` 对账此前都用启动时的 `cfg.proxies` 快照，于是运行期
通过 `[webServer]` 或服务端面板新增的代理，重连后不会重新上报、还会被服务端
判为 stale 回收；反过来，运行期**删掉**的代理重连后又会被旧快照注册回来。
这是官方 v0.71.1 / v0.72.0 修的「重连期间配置变更丢失」的同族形态。

- `client/src/main.rs`：抽出 `registration_list(&ProxyTable, &cfg)`，注册与对账
  共用同一份活表快照，并补回归测试锁住「活表与 cfg 快照故意不一致」的场景。

### 3. frps 会话主动关闭句柄

官方 v0.72.0 要求立即释放被替换/失败控制连接的复用会话。本项目的工作连接
本来就随 `ClientGuard::drop` 立即释放，但 yamux 会话对象只能等
`SESSION_IDLE_TIMEOUT`（30 秒）看门狗。

- `common/src/frp/mux.rs`：`MuxAcceptor` 加 oneshot 关闭句柄，实现 `Drop`
  与 `close()`；驱动 `select!` 加一支 break。accept 循环一结束（正常结束 /
  出错 / 被踢），会话与底层 TCP 立刻释放。

### 验证

`cargo test --workspace`：**1075 passed / 0 failed**，无编译警告。新增 5 条
回归测试：写永久阻塞按超时报错、优雅关闭不被写阻塞挂住、重连名单活表语义、
主动关闭立刻结束会话、acceptor drop 立刻释放底层连接。

---

## v0.8.8 — frps 主端口关闭 Nagle，交互式流量不再攒包

**线协议没变，0.8.7 / 0.8.8 可以混用。只有服务端有改动，只升 frps 即可。**

### 唯一的改动

frps 主端口（`bindPort`）accept 之后补上 `TCP_NODELAY`（`server/src/serve.rs:880`）。

Go 的 `net.TCPConn` 在 `newTCPConn` 里**默认就 `SetNoDelay(true)`**，所以官方 frps
从来不需要显式写这一行；tokio 则沿用内核默认，而 Linux 上 Nagle 是**开**的。
本项目此前补过 frpc 侧，漏了 frps 侧 —— 与官方行为不一致。

### 为什么这条影响体验

这条连接是 frps → frpc 的**发送端**。stcp / xtcp 中继时，下面这些流量都从它出去：

- RDP 的输入事件（鼠标移动、按键，只有几个字节）
- SSH 的击键与回显
- 各类控制消息、心跳

Nagle 会把小于 MSS 的包攒着等对端 ACK，再叠加 Windows 默认 200 ms 的延迟确认，
最坏情况每个小包被拖几十到两百毫秒。典型表现不是"整体慢"，而是**画面还行，
但打字一顿一顿、鼠标发飘**。大流量的画面数据本就是满 MSS，不受影响。

### 回归

`clippy -D warnings` 干净，`nfrp-server` 378 项测试全过。改动只涉及一个套接字选项，
设置失败也不影响转发（`set_nodelay(true).ok()`）。

### 升级

**只替换 frps 二进制并重启**。frpc 侧从最初版本起就已有这两处 `nodelay`
（`client/src/main.rs:1206` 连 frps 的主连接、`2219` 连本地服务），0.8.7 的 frpc
已包含，不需要跟着升。

---

## v0.8.7 — 默认资源上限、密钥派生缓存、发布链路整理

**线协议没变，0.8.6 / 0.8.7 可以混用，frpc 不必升级。**

### 需要留意的行为变化

三道转发连接上限默认从 `0（不限）` 改为 `5000 / 200 / 64`
（`max_total_conns` / `max_conns_per_client` / `max_pending_per_client`）。
以前认证通过的客户端不停建连就能把 task 数推到 OOM。客户端特别多时按容量调大，
想恢复旧行为把三项显式写成 `0`。配置文件不写这三项即生效。

### 其它改动

- 密钥派生按 token 缓存（原来每条连接重算 64 轮 HMAC-SHA1），并合并重复的
  `proxy_flags()` 查询；官方向量测试保留，跨实现加密用例全通。
- 修掉一个偶发失败的单测竞态（全局打洞表被另一个测试清空）。
- 统一启动期关于 timestamp 新鲜性窗口的过时告警文案。
- 发布说明与脚本移入版本控制、去掉硬编码路径与 TAG，必选产物缺失不再静默跳过，
  `SHA256SUMS.txt` 按实际产物自动生成；下载表不再列未构建的 Windows ARM64。
- README 补「生产部署建议配置」与集中的已知限制表；误报申诉材料改为引用
  仓库里确实存在的构建配置。
- CI 配置随源码入库在 `ci/`；因 PAT 缺 `workflow` scope，GitHub Actions
  **尚未启用**，启用步骤见 `ci/README.md`。

### 回归与升级

单测 1070 全过，`fmt --check` / `clippy -D warnings` 干净；真实二进制 e2e
（含 Rust ↔ 官方 Go frp 45 项、8 MiB 与 3×4 MiB 并发 wss）全绿。
服务端单独升级即可。

---

## v0.8.6 — QUIC 与官方 frp 互通、xtcp 首次打洞从 10 秒降到 0.5 秒

这一版来自一轮**跨实现**测试：拿官方 Go 版 frp 0.71.0 的真实二进制与本项目双向混搭，
逐项验证 8 种代理类型、6 种传输层、数据面加密/压缩，以及 stcp / xtcp 的四种
"谁做 provider"组合，共 41 项，全部通过。6 项属于上游或架构层面的已知不支持，
单独列在最后，也写进了 README。

### 修复 1：QUIC 连不上官方 frp（ALPN 用错标识）

现象：`transport.protocol = "quic"` 连官方 frps 永远失败，客户端只有一行
`QUIC 握手失败：aborted by peer: the cryptographic handshake failed: error 120`，
服务端什么都没记。反方向（官方 frpc 用 QUIC 连本项目 frps）同样失败。

根因：QUIC 的 ALPN 用的是私有值 `nfrp-frp`，官方两端写死 `frp`
（`client/connector.go:124`、`server/service.go:268` 的 `NextProtos`）。
ALPN 不匹配时对端直接中止 TLS 握手，报出来就是 error 120，日志里没有任何线索。

修复：`ALPN` 改为官方值 `frp`，两端同时接受旧的 `nfrp-frp`（`alpn_family()`），
所以老版本 nfrp 客户端/服务端照旧能连，官方也能连。新增单测
`alpn_matches_official_frp` 把这两个标识钉住。

### 修复 2：xtcp 首次打洞固定要等 10 秒

现象：官方 frpc 做 xtcp 时，第一次连接固定 10.5 秒才通，第二次起是毫秒级。
日志里 visitor 5 秒 `context deadline exceeded`、服务端 10 秒
`等待 provider 的 NatHoleClient 超时`，靠下一次重试才打通。

根因：frps 把 `NatHoleSid` 写进给 provider 的工作连接后**直接 drop**。
这条连接在 tcp_mux 下是 yamux 流，drop 以 RST 收场，对端还没读走的消息
被内核丢掉 —— 官方 frpc 侧只留一句 `xtcp read from workConn error: connection reset`，
既不回 `NatHoleClient`，整条配对只能等超时。
`common/src/frp/conn.rs` 里 `shutdown()` 的注释早写明这个坑，只是这处没用上。

修复：改成优雅关闭（先 flush 再 shutdown）。实测首次打洞 10.5s → 0.51s。

### 改进 3：xtcp 跨官方 frps 时不再让人对着 connection reset 猜

本项目的 xtcp 走自家 `p2pPort` 的 HELLO 牵线，官方 frps 上没有这个服务，
也不给 xtcp 做中继；官方 frpc 的 xtcp provider 又只认工作连接上的 `NatHoleSid`。
两边凑一起时过去只在用户侧显示 `connection reset`。

现在客户端会在两个位置明确告警：启动时（有 xtcp 代理但没配 `p2pPort`）、
以及 visitor 回退中继时，都直接说明"官方 frps 不提供 xtcp 中继，请改用 stcp"。

### 文档：互通矩阵按实测重写

之前那张表有一个格子是错的：`本项目 frpc --wss--> 官方 frps` 被标成可用，
实际不通 —— 我们的 wss 是 TLS 在外层、WS 在内层，而官方 frps 在 TLS **之前**
按明文 `GET /~!frp` 嗅探 WebSocket，两者对不上；官方自己连自己也是这个结果。

现在拆成"代理类型"和"传输层"两张表，补上 quic / kcp 两列，并把三处反直觉的
格子（wss、quic、kcp）逐条解释了原因。

### 测试脚本

`_interop/compat_go.py` 留在仓库里可直接复跑：
`python _interop/compat_go.py`（`--only A,B` 可挑阶段）。
`_interop/stun_min.py` 是给 xtcp 测试用的最小 STUN —— 官方客户端打洞前必须先向
STUN 拿到自己的映射地址，离线机器没有外网 STUN 时会报
`wait response from stun server timeout`，看起来像兼容性坏了，其实只是环境问题。

### 已知不支持（不是 bug，写清楚免得再踩）

| 项 | 原因 |
|---|---|
| `kcp` 跨实现 | 我们的 KCP 是 ikcp / kcp-go 的精简移植，没有 FEC 与会话握手，字节层面对不上 |
| `wss` 直连官方 frps | 官方在 TLS 之前嗅探，需要前置 nginx 终结 TLS；官方自己也不支持 |
| `xtcp` 跨实现 | 官方 provider 只处理 NatHoleSid、不接中继；官方 visitor 没有中继回退 |

跨官方要加密点对点，用 `stcp` —— 四种组合 × 8 MiB 全部通过。

## v0.8.5 — 面板代理状态不再“僵尸”、动态管理接口修复、frps.ini 兼容

这一版来自一轮**真实二进制的端到端测试**：不是跑单元测试，而是把 frps / frpc
真正启动起来，逐项验证 8 种代理类型、6 种传输层、面板全部接口、以及安全管控。
共 57 项端到端用例，全部通过。测出来的问题都不是“新功能不能跑”，而是
**一直在暗处、平时看不出来**的那类。

### 修复 1：客户端下线后，代理永远显示“在线”

面板上代理一旦注册就永远是绿色，客户端关了、被杀了、断网了都不变。连
“清理离线代理”也清不掉它（那个功能按 `online == false` 筛，永远筛不到）。
线上真实表现：一条早就退出的测试代理在面板上挂了几个小时。

根因：`ProxyStats::mark_closed()` 存在，但**整个代码库没有任何调用点**。
`CloseProxy` 那条路径调了另一处逻辑，而客户端被强杀 / 拔网线时根本发不出这条
消息，于是 `open()` 置的 `online = true` 再也没人复位。

修复：客户端会话整体回收时（`registry::cleanup_client`）补一次离线标记。
新增 `mark_closed_of(name, client_id)` 而不是直接复用 `mark_closed`——
重连场景下新会话可能刚注册了同名代理，按名字无条件置离线会把新那条一起标掉，
面板就变成“在线与否看两个清理任务谁先跑”。

### 修复 2：面板的“加代理 / 删代理 / 踢客户端”全部 405

三个写接口返回 `{"code":405,"msg":"method not allowed"}`，前端只显示“操作失败”。

根因在路由顺序：官方兼容层先接管 `/api/*`，其中“查询单条代理 / 单条客户端”
用的是**前缀匹配**——

```rust
if let Some(rest) = path.strip_prefix("/api/v2/proxies/") {
    if method != "GET" { return Some(v2_error(405, "method not allowed")); }
```

而 `/api/v2/proxies/add`、`/api/v2/proxies/remove`、`/api/v2/clients/kick`
的路径恰好都以这两个前缀开头，于是被当成“查单个”的请求，非 GET 一律 405。

修复：非 GET 时返回 `None`，把请求交回自有的 `api_v2` 处理。

### 修复 3：老式 `frps.ini` 根本读不了

能读 `frpc.ini`，但同一份格式的 `frps.ini` 交给服务端会在第 2 行报
`TOML parse error: expected newline` 直接退出。

根因：`ServerConfig::load()` 直接调 `parse_server_toml`，而带 TOML / INI 自动
识别的入口是 `parse_server`。客户端那边走的是 `parse_client`（自带识别），
所以只有服务端坏——用户只会以为“服务端不行”。

修完还发现第二层问题：INI 分支**漏了键名归一化**。`frp_legacy` 转换出来的
是官方键名（`bind_port`），而内部字段叫 `control_port`，不归一化就会被静默
忽略。这个比报错更坏：`bind_port = 7100` 能启动，但服务端实际跑在默认 7000 上，
用户改端口“不生效”，排查半天以为是端口被占。现在 INI 分支与 TOML 分支走同一套
归一化，并且也会正确记下“用户显式写过 bind_addr”（否则换个配置格式就绕过了
启动期的安全校验）。

### 修复 4：官方兼容接口的代理列表缺顶层 `name` / `type`

`GET /api/proxy/{type}` 把 `name` / `type` 只放在 `conf` 里，顶层没有。官方
`ProxyInfoResp` 是把这两个字段放在**最外层**的，按官方结构写的第三方脚本 /
面板读 `item.name` 会拿到 `undefined`。

### 验证

真实二进制端到端（frps + frpc 真进程，非进程内构造）：

| 维度 | 覆盖 |
|---|---|
| 代理类型 | tcp / udp / http / https / tcpmux / stcp / xtcp / sudp —— 8/8 |
| 传输层 | 明文 tcp / TLS / WebSocket / WSS / KCP / QUIC —— 6/6 |
| 面板接口 | v1 + v2 + 官方兼容共 17 个端点 + 未鉴权 401 —— 28/28 |
| 进阶 | 加密+压缩 / HTTP Basic Auth / 路径路由 / static_file 插件 / group 负载均衡 / frpc 重启恢复 —— 8/8 |
| 生命周期 | 上线可用 / 错误 token 被拒 / 下线标离线 / 端口释放 —— 5/5 |
| 安全与配置 | allow_ports 管控 / roles 限类型 / 动态增删代理 / 踢客户端 / frps.ini / tokenSource / includes —— 10/10 |

单元测试同步补齐：`mark_closed_of` 的归属判定、写接口路径不被前缀吞掉、
INI 配置加载与键名归一化。全仓 923 项通过，clippy `-D warnings` 干净。

> 测试脚本保留在 `_interop/e2e_all.py`、`e2e_stage5.py`、`e2e_stage6.py`，
> 改完代码可以直接重跑一遍，不必手工点面板。

## v0.8.4 — 修复 stcp 访问 Go 端加密代理时数据全乱（下行 0B）

现象：**Rust frpc 作为访问端（visitor）去连一台 Go frpc 提供的 stcp 服务，完全连不上。**
服务端日志只有一行耐人寻味的记录：

```text
visitor 接入成功，开始中继 proxy=win_rdp_stcp_wzy kind=stcp
转发结束：上行 47B / 下行 0B proxy=win_rdp_stcp_wzy
```

上行 47 字节正是 RDP 的 X.224 连接请求，说明**配对成功、请求也发出去了**，
但 provider 一个字节都没回来，25 秒后超时断开。

### 根因

只要那个 Go 端代理配了 `transport.useEncryption = true` 就必现；
把它注释掉，同一份配置**立刻通**。

原因是 frp 的数据面加密是**逐跳**的，两条跳用**不同的密钥**：

```text
访客 ⟷ frps ⟷ provider(frpc)
       ↑         ↑
  secretKey     token 派生（Auth.EncryptionKey）
```

官方 frps 在两侧各解一次：

```go
server/proxy/proxy.go:296      libio.WithEncryption(local, pxy.encryptionKey)  // token
server/visitor/visitor.go:86   libio.WithEncryption(rwc, []byte(l.sk))         // secretKey
```

而 Rust 版在 visitor 中继里把两侧都写死成"不加密"：

```rust
// 旧代码（错）
use_encryption: false,
use_compression: false,
data_key: None,
```

于是 frps 变成"密文透传"：provider 加密后的字节被原样交给访客，
两边对不上，数据全乱。**不加密的代理一直是好的**，所以这个问题藏了很久。

### 修复

* `PendingUser` 新增 `visitor_key` / `visitor_compression` 两个字段，
  让"访客那一侧"的密钥与 work 侧分开保存。
* visitor 配对时分别取两侧开关：
  * work 侧 ← `NewProxy.use_encryption`，密钥 `derive_key(token)`；
  * 访客侧 ← `NewVisitorConn.use_encryption`，密钥 `derive_key(secretKey)`。
* `bridge` 改为**两侧各自包装** `ProxyStream`（原来是只包 work 侧）。
  同时把限流分支从 4 条合并成 2 条，逻辑更直白。

### 验证

本地端到端（同一条链路上三种组合）：

| provider 配置 | 旧 0.8.3 | 新 0.8.4 |
|---|---|---|
| `useEncryption = true` | 6 秒超时，**0 字节** | 收到 `ECHO:PING` ✅ |
| `useEncryption = false` | 正常 | 正常（无回归） |

服务端测试 304 + 17 + 26 + 16 + 10 全通过，clippy `-D warnings` 干净。

> 顺带说明：本版**只需升级 frps**。Rust frpc 的 visitor 不需要任何改动，
> 也不需要它在配置里写 `useEncryption`（它那条链路仍按官方语义走明文）。

## v0.8.3 — 修复大流量下 WebSocket 重复建帧导致断连（务必 frps / frpc 同时升级）

现象：穿透远程桌面（stcp / tcp 走 WSS）时**空闲正常，一传大文件或画面刷新密集就断**，
服务端日志出现

```text
yamux 会话错误：decode error: decode error: unknown version: 23
```

`0x17` 正是 TLS 应用数据的首字节 —— 说明 yamux 读到的"帧头"根本不是帧头，
整条会话被错位数据打乱。官方 Go 版 frp 在同样链路上不复现。

### 根因

`WsStream::poll_write` 在底层流写满（返回 `Poll::Pending`）时，把整帧放进发送缓冲
并返回 `Pending`。按 `AsyncWrite` 契约，调用方随后会拿**同一份 buf** 再来一次；
而旧实现在重试时**重新执行了一次建帧**，于是同一段数据被写了**两遍**：

```text
正确：[帧头][载荷]                  → yamux 正常解析
旧版：[帧头][帧头][载荷][载荷]      → 对端从载荷中间开始当帧头读 → unknown version
```

小流量时 socket 发送缓冲从不写满，永远走不到 `Pending` 分支，所以只有"流量大"才触发。

### 修复

新增 `tx_pending_len` 字段记住"这一帧已经建好、还欠调用方一次长度回报"。
重试时先把残留字节冲干净，再回报**原来的长度**，绝不重新建帧。
另外补了一条回归测试：用 64 字节容量的底层流强制走进 `Pending` 重试路径，
逐字节比对收到的内容；带超时保护，避免修复被回退时用例挂死而不是失败。

### 升级提示

**服务端和客户端都要换。** 两端各自独立建帧，只升一端仍会在对端看到错位。
老版本号：`nfrp-server 0.8.2` / 本次：`0.8.3`。

---

## v0.8.2 — 后台面板换成官方 Go 版界面，补齐流量统计

`frps` 的监控面板不再使用内嵌的简易页面，而是**官方 frp v0.72 的前端**（Vue 3 +
element-plus），静态文件直接编译进二进制，开箱即用、无需任何额外部署。

### 1. 官方面板

- 打开 `webServer` 地址即自动跳转到官方界面：系统信息总览、按类型分组的代理列表、
  客户端列表（IP / 主机名 / 版本 / 在线状态）、每个代理最近 7 天的流量曲线。
- 实现了官方 JSON 契约的全部接口：
  `/api/serverinfo`、`/api/proxy/{type}[/{name}]`、`/api/traffic/{name}`、
  `/api/clients`、`/api/v2/system/info`、`/api/v2/clients`、`/api/v2/proxies`、
  `DELETE /api/proxies?status=offline` 等。
- 鉴权沿用 `webServer.user` / `webServer.password`（HTTP Basic，浏览器原生弹窗）。
- 原有 `/metrics`、`/api/status` 与自有 `/api/v2` 接口保持兼容。
- 仍支持 `webServer.assetsDir` 换皮：配置了目录就优先用目录里的文件。

### 2. 新增按代理的流量与连接统计

服务端转发路径（tcp / udp / stcp / xtcp 走 bridge，http / https / tcpmux 走
vhost）全部挂上按代理记账：今日上下行流量、当前连接数、最近 7 天按天的字节曲线、
代理的注册 / 关闭时间。客户端会话档案（首次登录、最近在线、操作系统 / 架构 /
版本 / 主机名 / IP）也会体现在面板的 Clients 页。

> 流量数据从升级后的首次启动开始累计。

### 3. 顺带的修正

- v2 代理列表的 `spec` 按官方联合类型返回（带 `type` 判别字段），修复官方面板
  Proxies 页报 `Unsupported proxy spec` 的问题。
- 服务端在 CDN / WAF 之后使用时，建议 `bindAddr = "::"` 开启 IPv6/IPv4 双栈监听：
  家宽无公网 IPv4 时 ESA 会走 IPv6 回源，`0.0.0.0` 只听 IPv4 会全部
  `connection_refused`（详见 [docs/CDN.md](https://github.com/wengjing086/rust-frp-wss-ip4p/blob/main/docs/CDN.md) 第八节）。
- v0.8.1 的全部改动（wss 握手带 `Origin` 过 CDN、服务端两级嗅探 + HTTP 兜底）
  完整保留。

### 4. 回归

- `cargo test` 全绿（common 532 项 + server 304 项，含 spec 判别字段回归测试）
- `clippy -D warnings` 通过
- 已在 iStoreOS（x86_64）实机部署验证：面板、API、客户端重连、stcp 隧道均正常

---

## 下载

每个 Release 附**独立可执行文件**（不打包）与 `SHA256SUMS.txt`。校验值由
`scripts/make_release.py` 按 `dist/` 中真实存在的产物自动生成 —— 请以对应版本
`SHA256SUMS.txt` 实际列出的文件为准，不要照抄任何静态表格。

常规交付平台：Windows x86_64、Linux x86_64 / ARM64（静态 musl，不依赖 glibc）。
Windows ARM64 需要 clang + `aarch64-pc-windows-gnullvm` 交叉工具链，缺工具链的
版本不提供该平台。

```bash
chmod +x frps_linux_amd64 frpc_linux_amd64
./frps_linux_amd64 -c frps.toml
```
