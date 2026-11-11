"""真实二进制的端到端功能测试。

为什么不用 cargo test：单元/e2e 测试都是**在进程内**构造 Registry 与 FrpConn，
绕过了配置文件解析、CLI 启动、服务端监听与面板路由。用户实际遇到的 bug
（stcp 加密、ws 重复建帧、代理永远显示在线）全都只在真实进程里出现。
"""
import base64
import json
import os
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import urllib.error
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVER = ROOT / "target" / "debug" / "nfrp-server.exe"
CLIENT = ROOT / "target" / "debug" / "nfrp-client.exe"
WORK = Path(tempfile.mkdtemp(prefix="nfrp_e2e_"))
TOKEN = "e2etoken-9f3a"

BASE = 20000  # 端口基址，测试期间独占一段
P_CONTROL = BASE + 0
P_VHOST_HTTP = BASE + 1
P_VHOST_HTTPS = BASE + 2
P_TCPMUX = BASE + 3
P_DASH = BASE + 4
P_TCP = BASE + 10
P_UDP = BASE + 11
P_STCP = BASE + 12
P_XTCP = BASE + 13
P_SUDP = BASE + 14

results = []
procs = []


def log(msg):
    print(msg, flush=True)


def record(name, ok, detail=""):
    results.append((name, ok, detail))
    log(f"  [{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))


def free_port_hint():
    pass


def wait_port(port, host="127.0.0.1", timeout=15.0):
    end = time.time() + timeout
    while time.time() < end:
        try:
            with socket.create_connection((host, port), timeout=0.4):
                return True
        except OSError:
            time.sleep(0.1)
    return False


def start_proc(args, logfile, cwd=None):
    f = open(logfile, "wb")
    kw = {}
    if sys.platform == "win32":
        kw["creationflags"] = 0x08000000  # CREATE_NO_WINDOW
    p = subprocess.Popen(args, stdout=f, stderr=subprocess.STDOUT, cwd=cwd or str(WORK), **kw)
    procs.append((p, f, logfile))
    return p


def stop_all():
    for p, f, _ in procs:
        try:
            p.terminate()
        except Exception:
            pass
    time.sleep(0.3)
    for p, f, _ in procs:
        try:
            if p.poll() is None:
                p.kill()
        except Exception:
            pass
        try:
            f.close()
        except Exception:
            pass
    procs.clear()


def write_cfg(name, text):
    p = WORK / name
    p.write_text(text, encoding="utf-8")
    return p


# ---------------- 后端服务 ----------------

class TcpEcho(threading.Thread):
    """回显 + 前缀，用来确认连到的确实是这条代理指向的后端。"""

    def __init__(self, tag):
        super().__init__(daemon=True)
        self.tag = tag
        self.sock = socket.socket()
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.port = self.sock.getsockname()[1]
        self.end = False

    def run(self):
        while not self.end:
            try:
                c, _ = self.sock.accept()
            except OSError:
                return
            threading.Thread(target=self.handle, args=(c,), daemon=True).start()

    def handle(self, c):
        try:
            c.sendall(f"{self.tag}:".encode())
            while True:
                d = c.recv(4096)
                if not d:
                    break
                c.sendall(d)
        except OSError:
            pass
        finally:
            c.close()


class UdpEcho(threading.Thread):
    def __init__(self, tag):
        super().__init__(daemon=True)
        self.tag = tag
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.port = self.sock.getsockname()[1]
        self.end = False

    def run(self):
        while not self.end:
            try:
                d, a = self.sock.recvfrom(65535)
            except OSError:
                return
            try:
                self.sock.sendto(f"{self.tag}:".encode() + d, a)
            except OSError:
                pass


class HttpBackend(threading.Thread):
    """最小 HTTP/1.1 服务，返回 tag 与请求路径。"""

    def __init__(self, tag, tls_ctx=None):
        super().__init__(daemon=True)
        self.tag = tag
        self.ctx = tls_ctx
        self.sock = socket.socket()
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.port = self.sock.getsockname()[1]
        self.end = False

    def run(self):
        while not self.end:
            try:
                c, _ = self.sock.accept()
            except OSError:
                return
            threading.Thread(target=self.handle, args=(c,), daemon=True).start()

    def handle(self, c):
        try:
            if self.ctx:
                c = self.ctx.wrap_socket(c, server_side=True)
            buf = b""
            while b"\r\n\r\n" not in buf:
                d = c.recv(4096)
                if not d:
                    return
                buf += d
            head = buf.split(b"\r\n\r\n")[0].decode("latin-1")
            lines = head.split("\r\n")
            path = lines[0].split(" ")[1] if " " in lines[0] else "/"
            host = ""
            for ln in lines[1:]:
                if ln.lower().startswith("host:"):
                    host = ln.split(":", 1)[1].strip()
            # 回显 Host 便于验证校验 host_header_rewrite
            body = f"{self.tag}|path={path}|host={host}"
            resp = (
                "HTTP/1.1 200 OK\r\n"
                f"Content-Length: {len(body)}\r\n"
                f"X-Backend: {self.tag}\r\n"
                "Connection: close\r\n\r\n" + body
            )
            c.sendall(resp.encode())
        except OSError:
            pass
        finally:
            try:
                c.close()
            except Exception:
                pass


def http_get(port, host_header, path="/", extra=None, timeout=6, https=False):
    s = socket.create_connection(("127.0.0.1", port), timeout=timeout)
    if https:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        s = ctx.wrap_socket(s, server_hostname=host_header)
    req = f"GET {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n"
    for k, v in (extra or {}).items():
        req += f"{k}: {v}\r\n"
    req += "\r\n"
    s.sendall(req.encode())
    data = b""
    while True:
        d = s.recv(8192)
        if not d:
            break
        data += d
    s.close()
    text = data.decode("latin-1", "replace")
    status = int(text.split(" ")[1]) if text.startswith("HTTP/") else 0
    body = text.split("\r\n\r\n", 1)[1] if "\r\n\r\n" in text else ""
    return status, body


def api(path, port=P_DASH, user="admin", pwd="admin", method="GET", body=None):
    url = f"http://127.0.0.1:{port}{path}"
    req = urllib.request.Request(url, method=method)
    if user:
        tok = base64.b64encode(f"{user}:{pwd}".encode()).decode()
        req.add_header("Authorization", f"Basic {tok}")
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, data=data, timeout=8) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except Exception as e:
        return 0, str(e)


