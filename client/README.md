# nfrp-client

`rust-frp-wss-ip4p` 的客户端可执行文件，等价官方 `frpc`；Release 包中命名为
`frpc` / `frpc.exe`。

配置文件与官方 frp 完全一致，新增两项自动能力：

- **IP4P**：`serverAddr` 为域名时，拨号前解析 DNS TXT / IP4P AAAA 记录，
  用记录里的动态公网 `IP:端口` 连接；无记录时静默回退普通模式。见
  [docs/IP4P.md](../docs/IP4P.md)。
- **WSS 直连**：`transport.protocol = "wss"` 发标准 TLS ClientHello，
  可直接连本项目 frps 的 `bindPort`，无需 nginx。见 [docs/WSS.md](../docs/WSS.md)。

完整说明见仓库根目录 [README.md](../README.md)。
