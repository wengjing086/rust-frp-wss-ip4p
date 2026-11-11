# frps 同端口 WSS 直连

## 问题

官方 frps 在 `bindPort` 上用 `mux.Mux` 按**明文前缀** `GET /~!frp` 分流：

```text
TCP 首字节 = 'G'  → 明文 WebSocket 监听
TCP 首字节 = 0x16 → TLS 监听（frp over TLS）
其他              → 明文 frp
```

`transport.protocol = "wss"` 的客户端套壳顺序是 `TCP → TLS → WebSocket`，
首字节是 TLS ClientHello 的 `0x16`，官方 frps 只会把它送进 TLS 监听，
解密之后仍按 frp 协议解析，于是**连上就断**。官方的做法是让用户自己
在前面挂 nginx 终结 TLS 再反代 WebSocket，配置与证书都多一层。

`wengjing086/frp-wss-ipgeo` 的补丁解决了这个问题：TLS 解密后 peek
`GET /~!frp`，命中就交给 WSS 监听；未命中把字节回放，回到原版行为。

## 本项目的实现

改动集中在 `server/src/serve.rs` 的 `handle_conn()`，引入 `probe_ws()`
做两级嗅探：

```text
                       ┌─ 'G' ──► probe_ws()（TLS 之前）
TCP 读 1 字节 ─────────┤            │ 命中 → ws::accept() → 明文 WebSocket
                       │            └ 未命中 → TLS 协商 → 普通 frp
                       │
                       └─ 其他 ─► tls::accept_server()
                                    │ 0x17 frp 自定义首字节 → TLS
                                    │ 0x16 标准 ClientHello  → TLS
                                    │ 其他                    → 明文透传
                                    ▼
                              ★ probe_ws()（TLS 解密之后）
                                    │ 命中 → ws::accept() → WSS
                                    └ 未命中 → PrefixedStream 回放 → 原版 frp
```

关键点：

1. **字节零丢失**。`probe_ws()` 逐字节比对 `GET /~!frp`，一旦不匹配立即停止，
   所有读过的字节都用 `PrefixedStream` 塞回流的最前面。非 WebSocket 客户端
   只多读 1 个字节，行为与改造前一致。
2. **探测有时限**。明文路径用 `PROBE_TIMEOUT`（10s，与原版一致）；
   TLS 内的二次探测用 `WSS_PROBE_TIMEOUT`（3s），避免解密后首字节恰好是
   `G`、而客户端首包又很短时被拖住。超时只判定为「非 WebSocket」，不丢数据。
3. **客户端发标准 TLS ClientHello**。官方 frpc 的 `wss` 分支不挂
   `DialHookCustomTLSFirstByte`，所以本项目客户端在 `wss` 时强制关闭 frp
   自定义首字节 `0x17`，改发 `0x16` —— 这样也能连打了补丁的官方 frps 或 nginx。
   服务端两种首字节都收，互不冲突。
4. **零配置**。`frps.toml` 不需要任何 WSS 相关项；`frpc.toml` 只用官方已有的
   `transport.protocol = "wss"`。

## 使用

frps（配置不变）：

```toml
bindAddr = "0.0.0.0"
bindPort = 7000
auth.token = "强令牌"
```

frpc：

```toml
serverAddr = "your.server.com"
serverPort = 7000
auth.token = "强令牌"
transport.protocol = "wss"
```

frps 用内置自签证书终结 TLS；官方 frpc 默认 `InsecureSkipVerify = true`，
本项目客户端默认一致。若需要严格校验，按官方写法配置
`transport.tls.trustedCaFile` / `tlsServerName` / `verifyServerCert`。

> 生产环境建议在 frps 前面放正规 CA 证书（或让 frps 加载 `certFile` /
> `keyFile`），避免长期依赖自签 + 不校验。

## 不可用的组合

`transport.tls.enable = true` **且** `transport.protocol = "websocket"`
（明文 WebSocket + 独立的 TLS 层）是坏组合：TLS 加密后服务端看到的是密文
请求行，无法嗅探。客户端会打印明确告警，请改用 `protocol = "wss"`。

## 互通矩阵

| frps \ frpc | tcp | websocket | tls | **wss** |
|---|---|---|---|---|
| 官方 frps | ✅ | ✅ | ✅ | ❌（需 nginx 终结 TLS） |
| 本项目 frps | ✅ | ✅ | ✅ | ✅（同端口直连） |
| frp-wss-ipgeo 补丁版 frps | ✅ | ✅ | ✅ | ✅（同端口直连） |
