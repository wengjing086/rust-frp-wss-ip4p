# rust-frp-wss-ip4p

**用 Rust 写的 frps / frpc 整合发行版**：把三个仓库的能力合到一个项目里，
配置文件与官方 frp 完全保持一致，服务端同端口原生支持 WSS，客户端支持 IP4P，
并针对 Windows 安全软件的误报做了合规处理。

| 需求 | 实现 |
|---|---|
| ① `frps.toml` / `frpc.toml` **一个字都不用改** | 沿用上游 TOML / legacy INI 兼容层；WSS 与 IP4P 都是**自动探测**，零新增配置键 |
| ② frps / frpc 支持 **WSS** | frps 在 `bindPort` 上同时接受 TCP / TLS / WSS；frpc 用官方 `transport.protocol = "wss"` 直连，无需 nginx |
| ③ frpc 支持 **IP4P** | 拨号前解析 DNS TXT（base64/明文 `IP:端口`）→ IP4P AAAA 编码 → 回退普通模式 |
| ④ **不被杀毒软件误杀** | Rust 原生编译（无 Go 运行时特征）+ 完整版本资源 + `asInvoker` 清单 + 不加壳不混淆 + 可复现构建与 SHA256 校验 |
| 修好代码并发布 | 源码在 GitHub，Release 提供多平台预编译二进制与 `SHA256SUMS.txt` |

---

## 项目来源与致谢

