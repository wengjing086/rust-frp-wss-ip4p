# IP4P 客户端解析

## 适用场景

frps 在 NAT 之后，公网 IP 与映射端口会动态变化（natmap / lucky / DDNS 等
工具把当前 `IP:端口` 写进 DNS）。frpc 不能写死 `serverPort`，需要在每次
拨号前把真实端点查出来。

## 解析规则

`common/src/ip4p.rs` 实现，优先级与 `wengjing086/frp-ip4p` 一致：

### 1. TXT 记录

```
your.ip4p.example.com.  IN TXT  "MjAzLjAuMTEzLjc6MTgyNDY="
```

解码得到 `203.0.113.7:18246`。兼容：

- base64 标准 / 无填充 / URL-safe / URL-safe 无填充，四种变体；
- 明文 `203.0.113.7:18246`；
- 记录值两侧的引号与空白（部分 resolver 会保留）；
- IPv6 字面量 `[2001:db8::1]:7000`。

拒绝：非 IP 主机名、端口为 0 或越界、端口非数字、空值。

### 2. AAAA 记录（IP4P 编码）

`2001:0000::/16` 段内的 IPv6 地址，布局：

```text
字节  0  1  2  3  ...  10   11   12  13  14  15
     20 01 00 00      p-hi p-lo  a  b  c  d
                       └─ port ─┘  └── IPv4 ──┘
```

即 `port = (b10 << 8) | b11`，`IPv4 = b12.b13.b14.b15`。

### 3. 回退普通模式

无匹配记录时原样使用 `serverAddr` + `serverPort`，与官方 frp 完全一致。
**没有 IP4P 记录的普通域名不会有任何行为变化。**

## 端口覆盖

解析成功时，记录里的端口**覆盖** `serverPort`；因此用 IP4P 时可以不写
`serverPort`。但 TLS SNI 与 WebSocket 的 Host 头仍使用配置里的原始域名 ——
这点很重要，否则自签证书校验与 nginx 按 Host 分流都会被破坏。

## 生效范围

| 拨号路径 | 是否经过 IP4P |
|---|---|
| TCP / `websocket` / `wss` (`raw_connect`) | ✅ |
| KCP (`kcp_connect`) | ✅ |
| QUIC (`quic_server_addr`) | ✅ |
| NLink QUIC 未显式配 `quic_addr` 时 | ✅ |
| NLink QUIC 显式配了 `quic_addr` | ❌（已写死地址） |
| xtcp 打洞的 rendezvous (`p2p.rs`) | ❌（与 Go 版补丁范围一致） |
| `serverAddr` 是 IP 字面量 | ❌（直接跳过 DNS，零开销） |

## DNS 服务器选择

1. 配置了官方已有的 `dnsServer` ⇒ 只用它，行为确定，不碰公网。
2. 否则用系统 DNS：Linux 读 `/etc/resolv.conf`；Windows 用 `GetNetworkParams`
   读取适配器配置（**不起 `ipconfig` 子进程**，避免可疑的进程创建行为）。
3. 系统 DNS 完全没应答时才回落到公共 DNS（223.5.5.5 / 119.29.29.29 /
   8.8.8.8 / 1.1.1.1）。

「有效应答但没有 TXT 记录 / NXDOMAIN」被当作**确定的否定结果**，不会
再向公共 DNS 发查询 —— 普通域名不会白白往外发包。

## 缓存

每次拨号都解析（Go 版同样如此），但加了进程内缓存避免 DNS 开销：

- 命中 IP4P：缓存 10 秒（natmap 端口可能变化，不能太久）；
- 未命中：缓存 60 秒（普通域名不必每条工作连接查一轮）。

`ip4p::clear_cache()` 可供热重载 / 测试调用。

## 排查

```bash
nslookup -type=TXT your.ip4p.example.com
# 或
dig TXT your.ip4p.example.com
echo 'MjAzLjAuMTEzLjc6MTgyNDY=' | base64 -d      # ⇒ 203.0.113.7:18246
```

客户端日志会明确打印解析结果与来源（`txt` / `aaaa`）；没有记录时是
`ip4p: 未发现 IP4P 记录，按普通模式连接`（debug 级别）。

> 注意：TXT 记录可能被某些 DNS 转发器过滤。此时可显式指定
> `dnsServer = "223.5.5.5:53"`（官方已有键）。