# ---------------- frps / frpc 配置 ----------------

FRPS_TOML = f"""
bind_addr = "127.0.0.1"
bind_port = {P_CONTROL}
token = "{TOKEN}"
vhost_http_port = {P_VHOST_HTTP}
vhost_https_port = {P_VHOST_HTTPS}
tcpmux_http_connect_port = {P_TCPMUX}
dashboard_port = {P_DASH}
dashboard_user = "admin"
dashboard_pwd = "admin"
log_level = "info"
heartbeat_timeout = 90
"""


def frpc_toml(extra_proxies="", extra_visitors="", transport=""):
    return f"""
server_addr = "127.0.0.1"
server_port = {P_CONTROL}
token = "{TOKEN}"
tcp_mux = true
pool_count = 1
log_level = "info"
{transport}
{extra_proxies}
{extra_visitors}
"""


def roundtrip_tcp(port, payload=b"hello-e2e", timeout=8):
    s = socket.create_connection(("127.0.0.1", port), timeout=timeout)
    s.settimeout(timeout)
    try:
        prefix = b""
        while b":" not in prefix:
            d = s.recv(64)
            if not d:
                return b""
            prefix += d
        tag, rest = prefix.split(b":", 1)
        s.sendall(payload)
        got = rest
        while len(got) < len(payload):
            d = s.recv(4096)
            if not d:
                break
            got += d
        return tag + b":" + got
    finally:
        s.close()


def roundtrip_udp(port, payload=b"hello-udp", timeout=6):
    u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    u.settimeout(timeout)
    try:
        u.sendto(payload, ("127.0.0.1", port))
        d, _ = u.recvfrom(65535)
        return d
    finally:
        u.close()


