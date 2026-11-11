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
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let cfg = ServerConfig {
        bind_addr: "127.0.0.1".into(),
        token: TOKEN.to_string(),
        ..Default::default()
    };
    tokio::spawn(async move {
        if let Err(e) = serve_on(listener, Arc::new(cfg), Arc::new(Registry::unlimited())).await {
            eprintln!("测试服务端退出：{e:#}");
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
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
