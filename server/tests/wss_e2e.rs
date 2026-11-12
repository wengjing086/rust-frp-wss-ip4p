//! 端到端回归：同端口 WSS 直连，以及新增的「TLS 后二次嗅探」不破坏原有路径。
//!
//! 这组用例的意义在于：`handle_conn()` 现在会在 TLS 解密之后再读 1~10 个字节
//! 做 WebSocket 前缀比对，非 WebSocket 连接必须**逐字节无损回放**。只靠单元测试
//! 证明不了「frp over TLS 仍然能握手」，必须真起一个服务端跑一遍线协议。
//!
//! 覆盖四条路径：
//!
//! | 用例 | 客户端 | 服务端分流 |
//! |---|---|---|
//! | `wss_login_over_bind_port` | TCP → 标准 TLS → WebSocket | TLS 后嗅探命中 ⇒ WSS |
//! | `standard_tls_login_still_works` | TCP → 标准 TLS(0x16) | TLS 后嗅探未命中 ⇒ 回放 ⇒ frp |
//! | `custom_head_byte_tls_login_still_works` | TCP → 0x17 + TLS | 同上（frp 自定义首字节） |
//! | `plain_websocket_login_still_works` | TCP → 明文 WebSocket | TLS 之前嗅探命中 ⇒ WS |

use std::{collections::HashMap, sync::Arc, time::Duration};

use nfrp_common::{
    config::ServerConfig,
    frp::{conn, stream::BoxStream, tls, WireVersion},
    security::Credential,
    ws,
};
use nfrp_server::{serve_on, Registry};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "wss-e2e-token";

async fn start_server() -> u16 {
    start_server_with(ServerConfig {
        bind_addr: "127.0.0.1".into(),
        token: TOKEN.to_string(),
        ..Default::default()
    })
    .await
    .0
}

/// 起一个服务端并**把注册表交出来**（测 `X-Forwarded-For` 要看客户端 IP）。
async fn start_server_with(cfg: ServerConfig) -> (u16, Arc<Registry>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let registry = Arc::new(Registry::unlimited());
    let reg = registry.clone();
    tokio::spawn(async move {
        if let Err(e) = serve_on(listener, Arc::new(cfg), reg).await {
            eprintln!("测试服务端退出：{e:#}");
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, registry)
}

fn tls_options(disable_custom_first_byte: bool) -> tls::ClientTlsOptions<'static> {
    tls::ClientTlsOptions {
        enable: true,
        server_name: "localhost",
        disable_custom_first_byte,
        verify: false, // 官方 frpc 默认 InsecureSkipVerify = true
        trusted_ca_file: "",
        client_cert_file: "",
        client_key_file: "",
    }
}

async fn login(stream: BoxStream) -> String {
    let metas: HashMap<String, String> = HashMap::new();
    let (conn, run_id, _udp_binary, _caps) = conn::client_handshake(
        stream,
        WireVersion::V2,
        &Credential::Token(TOKEN.to_string()),
        "wss-e2e-client",
        "wss-tester",
        &metas,
        0,
        Default::default(),
    )
    .await
    .expect("frp 握手必须成功");
    assert!(!run_id.is_empty(), "服务端必须分配 run_id");
    drop(conn);
    run_id
}

/// 本项目新增的核心能力：`transport.protocol = "wss"` 直连 bindPort。
#[tokio::test]
async fn wss_login_over_bind_port() {
    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    // wss 客户端发标准 TLS ClientHello（不挂 frp 自定义首字节 0x17）。
    let tls_stream = tls::connect_client(Box::pin(tcp), &tls_options(true))
        .await
        .expect("TLS 握手");
    let ws = ws::connect(tls_stream, "localhost", ws::FRP_WS_PATH, &[])
        .await
        .expect("WebSocket 升级（WSS）");
    login(Box::pin(ws)).await;
}

/// 新增的 TLS 后探测必须把读到的字节完整回放，否则 frp over TLS 会握手失败。
#[tokio::test]
async fn standard_tls_login_still_works() {
    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let tls_stream = tls::connect_client(Box::pin(tcp), &tls_options(true))
        .await
        .expect("TLS 握手");
    login(tls_stream).await;
}

/// frp 自定义首字节 0x17 的那条 TLS 分支同样不能被破坏。
#[tokio::test]
async fn custom_head_byte_tls_login_still_works() {
    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let tls_stream = tls::connect_client(Box::pin(tcp), &tls_options(false))
        .await
        .expect("TLS 握手（0x17 首字节）");
    login(tls_stream).await;
}

/// 官方明文 WebSocket 路径保持不变。
#[tokio::test]
async fn plain_websocket_login_still_works() {
    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let ws = ws::connect(Box::pin(tcp), "localhost", ws::FRP_WS_PATH, &[])
        .await
        .expect("WebSocket 升级（明文）");
    login(Box::pin(ws)).await;
}

// ---------------------------------------------------------------------------
// ★ v0.8.1：CDN 适配
// ---------------------------------------------------------------------------

/// **路径无关的回归用例**：任意路径都必须能升级。
///
/// 背景：服务端**不能**死认 `GET /~!frp`，否则自定义路径永远握不上手。
/// 注意：改路径本身不是 CDN 403 的解法（真因是缺 `Origin`），这里只验证
/// 「路径不参与判定」这件事，见 docs/CDN.md。
#[tokio::test]
async fn wss_login_with_custom_path() {
    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let tls_stream = tls::connect_client(Box::pin(tcp), &tls_options(true))
        .await
        .expect("TLS 握手");
    let ws = ws::connect(tls_stream, "frps.example.com", "/any/path", &[])
        .await
        .expect("自定义路径的 WSS 升级");
    login(Box::pin(ws)).await;
}

/// 明文分支同样支持自定义路径（`protocol = "websocket"` + CDN 明文回源）。
#[tokio::test]
async fn plain_websocket_with_custom_path() {
    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let ws = ws::connect(Box::pin(tcp), "frps.example.com", "/api/v1/stream", &[])
        .await
        .expect("自定义路径的明文 WS 升级");
    login(Box::pin(ws)).await;
}

/// **CDN 回源健康检查**：`GET /` 必须拿到 200。
///
/// 老实现会把这条明文 HTTP 丢给 TLS 协商，协商失败直接断连 ——
/// CDN 于是判定"源站不健康"，对外回 520 / 403，客户端连 101 都看不到。
#[tokio::test]
async fn cdn_health_check_gets_200() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port = start_server().await;
    let mut tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    tcp.write_all(b"GET / HTTP/1.1\r\nHost: frps.example.com\r\nUser-Agent: esa-health\r\n\r\n")
        .await
        .expect("发送健康检查请求");
    let mut resp = String::new();
    tcp.read_to_string(&mut resp).await.expect("读取响应");
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "CDN 健康检查必须拿到 200，实际：{resp:?}"
    );
}

