## v0.8.7 — 默认资源上限、密钥派生缓存、发布链路可信度整理

这一版**没有改线协议**，也没有引入新的互通能力；重点是把「跑起来没问题、
但部署到公网长期不够稳 / 对外承诺对不上」的几处收掉。0.8.6 与 0.8.7 可以混用，
frpc 不是必须升级（本版所有服务端行为变化都向后兼容）。

### 修复与加固

**1. 三条转发闸门默认从「不限」改成保守上限。**
`max_total_conns` / `max_conns_per_client` / `max_pending_per_client` 以前默认
`0 = 不限`，而 `server/src/limits.rs` 的模块注释自己写明这是防 OOM 用的 ——
预认证握手有 `PREAUTH_MAX = 2048` 闸门，认证后的转发连接却完全没有上限。
现在默认 `5000 / 200 / 64`（就是示例配置里一直推荐的那组值），**显式写 `0`
仍能恢复旧的「不限」行为**。

这里有个容易漏的坑：结构体没有容器级 `#[serde(default)]`，只改 `impl Default`
会导致「代码里 new 出来的配置有限流、读 TOML 解析出来的没有」。所以同时给三个字段
指定了 `#[serde(default = "...")]` 函数，并加了回归测试把两条路径钉在一起。

**2. 每条连接不再重算 64 轮 HMAC-SHA1。**
`derive_key(token)` 是官方向量的纯派生函数，token 是进程级常量，但控制通道、
每条加密隧道、stcp 双侧都按连接调用一次。新增 `derive_key_cached()` 按 token
内容缓存（**不能用单值 `OnceLock`**：stcp visitor 侧用的是每个代理各自的
`secretKey`，串值会变成「连得上但数据全乱」），运行时调用点全部切过去；
纯函数保留，官方向量测试继续测它。另把 `PendingUser` 构造里对同一代理名的
3 次 `proxy_flags()` 查询合并成 1 次。跨实现加密用例（`compat_go.py` 阶段 D）
全部通过，确认缓存没有串密钥。

**3. 修掉一个偶发失败的单测竞态。**
`nathole_official::nat_report_成功会回写分数` 会偶发挂掉，原注释归因为
「撞 session 名」。实际根因是另一个测试调用 `configure_analysis_reserve(0)`，
它会**清空整张全局 `hole_records` 表**，唯一 sid 挡不住全表清空。现在所有读写该表的
测试统一用已有的 `test_lock` 串行，不再互相踩。

**4. 启动期告警文案自相矛盾。**
`server/src/main.rs` 里一处写着「服务端不校验时间戳新鲜性」，同文件另一处又写着
「v0.5.7 已更正，全部 6 个凭证入口都加了 ±300 秒窗口」。现在统一成
「窗口已收窄到 5 分钟，但窗口内抓到一条 Login 仍可离线枚举 token」——
结论不变（弱 token 依然要告警），但不再误导运维判断重放面。

### 工程化与发布链路

- **CI 配置整理**：`ci/ci.yml` 在 push / PR 跑 `fmt --check` /
  `clippy -D warnings` / `cargo test --workspace` 与三平台 release 构建；
  smoke 里以前写 `|| true`（构建出跑不起来的二进制也会变绿），已去掉。
  ⚠️ 当前使用的 PAT 只有 `repo` 权限、**没有 `workflow` scope**，GitHub 会拒绝
  任何对 `.github/workflows/` 的写入（API 也返回 404），所以这两个 workflow
  **尚未在 GitHub 上启用**。同内容配置以模板形式随源码入库在 `ci/` 下，
  启用步骤见 `ci/README.md`：给 PAT 勾 `workflow` 后推送，或在网页端新建文件。
- **统一发布契约**：`ci/release.yml` 原来打 3 平台压缩包并自动上传 Release，
  与 `dist/` 交付的「4 平台独立可执行文件 + SHA256SUMS」是两套。现在
  `release.yml` 改为**可复现性校验构建**，只产出同名独立文件与哈希、上传为
  Actions artifacts，**不写 Release**；正式资产仍由 `scripts/build_all.sh`
  本地交叉编译（gnu 版本资源 / musl 静态链接）后上传，避免同名资产互相覆盖。