def stage1_core_types():
    """核心代理类型：tcp / udp / http / https / tcpmux / stcp / xtcp / sudp"""
    log("\n=== 阶段 1：核心代理类型 ===")
    tcp_be = TcpEcho("tcp"); tcp_be.start()
    udp_be = UdpEcho("udp"); udp_be.start()
    http_be = HttpBackend("http"); http_be.start()
    https_be = HttpBackend("https", tls_ctx=make_tls_ctx()); https_be.start()
    mux_be = TcpEcho("mux"); mux_be.start()
    stcp_be = TcpEcho("stcp"); stcp_be.start()
    xtcp_be = TcpEcho("xtcp"); xtcp_be.start()
    sudp_be = UdpEcho("sudp"); sudp_be.start()

    frps = write_cfg("frps1.toml", FRPS_TOML)
    proxies = f"""
[[proxies]]
name = "p_tcp"
type = "tcp"
local_addr = "127.0.0.1:{tcp_be.port}"
remote_port = {P_TCP}

[[proxies]]
name = "p_udp"
type = "udp"
local_addr = "127.0.0.1:{udp_be.port}"
remote_port = {P_UDP}

[[proxies]]
name = "p_http"
type = "http"
local_addr = "127.0.0.1:{http_be.port}"
custom_domains = ["web.e2e.local"]

[[proxies]]
name = "p_https"
type = "https"
local_addr = "127.0.0.1:{https_be.port}"
custom_domains = ["tls.e2e.local"]

[[proxies]]
name = "p_mux"
type = "tcpmux"
multiplexer = "httpconnect"
local_addr = "127.0.0.1:{mux_be.port}"
custom_domains = ["mux.e2e.local"]

[[proxies]]
name = "p_stcp"
type = "stcp"
local_addr = "127.0.0.1:{stcp_be.port}"
secret_key = "sk-stcp"

[[proxies]]
name = "p_xtcp"
type = "xtcp"
local_addr = "127.0.0.1:{xtcp_be.port}"
secret_key = "sk-xtcp"

[[proxies]]
name = "p_sudp"
type = "sudp"
local_addr = "127.0.0.1:{sudp_be.port}"
secret_key = "sk-sudp"
"""
    visitors = f"""
[[visitors]]
name = "v_stcp"
type = "stcp"
server_name = "p_stcp"
secret_key = "sk-stcp"
bind_addr = "127.0.0.1"
bind_port = {P_STCP}

[[visitors]]
name = "v_xtcp"
type = "xtcp"
server_name = "p_xtcp"
secret_key = "sk-xtcp"
bind_addr = "127.0.0.1"
bind_port = {P_XTCP}

[[visitors]]
name = "v_sudp"
type = "sudp"
server_name = "p_sudp"
secret_key = "sk-sudp"
bind_addr = "127.0.0.1"
bind_port = {P_SUDP}
"""
    frpc = write_cfg("frpc1.toml", frpc_toml(proxies, visitors))

    start_proc([str(SERVER), "-c", str(frps)], WORK / "frps1.log")
    if not wait_port(P_CONTROL):
        record("frps 启动", False, "控制端口没监听")
        return False
    record("frps 启动", True)
    start_proc([str(CLIENT), "-c", str(frpc)], WORK / "frpc1.log")
    time.sleep(3.5)

    # --- tcp ---
    try:
        got = roundtrip_tcp(P_TCP)
        record("tcp 代理", got == b"tcp:hello-e2e", f"收到 {got!r}")
    except Exception as e:
        record("tcp 代理", False, repr(e))

    # --- udp ---
    try:
        got = roundtrip_udp(P_UDP)
        record("udp 代理", got == b"udp:hello-udp", f"收到 {got!r}")
    except Exception as e:
        record("udp 代理", False, repr(e))

    # --- http vhost ---
    try:
        st, body = http_get(P_VHOST_HTTP, "web.e2e.local", "/abc")
        ok = st == 200 and body.startswith("http|path=/abc")
        record("http 代理（域名+路径）", ok, f"status={st} body={body[:60]!r}")
    except Exception as e:
        record("http 代理（域名+路径）", False, repr(e))

    # --- http 未知域名应 404 ---
    try:
        st, body = http_get(P_VHOST_HTTP, "nope.e2e.local", "/")
        record("http 未知域名回 404", st == 404, f"status={st}")
    except Exception as e:
        record("http 未知域名回 404", False, repr(e))

    # --- https vhost ---
    try:
        st, body = http_get(P_VHOST_HTTPS, "tls.e2e.local", "/x", https=True)
        ok = st == 200 and body.startswith("https|")
        record("https 代理（SNI 透传）", ok, f"status={st} body={body[:60]!r}")
    except Exception as e:
        record("https 代理（SNI 透传）", False, repr(e))

    # --- tcpmux ---
    try:
        s = socket.create_connection(("127.0.0.1", P_TCPMUX), timeout=8)
        s.settimeout(8)
        s.sendall(b"CONNECT mux.e2e.local:80 HTTP/1.1\r\nHost: mux.e2e.local:80\r\n\r\n")
        buf = b""
        while b"\r\n\r\n" not in buf:
            d = s.recv(4096)
            if not d:
                break
            buf += d
        ok = b"200" in buf.split(b"\r\n")[0]
        s.sendall(b"ping")
        got = s.recv(64)
        s.close()
        record("tcpmux 代理（HTTP CONNECT）", ok and got == b"mux:ping", f"{buf[:40]!r} {got!r}")
    except Exception as e:
        record("tcpmux 代理（HTTP CONNECT）", False, repr(e))

    # --- stcp ---
    try:
        got = roundtrip_tcp(P_STCP)
        record("stcp 访客隧道", got == b"stcp:hello-e2e", f"收到 {got!r}")
    except Exception as e:
        record("stcp 访客隧道", False, repr(e))

    # --- xtcp（无 p2p_port 时退化为中继）---
    try:
        got = roundtrip_tcp(P_XTCP)
        record("xtcp 访客隧道（回退中继）", got == b"xtcp:hello-e2e", f"收到 {got!r}")
    except Exception as e:
        record("xtcp 访客隧道（回退中继）", False, repr(e))

    # --- sudp ---
    try:
        got = roundtrip_udp(P_SUDP)
        record("sudp 访客隧道", got == b"sudp:hello-udp", f"收到 {got!r}")
    except Exception as e:
        record("sudp 访客隧道", False, repr(e))

    # 记住状态供阶段 2 用
    globals()["_stage1_procs"] = True
    return True