| 来源 | 取用了什么 |
|---|---|
| [Nu0vo1212/Rust-Frp](https://github.com/Nu0vo1212/Rust-Frp) (v0.7.6, Apache-2.0) | Rust 协议基座：`common` 协议/配置/加密/传输、`server`、`client` 三个 crate，与官方 frp v0.71.0 线协议互通 |
| [wengjing086/frp-wss-ipgeo](https://github.com/wengjing086/frp-wss-ipgeo) | frps **同端口 WSS 直连**的思路：TLS 解密后 peek `GET /~!frp`，命中走 WebSocket 升级，否则字节回放继续原生 frp |
| [wengjing086/frp-ip4p](https://github.com/wengjing086/frp-ip4p) | frpc 的 IP4P 解析规则：TXT（四种 base64 变体 / 明文）→ AAAA（`2001:0000::/16`）→ 回退普通模式，端口覆盖 `serverPort` |
| [fatedier/frp](https://github.com/fatedier/frp) | 线协议与配置文件格式（本项目按其文档实现互通，不含其源码） |

> 本项目是**整合发行版**，不是官方 frp，也不是 NFrp 的镜像。
> 归属与第三方许可见 [NOTICE](NOTICE)。

---

## 快速开始

### 1. 服务端（配置完全不用改）

沿用你现有的 `frps.toml`：

```toml
bindAddr = "0.0.0.0"
bindPort = 7000
auth.method = "token"
auth.token = "换成一个足够随机的强令牌"
```

```bash
frps -c frps.toml
```

`bindPort` 现在会**自动**同时接受：

- 原生 `tcp`（默认）
- `transport.tls.enable = true` 的 frp over TLS
- `transport.protocol = "websocket"` 的明文 WebSocket
- `transport.protocol = "wss"` 的 **TLS 内 WebSocket（本项目新增，同端口直连）**

### 2. 客户端

沿用你现有的 `frpc.toml`。要用 WSS 只改官方本来就有的那一行：

```toml
serverAddr = "your.server.com"
serverPort = 7000
auth.method = "token"
auth.token = "和服务端一致的强令牌"

transport.protocol = "wss"     # 唯一新增的一行；官方 frpc 也是这么写

[[proxies]]
name = "ssh"
type = "tcp"
localIP = "127.0.0.1"
localPort = 22
remotePort = 6022
```

```bash
frpc -c frpc.toml
```

看到 `login to server success` 即成功。

### 3. IP4P（同样不用改配置）

把 `serverAddr` 填成发布 IP4P 记录的域名即可，**无需任何新键**：

```toml
serverAddr = "your.ip4p.example.com"
# serverPort 可省略：TXT / AAAA 里解析出的端口会自动覆盖它
auth.token = "..."
```

客户端按以下顺序解析，命中即返回：

| 优先级 | 记录 | 内容 |
|---|---|---|
| 1 | TXT | `base64("IPv4:端口")`，如 `MjAzLjAuMTEzLjc6MTgyNDY=` ⇒ `203.0.113.7:18246`；也兼容明文 |
| 2 | AAAA | IP4P 编码 IPv6（`2001:0000::/16`），端口在字节 10–11，IPv4 在字节 12–15 |
| 3 | 回退 | 无匹配记录 ⇒ 原样使用 `serverAddr` + `serverPort`，与官方行为一致 |

日志示例：

```
ip4p: 由 TXT 记录解析出服务端地址 host=your.ip4p.example.com resolved=203.0.113.7:18246
```

`dnsServer`（官方已有键）若配置，TXT / AAAA 查询会走它；否则走系统 DNS。
`serverAddr` 是 IP 字面量时**完全跳过 DNS 查询**，零额外开销。

详见 [docs/IP4P.md](docs/IP4P.md) 与 [docs/WSS.md](docs/WSS.md)。

---

## 从源码构建

要求 Rust 1.75+（推荐 stable）。本项目**不引入任何新的第三方依赖**，
所有能力都基于基座已有的 crate 实现。

```bash
cargo build --release
# 产物：target/release/nfrp-server(.exe)、target/release/nfrp-client(.exe)
# Release 包里按官方习惯重命名为 frps / frpc
```

质量门禁：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Windows 版本资源由 `scripts/gen_winres.py` 生成，`.res` 已随仓库提交，
构建时**不需要** `rc.exe` / Windows SDK：

```bash
python scripts/gen_winres.py          # 重新生成（改版本号后）
python scripts/gen_winres.py --check  # 只校验结构
```

---

## 关于杀毒软件误报

Go 版 frp 常被报成 `HackTool:Win32/Frp` 之类，主因是 Go 运行时特征 + 隧道类
工具的历史滥用标签。本项目采取的都是**合规、可审计**的措施，不是免杀：

- Rust 原生编译，不含 Go 运行时；
- 完整 VERSIONINFO（公司 / 产品 / 描述 / 版本 / 版权 / 原始文件名）；
- 应用清单声明 `asInvoker`（不请求管理员权限）、DPI / 长路径感知；
- 不加壳、不压缩、不混淆、不注入、不自修改、不写自启动、不在运行时下载可执行内容；
- 全部二进制由 GitHub Actions 从公开源码构建，附 `SHA256SUMS.txt` 便于核验与申诉。

详见 [docs/ANTIVIRUS.md](docs/ANTIVIRUS.md)（含误报申诉渠道与自证清单）。
**没有任何措施能保证所有厂商零误报**；遇到误报请走正规申诉，而不是加壳。

---

## 兼容性与限制

- 线协议兼容官方 frp **v0.71.0**（v1 默认，可选 v2）；`frpc -v` 输出仍为裸 `0.71.0`。
- `frps` / `frpc` 与官方二进制可混搭；第三方 frp 服务商下发的配置可直接使用。
- WSS 直连是**服务端增强**：连未打补丁的官方 frps 时，wss 仍需前置 nginx 终结 TLS。
- IP4P 只作用于客户端拨号路径（TCP / WS / WSS / KCP / QUIC）；xtcp 打洞的
  rendezvous 地址仍按 `serverAddr` 处理，与 Go 版补丁范围一致。
- `serverAddr` 是域名时会做一次 TXT/AAAA 探测；命中缓存 10 秒、未命中缓存 60 秒。
- 反向代理 / nginx 场景：wss 客户端的 TLS SNI 与 WebSocket Host 仍使用配置里的
  原始域名，IP4P 只改变实际连接目标，不会破坏按 Host 分流与证书校验。

---

## 安全与免责声明

- 部署到公网请务必使用强 token、开启 TLS 并限制 `allowPorts`。
- 本软件仅供在**你拥有或获得明确授权**的设备与网络上使用；使用者须自行遵守
  所在地法律法规。
- 许可证：**Apache License 2.0**，见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。