- **发布脚本进版本控制**：`RELEASE_NOTES.md`、`make_release.py`、
  `upload_assets.py` 原先都在被整目录忽略的 `dist/` 下（`dist/keys/` 有签名私钥，
  不做否定放行），别人拿到源码根本无法复现发布。现移到 `docs/` 与 `scripts/`。
- **脚本去硬编码**：TAG 从 `Cargo.toml` 推导（`upload_assets.py` 原先停在
  `v0.8.0`）；token 优先读 `GITHUB_TOKEN` / `GH_TOKEN` 环境变量；
  工具链路径可由 `TOOLS_DIR` 覆盖；`build_all.sh` 改 `set -euo pipefail`，
  必选平台构建失败不再「跳过」后拿旧产物发版；`SHA256SUMS.txt` 按实际存在的
  产物自动生成。
- **下载表不再承诺不存在的文件**：Windows ARM64 需 clang + gnullvm，
  当前环境缺 clang，v0.8.6 / v0.8.7 都没这个产物，文档与 `SHA256SUMS.txt`
  现在一致。README / `docs/ANTIVIRUS.md` 里「全部二进制由 GitHub Actions 构建」
  的说法也已改成与事实相符的表述（此前误报申诉模板还引用了一个仓库里
  根本不存在的 `.github/workflows/release.yml`）。
- **README 新增「生产部署建议配置」**：三道限流的默认值与 `0 = 不限` 语义、
  `udpPacketSize` 默认 `65507` 与官方 `1500` 的差异、TLS 默认
  `InsecureSkipVerify = true`（只防窃听、不防主动 MITM）及其显式开启方式；
  并把散落在源码注释里的已知限制（macOS 无 TUN、legacy INI 不支持 tcpmux、
  OIDC 302、KCP / wss / xtcp 跨实现等）集中成一节。

### 回归

- 单测 **1070 项全过**（新增 3 项：缓存派生一致性与按 token 隔离、
  TOML 默认值与 `impl Default` 一致、显式 `0` 恢复不限）
- `cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 干净
- 真实二进制 e2e：内部 `e2e_all.py` 42/42、跨官方 Go frp `compat_go.py`
  **45/45 通过 + 7 项已知不支持**、阶段 5 生命周期 5/5、阶段 6 配置兼容 10/10、
  踢客户端回归通过
- 8 MiB 单条与 3×4 MiB 并发 wss 大流量全通；stcp 跨版本四种组合含 8 MiB 全通

### 升级说明

服务端单独升级即可拿到全部改动，旧版 frpc（含 0.8.5 / 0.8.6 与官方 Go frpc）
连新版 frps 正常工作。唯一需要留意的行为变化是三道连接上限：客户端特别多或
单客户端并发很高时按容量调大，想完全恢复旧行为就把三项显式写成 `0`。

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

## 下载（独立可执行文件，不打包）

| 平台 | 服务端 | 客户端 |
|---|---|---|
| Windows x86_64 | `frps_windows_amd64.exe` | `frpc_windows_amd64.exe` |
| Linux x86_64（静态 musl） | `frps_linux_amd64` | `frpc_linux_amd64` |
| Linux ARM64（静态 musl） | `frps_linux_arm64` | `frpc_linux_arm64` |
| Windows ARM64 | **本版暂不提供** | **本版暂不提供** |

Windows ARM64 需要 clang + `aarch64-pc-windows-gnullvm` 交叉工具链，当前构建环境
缺 clang，v0.8.6 / v0.8.7 都没有这个平台的产物；补齐时会在对应版本说明里写明。
请以 `SHA256SUMS.txt` 里**实际列出**的文件为准 —— 校验值由
`scripts/make_release.py` 按 `dist/` 中真实存在的产物自动生成，不手工维护。

Linux 版静态链接，不依赖 glibc：

```bash
chmod +x frps_linux_amd64 frpc_linux_amd64
./frps_linux_amd64 -c frps.toml
```