def make_tls_ctx():
    """生成自签证书并返回 server 侧 SSLContext（https 后端与插件都要用）。"""
    crt = WORK / "e2e.crt"
    key = WORK / "e2e.key"
    if not crt.exists():
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-keyout", str(key), "-out", str(crt), "-days", "2",
             "-subj", "/CN=e2e.local",
             "-addext", "subjectAltName=DNS:e2e.local,DNS:tls.e2e.local,DNS:web.e2e.local"],
            check=True, capture_output=True,
        )
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(str(crt), str(key))
    return ctx


def stage2_dashboard():
    """面板 API：v1 / v2 / 官方兼容接口"""
    log("\n=== 阶段 2：面板 API ===")
    checks = [
        ("/api/healthz 免鉴权", "/api/healthz", "text"),
        ("/api/status", "/api/status", "status"),
        ("/api/v2/status", "/api/v2/status", "status"),
        ("/api/v2/clients", "/api/v2/clients", "clients"),
        ("/api/v2/proxies", "/api/v2/proxies", "proxies"),
        ("/api/v2/visitors", "/api/v2/visitors?all=1", "visitors"),
        ("/api/v2/ports", "/api/v2/ports", "ports"),
        ("/api/v2/audit", "/api/v2/audit?all=1", "audit"),
        ("/api/v2/version", "/api/v2/version", "version"),
        ("官方 /api/serverinfo", "/api/serverinfo", "serverinfo"),
        ("官方 /api/proxy/tcp", "/api/proxy/tcp", "proxy_tcp"),
        ("官方 /api/proxy/stcp", "/api/proxy/stcp", "proxy_stcp"),
        ("官方 /api/traffic/p_tcp", "/api/traffic/p_tcp", "traffic"),
        ("官方 /api/clients", "/api/clients", "clients"),
        ("官方 /api/v2/system/info", "/api/v2/system/info", "sysinfo"),
        ("官方 /api/v2/clients", "/api/v2/clients", "clients2"),
        ("官方 /api/v2/proxies", "/api/v2/proxies", "proxies2"),
    ]
    for name, path, _kind in checks:
        st, body = api(path)
        if _kind == "text":
            # healthz 按契约是纯文本 ok，不是 JSON
            ok = st == 200 and body.strip() == "ok"
            record(name, ok, f"status={st} body={body.strip()!r}")
            continue
        ok = st == 200 and body.strip().startswith(("{", "["))
        detail = f"status={st}"
        if not ok:
            detail += f" body={body[:200]!r}"
        elif _kind == "proxy_tcp":
            # 官方契约：数组里每项的 spec 必须有 type 判别字段
            try:
                data = json.loads(body)
                arr = data.get("proxies", []) if isinstance(data, dict) else data
                if arr:
                    first = arr[0]
                    # 官方契约：顶层要有 name/type，便于脚本直接筛
                    t = first.get("type") or (first.get("conf") or {}).get("type")
                    nm = first.get("name") or (first.get("conf") or {}).get("name")
                    missing = [k for k in ("name", "type") if not first.get(k)]
                    if missing:
                        ok = False
                        detail += f" 顶层缺 {missing}（只有 conf 里有 name={nm} type={t}）"
                    else:
                        detail += f" name={nm} type={t}"
                else:
                    ok = False
                    detail += " proxies 为空"
            except Exception as e:
                ok = False
                detail += f" JSON 解析失败({e}) 原始={body[:300]!r}"
        elif _kind == "traffic":
            try:
                arr = json.loads(body)
                detail += f" 天数={len(arr)}"
            except Exception as e:
                ok = False
                detail += f" JSON 解析失败 {e}"
        record(name, ok, detail)

    # 未鉴权必须 401
    st, _ = api("/api/v2/status", user=None)
    record("面板未鉴权拒绝", st == 401, f"status={st}")


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def run_transport_case(name, server_extra, client_transport, timeout=25, client_port=None):
    """一套独立的 frps + frpc，只跑一条 tcp 代理，验证某种传输层。"""
    be = TcpEcho("t"); be.start()
    ctrl = free_port()
    rport = free_port()
    sp = WORK / f"frps_{name}.toml"
    sp.write_text(
        f'binding_placeholder = 0\nbind_addr = "127.0.0.1"\nbind_port = {ctrl}\n'
        f'token = "{TOKEN}"\nlog_level = "info"\n{server_extra}\n', encoding="utf-8")
    cp = WORK / f"frpc_{name}.toml"
    cp.write_text(
        f'server_addr = "127.0.0.1"\nserver_port = {client_port or ctrl}\ntoken = "{TOKEN}"\n'
        f'log_level = "info"\ntcp_mux = true\n{client_transport}\n'
        f'[[proxies]]\nname = "tp"\ntype = "tcp"\nlocal_addr = "127.0.0.1:{be.port}"\n'
        f'remote_port = {rport}\n', encoding="utf-8")

    local_procs = []
    f1 = open(WORK / f"frps_{name}.log", "wb")
    p1 = subprocess.Popen([str(SERVER), "-c", str(sp)], stdout=f1, stderr=subprocess.STDOUT,
                          cwd=str(WORK), creationflags=0x08000000 if sys.platform == "win32" else 0)
    local_procs.append((p1, f1))
    time.sleep(1.5)
    f2 = open(WORK / f"frpc_{name}.log", "wb")
    p2 = subprocess.Popen([str(CLIENT), "-c", str(cp)], stdout=f2, stderr=subprocess.STDOUT,
                          cwd=str(WORK), creationflags=0x08000000 if sys.platform == "win32" else 0)
    local_procs.append((p2, f2))
    time.sleep(5)

    ok = False
    detail = ""
    try:
        got = roundtrip_tcp(rport, timeout=10)
        ok = got == b"t:hello-e2e"
        detail = f"收到 {got!r}"
    except Exception as e:
        detail = repr(e)

    for p, f in local_procs:
        try:
            p.terminate(); p.kill()
        except Exception:
            pass
        try:
            f.close()
        except Exception:
            pass
    be.end = True
    try:
        be.sock.close()
    except Exception:
        pass
    record(name, ok, detail)
    if not ok:
        # 失败时把两侧日志的关键行打出来，避免再手工复现
        for tag, path in (("frps", WORK / f"frps_{name}.log"), ("frpc", WORK / f"frpc_{name}.log")):
            try:
                lines = [ln for ln in path.read_text(encoding="utf-8", errors="replace").splitlines()
                         if any(k in ln for k in ("WARN", "ERROR", "错误", "失败", "握手", "建立", "登录", "代理"))]
                log(f"    --- {tag} 日志（关键行）---")
                for ln in lines[-12:]:
                    log(f"    {ln}")
            except Exception as e:
                log(f"    {tag} 日志读取失败：{e}")
    return ok