/// HTTPS 回源的 CDN：健康检查走 TLS，结论必须一样（200）。
#[tokio::test]
async fn cdn_https_health_check_gets_200() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port = start_server().await;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let mut tls_stream = tls::connect_client(Box::pin(tcp), &tls_options(true))
        .await
        .expect("TLS 握手");
    tls_stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: frps.example.com\r\n\r\n")
        .await
        .expect("发送健康检查请求");
    let mut resp = String::new();
    tls_stream
        .read_to_string(&mut resp)
        .await
        .expect("读取响应");
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "HTTPS 回源的健康检查必须拿到 200，实际：{resp:?}"
    );
}

/// 带 `X-Forwarded-For` 的 CDN 回源：配了 `trusted_proxies` 才采信，
/// 采信后注册表里出来的是**真实客户端 IP**，不是 CDN 的回源地址。
#[tokio::test]
async fn trusted_proxy_uses_forwarded_for() {
    let (port, registry) = start_server_with(ServerConfig {
        bind_addr: "127.0.0.1".into(),
        token: TOKEN.to_string(),
        trusted_proxies: vec!["127.0.0.1/32".into()],
        ..Default::default()
    })
    .await;

    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let tls_stream = tls::connect_client(Box::pin(tcp), &tls_options(true))
        .await
        .expect("TLS 握手");
    let ws = ws::connect(
        tls_stream,
        "frps.example.com",
        "/any/path",
        &[("X-Forwarded-For", "203.0.113.9, 10.0.0.2")],
    )
    .await
    .expect("带 XFF 的 WSS 升级");

    let metas: HashMap<String, String> = HashMap::new();
    let (conn, run_id, _udp, _caps) = conn::client_handshake(
        Box::pin(ws),
        WireVersion::V2,
        &Credential::Token(TOKEN.to_string()),
        "cdn-client",
        "cdn-user",
        &metas,
        0,
        Default::default(),
    )
    .await
    .expect("frp 握手必须成功");

    let ip = registry
        .clients()
        .into_iter()
        .find(|c| c.run_id == run_id)
        .map(|c| c.client_ip);
    assert_eq!(
        ip,
        Some("203.0.113.9".parse().unwrap()),
        "可信反代场景下必须用 X-Forwarded-For 里的客户端地址"
    );
    drop(conn);
}

/// 没配 `trusted_proxies` 时**不能**采信 XFF —— 否则任何人都能自称任意 IP，
/// 直接绕过 IP 白黑名单。
#[tokio::test]
async fn untrusted_peer_ignores_forwarded_for() {
    let (port, registry) = start_server_with(ServerConfig {
        bind_addr: "127.0.0.1".into(),
        token: TOKEN.to_string(),
        ..Default::default()
    })
    .await;

    let tcp = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接服务端");
    let ws = ws::connect(
        Box::pin(tcp),
        "frps.example.com",
        "/any/path",
        &[("X-Forwarded-For", "203.0.113.9")],
    )
    .await
    .expect("明文 WS 升级");

    let metas: HashMap<String, String> = HashMap::new();
    let (conn, run_id, _udp, _caps) = conn::client_handshake(
        Box::pin(ws),
        WireVersion::V2,
        &Credential::Token(TOKEN.to_string()),
        "spoof-client",
        "spoof-user",
        &metas,
        0,
        Default::default(),
    )
    .await
    .expect("frp 握手必须成功");

    let ip = registry
        .clients()
        .into_iter()
        .find(|c| c.run_id == run_id)
        .map(|c| c.client_ip);
    assert_eq!(
        ip,
        Some("127.0.0.1".parse().unwrap()),
        "没白名单时必须忽略伪造的 X-Forwarded-For"
    );
    drop(conn);
}
