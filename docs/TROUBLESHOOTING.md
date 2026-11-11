# 排障：wss 连不上

## 先判断"403 是谁返回的"

frp 自己**不会**返回 HTTP 403。客户端报

```
WebSocket 握手失败（frps.example.com/~!frp）: 服务端返回 403（期望 101）
```

说明请求在到达 frps **之前**就被 HTTP 层处理掉了 —— 也就是你前面挂了反向代理，
或者域名走了 CDN。

一条命令就能看出来：

```bash
curl -s -i -H "Connection: Upgrade" -H "Upgrade: websocket" \
     -H "Sec-WebSocket-Version: 13" \
     -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
     https://frps.example.com/~!frp | head -20
```

重点看 `Server` 响应头：

| `Server` 头 | 说明 | 处理方式 |
|---|---|---|
| `nginx` | 自建反代 | 补 `location /~!frp` 的 Upgrade 转发（见下） |
| `ESA` / `AliyunOSS` / `Tengine` | 阿里云 CDN / 边缘安全加速 | CDN 会拦 `~!frp` 这类路径，**必须绕过 CDN** |
| `cloudflare` | Cloudflare | 同上，且免费版通常不允许非 80/443 端口的隧道流量 |
| `frps` / 无 | 直连到 frps | 上面的 403 不应出现；若出现请看服务端日志 |

## 情况一：前面是自建 nginx

nginx 默认不转发 `Upgrade`，WebSocket 握手会被当成普通 HTTP 请求，
`/~!frp` 这个路径又没有对应的 `location`，于是回 403 / 404。

```nginx
location /~!frp {
    proxy_pass http://127.0.0.1:7000;      # 指向 frps 的 bindPort
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;

    # frp 是长连接，读超时要放大，否则空闲一会儿就被 nginx 掐断
    proxy_read_timeout 3600s;
    proxy_send_timeout 3600s;
    proxy_buffering off;
}
```

改完 `nginx -s reload` 再试。

> 对比：官方 frps 必须这样绕；**本项目 frps 不需要** —— 它能在 `bindPort` 上直接
> 接住 `wss`。有 nginx 只是为了复用一个 443，不是功能上的必需。

## 情况二：域名走了 CDN（最常见）

典型特征是响应里带 `Server: ESA`、`via: ...ens-cache...`、`EagleId`、
`Set-Cookie: acw_tc=...` 这类头。此时：

- 403 是 **CDN 的 WAF** 给的：`/~!frp` 这种"路径里带特殊字符 + 立即 Upgrade"
  的请求在风控规则里很像扫描器，会被直接拦掉；
- 就算放行了路径，普通 HTTPS 请求还会看到 `520 Origin Error` /
  `error=http_response_incomplete` —— 因为源站是 frps，它收到 TLS 后不会回
  HTTP 响应，CDN 判定"回源异常"。

**结论：frp 的控制连接不要过 CDN。** 隧道流量是长连接 + 非 HTTP 语义，CDN 的
缓存与风控模型都不适合它，即使勉强通了，限速和断流也会让你不停重连。

正确的做法是让 frpc **直连源站**：

1. 在 DNS 里把该记录改成"仅解析"（阿里云叫"关闭 CDN 加速"、Cloudflare 叫灰云
   `DNS only`），让它直接指向源站 IP；
2. frpc 的 `serverAddr` 用这个域名（或直接写 IP）；
3. `serverPort` 写 frps 实际监听的端口（`bindPort`，例如 7000）。

如果想继续用 443，就让 frps 自己监听 443：

```toml
# frps.toml
bindPort = 443
```

```toml
# frpc.toml
serverAddr = "frps.example.com"
serverPort = 443
transport.protocol = "wss"      # 本项目 frps 直接接住，不需要 nginx
```

## 情况三：确实是 frps 在 443 上，但用的是官方 Go 版

官方 frps 在 **TLS 之前**嗅探明文 `GET /~!frp`，wss 的首字节是 `0x16`，
永远匹配不上。表现为"连上就断"，两边日志都看不出所以然。

- 要么按情况一挂 nginx 终结 TLS；
- 要么换成**本项目的 frps**：它在 TLS 解密之后再嗅探一次，wss 可以直接连
  `bindPort`，服务端零配置。

## 怎么确认自己连的到底是谁

```bash
# 看端口是谁在听
curl -s -o /dev/null -w "%{http_code}\n" https://frps.example.com/
# 200/403/404 都说明前面有 HTTP 服务（反代或 CDN）
# 直接超时 / 连接被重置，才说明是 frps 自己在监听
```

再看 frpc 的日志前缀：`WebSocket 握手失败（frps.example.com/~!frp）` 里的
`Host/SNI` 是**配置里的域名**，不是实际连到的 IP —— 走 IP4P 时两者可能不同，
排查时不要被这一点误导。