def stage3_transports():
    log("\n=== 阶段 3：传输层 ===")
    run_transport_case("tcp 明文", "", "")
    run_transport_case("TLS 传输", "", "tls_enable = true")
    run_transport_case("WebSocket 传输", "", "websocketEnable = true")
    run_transport_case("WSS 传输", "", "websocketEnable = true\ntls_enable = true")
    kcp_port = free_port()
    # 官方语义：配了 kcpBindPort 之后，客户端必须把 serverPort 指向那个端口
    run_transport_case("KCP 传输", f"kcpBindPort = {kcp_port}",
                       'transport_protocol = "kcp"', client_port=kcp_port)
    run_transport_case("QUIC 传输", 'transport_protocol = "quic"',
                       'transport_protocol = "quic"')


def stage4_advanced():
    """加密压缩 / http 鉴权 / 路径路由 / 插件 / group 负载均衡 / 客户端面板 / 断线重连"""
    log("\n=== 阶段 4：进阶功能 ===")
    enc_be = TcpEcho("enc"); enc_be.start()
    auth_be = HttpBackend("auth"); auth_be.start()
    loc_be = HttpBackend("loc"); loc_be.start()
    g1_be = TcpEcho("g1"); g1_be.start()
    g2_be = TcpEcho("g2"); g2_be.start()

    static_dir = WORK / "site"
    static_dir.mkdir(exist_ok=True)
    (static_dir / "index.html").write_text("STATIC-OK", encoding="utf-8")

    ctrl = free_port()
    r_enc, r_static, r_group, r_cli_panel = free_port(), free_port(), free_port(), free_port()
    vh = free_port()

    dash4 = free_port()
    sp = WORK / "frps4.toml"
    sp.write_text(
        f'bind_addr = "127.0.0.1"\nbind_port = {ctrl}\ntoken = "{TOKEN}"\n'
        f'vhost_http_port = {vh}\ndashboard_port = {dash4}\n'
        f'dashboard_user = "admin"\ndashboard_pwd = "admin"\nlog_level = "info"\n', encoding="utf-8")

    cp = WORK / "frpc4.toml"
    cp.write_text(f"""
server_addr = "127.0.0.1"
server_port = {ctrl}
token = "{TOKEN}"
tcp_mux = true
log_level = "info"

[webServer]
addr = "127.0.0.1"
port = {r_cli_panel}

[[proxies]]
name = "p_enc"
type = "tcp"
local_addr = "127.0.0.1:{enc_be.port}"
remote_port = {r_enc}
transport.useEncryption = true
transport.useCompression = true

[[proxies]]
name = "p_auth"
type = "http"
local_addr = "127.0.0.1:{auth_be.port}"
custom_domains = ["auth.e2e.local"]
http_user = "u1"
http_pwd = "p1"

[[proxies]]
name = "p_loc"
type = "http"
local_addr = "127.0.0.1:{loc_be.port}"
custom_domains = ["loc.e2e.local"]
locations = ["/api"]

[[proxies]]
name = "p_static"
type = "tcp"
remote_port = {r_static}
plugin = "static_file"
plugin_local_path = "{static_dir.as_posix()}"

[[proxies]]
name = "g_a"
type = "tcp"
local_addr = "127.0.0.1:{g1_be.port}"
remote_port = {r_group}
group = "grp"
group_key = "gk"

[[proxies]]
name = "g_b"
type = "tcp"
local_addr = "127.0.0.1:{g2_be.port}"
remote_port = {r_group}
group = "grp"
group_key = "gk"
""", encoding="utf-8")

    f1 = open(WORK / "frps4.log", "wb")
    p1 = subprocess.Popen([str(SERVER), "-c", str(sp)], stdout=f1, stderr=subprocess.STDOUT,
                          cwd=str(WORK), creationflags=0x08000000 if sys.platform == "win32" else 0)
    procs.append((p1, f1, WORK / "frps4.log"))
    time.sleep(1.5)
    f2 = open(WORK / "frpc4.log", "wb")
    p2 = subprocess.Popen([str(CLIENT), "-c", str(cp)], stdout=f2, stderr=subprocess.STDOUT,
                          cwd=str(WORK), creationflags=0x08000000 if sys.platform == "win32" else 0)
    procs.append((p2, f2, WORK / "frpc4.log"))
    time.sleep(4)

    # 加密 + 压缩
    try:
        got = roundtrip_tcp(r_enc)
        record("tcp 加密+压缩", got == b"enc:hello-e2e", f"收到 {got!r}")
    except Exception as e:
        record("tcp 加密+压缩", False, repr(e))

    # http Basic Auth
    try:
        st_no, _ = http_get(vh, "auth.e2e.local", "/")
        tok = base64.b64encode(b"u1:p1").decode()
        st_ok, body = http_get(vh, "auth.e2e.local", "/", {"Authorization": f"Basic {tok}"})
        record("http Basic Auth", st_no == 401 and st_ok == 200,
               f"无凭据={st_no} 有凭据={st_ok}")
    except Exception as e:
        record("http Basic Auth", False, repr(e))

    # 路径前缀路由
    try:
        st_hit, body = http_get(vh, "loc.e2e.local", "/api/x")
        st_miss, _ = http_get(vh, "loc.e2e.local", "/other")
        ok = st_hit == 200 and "path=/api/x" in body and st_miss == 404
        record("http 路径路由 locations", ok, f"/api/x={st_hit} /other={st_miss}")
    except Exception as e:
        record("http 路径路由 locations", False, repr(e))

    # 插件 static_file
    try:
        st, body = http_get(r_static, "anything.local", "/index.html")
        record("插件 static_file", st == 200 and "STATIC-OK" in body, f"status={st} body={body[:40]!r}")
    except Exception as e:
        record("插件 static_file", False, repr(e))

    # group 负载均衡：多次连接应落到两个不同后端
    try:
        tags = set()
        for _ in range(8):
            got = roundtrip_tcp(r_group)
            if b":" in got:
                tags.add(got.split(b":")[0])
        record("group 负载均衡", tags == {b"g1", b"g2"}, f"命中后端={sorted(tags)}")
    except Exception as e:
        record("group 负载均衡", False, repr(e))

    # 客户端本地面板
    st, body = api("/api/status", port=r_cli_panel, user=None)
    record("frpc 本地面板 /api/status", st == 200, f"status={st}")

    # 服务端面板该能看到 group 与 plugin 代理
    st, body = api("/api/v2/proxies", port=dash4)
    try:
        data = json.loads(body)
        items = data.get("data", {}).get("items", []) if isinstance(data, dict) else []
        names = {i.get("name") for i in items}
        record("面板列出全部代理", {"p_enc", "p_auth", "p_loc", "p_static", "g_a", "g_b"} <= names,
               f"看到 {sorted(n for n in names if n)}")
    except Exception as e:
        record("面板列出全部代理", False, repr(e))

    # 断线重连：杀掉 frpc 再起，代理应恢复
    try:
        p2.terminate()
        time.sleep(1.5)
        f3 = open(WORK / "frpc4b.log", "wb")
        p3 = subprocess.Popen([str(CLIENT), "-c", str(cp)], stdout=f3, stderr=subprocess.STDOUT,
                              cwd=str(WORK), creationflags=0x08000000 if sys.platform == "win32" else 0)
        procs.append((p3, f3, WORK / "frpc4b.log"))
        time.sleep(4.5)
        got = roundtrip_tcp(r_enc)
        record("frpc 重启后代理恢复", got == b"enc:hello-e2e", f"收到 {got!r}")
    except Exception as e:
        record("frpc 重启后代理恢复", False, repr(e))


def main():
    only = sys.argv[1] if len(sys.argv) > 1 else "all"
    log(f"工作目录：{WORK}")
    try:
        if only in ("all", "1"):
            if stage1_core_types():
                stage2_dashboard()
            stop_all()
        if only in ("all", "3"):
            stage3_transports()
            stop_all()
        if only in ("all", "4"):
            stage4_advanced()
            stop_all()
    finally:
        stop_all()
    log("\n================ 汇总 ================")
    passed = sum(1 for _, ok, _ in results if ok)
    failed = [r for r in results if not r[1]]
    log(f"通过 {passed} / {len(results)}")
    for name, _, detail in failed:
        log(f"  FAIL {name}: {detail}")
    log(f"\n日志目录：{WORK}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
