# nfrp-server

`rust-frp-wss-ip4p` 的服务端可执行文件，等价官方 `frps`；Release 包中命名为
`frps` / `frps.exe`。

配置文件与官方 frp 完全一致。在 `bindPort` 上**额外**原生支持 WSS 直连：
TLS 解密后嗅探 `GET /~!frp`，命中走 WebSocket 升级，未命中回放字节继续原生
frp over TLS。原理见 [docs/WSS.md](../docs/WSS.md)。

完整说明见仓库根目录 [README.md](../README.md)。
