# -*- coding: utf-8 -*-
"""生成 README 用的三张静态 SVG 示意图（尺寸固定、白底、不依赖页面渲染器）。"""
import html
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "docs" / "images"
FONT = "system-ui,-apple-system,'Segoe UI','Microsoft YaHei',sans-serif"
INK, MUTED = '#1f2328', '#57606a'
BLUE, TEAL, AMBER = '#185FA5', '#0F6E56', '#BA7517'
LINE = '#d8dee4'


def esc(s):
    return html.escape(str(s))


def T(x, y, s, size=12, color=MUTED, weight=400, anchor='middle'):
    return (f'<text x="{x}" y="{y}" text-anchor="{anchor}" font-family="{FONT}" '
            f'font-size="{size}" font-weight="{weight}" fill="{color}">{esc(s)}</text>')


def R(x, y, w, h, fill='#ffffff', stroke=LINE, rx=10, sw=1.2, dash=''):
    d = f' stroke-dasharray="{dash}"' if dash else ''
    return (f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" '
            f'fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{d}/>')


def AR(x1, y1, x2, y2, color=MUTED):
    return (f'<path d="M{x1} {y1} L{x2} {y2}" fill="none" stroke="{color}" '
            f'stroke-width="1.8" marker-end="url(#ar)"/>')


def badge(x, y, n, color):
    return [f'<circle cx="{x}" cy="{y}" r="11" fill="{color}"/>',
            T(x, y + 4, str(n), 11, '#ffffff', 600)]


def pill(x, y, w, label, color):
    return [R(x, y, w, 22, color, color, 11, 0.6),
            T(x + w / 2, y + 15, label, 11, '#ffffff', 600)]


def head(w, h, title, right):
    return [f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img">',
            f'<title>{esc(title)}</title>',
            '<defs><marker id="ar" viewBox="0 0 10 10" refX="8" refY="5" '
            'markerWidth="6" markerHeight="6" orient="auto-start-reverse">',
            f'<path d="M2 1L8 5L2 9" fill="none" stroke="{MUTED}" stroke-width="1.5" '
            'stroke-linecap="round" stroke-linejoin="round"/></marker></defs>',
            R(0, 0, w, h, '#ffffff', '#ffffff', 12, 0),
            T(20, 27, title, 15, INK, 600, 'start'),
            T(w - 20, 27, right, 11.5, MUTED, 400, 'end'),
            f'<path d="M20 39 L{w - 20} 39" stroke="{LINE}" stroke-width="1" fill="none"/>']


def card(x, y, w, h, title, sub, color, dashed=False):
    return [R(x, y, w, h, '#ffffff', color, 9, 1.3, '5 4' if dashed else ''),
            f'<rect x="{x}" y="{y}" width="4" height="{h}" rx="2" fill="{color}"/>',
            T(x + w / 2, y + h / 2 - 7, title, 14, INK, 600),
            T(x + w / 2, y + h / 2 + 14, sub, 11.5, MUTED)]


def strip(x, y, w, h, text, fill, stroke, size=12):
    return [R(x, y, w, h, fill, stroke, 9, 1.1),
            T(x + w / 2, y + h / 2 + 4, text, size, INK, 500)]


def row(x, y, w, n, a, b, color, cx, badge_x):
    return ([R(x, y, w, 44, '#ffffff', color, 8, 1)]
            + badge(badge_x, y + 22, n, color)
            + [T(cx, y + 19, a, 12, INK, 600), T(cx, y + 35, b, 11, MUTED)])


# ---------------- 图 1：整体拓扑 ----------------
s = head(800, 400, '整体拓扑 · 内网到公网的一条隧道', 'frp v0.71.0 线协议 · 配置零改动')
s += [R(16, 50, 210, 230, '#eef5fc', '#c9dcf0', 12, 1.2),
      R(272, 50, 210, 230, '#f7f8fa', '#d0d7de', 12, 1.2, '5 4'),
      R(528, 50, 256, 230, '#e8f5f0', '#b8ddd0', 12, 1.2)]
s += [T(28, 72, '内网 / 家宽', 12, BLUE, 600, 'start'),
      T(284, 72, '公网链路（可选）', 12, MUTED, 600, 'start'),
      T(540, 72, '公网服务器', 12, TEAL, 600, 'start')]
s += card(28, 84, 186, 62, '业务服务', 'RDP 3389 · SSH 22 · Web 80', BLUE)
s += [AR(121, 146, 121, 165)]
s += card(28, 168, 186, 62, 'frpc', '官方 frpc 或本项目 frpc', BLUE)
s += pill(28, 242, 78, '无公网 IP', BLUE)
s += card(284, 84, 186, 62, 'CDN / WAF', '路径无关 · 认 WS 升级', MUTED, dashed=True)
s += card(284, 168, 186, 62, 'IP4P 解析', 'TXT / AAAA → IP:端口', MUTED)
s += pill(284, 242, 78, '可跳过', MUTED)
s += card(540, 84, 232, 62, 'frps · bindPort 7000', '同端口自动识别四种协议', TEAL)
for i, p in enumerate(['TCP', 'TLS', 'WS', 'WSS']):
    s += pill(540 + i * 58, 154, 52, p, TEAL if i >= 2 else BLUE)
s += card(540, 188, 232, 62, '控制面 + 内嵌面板', 'yamux · 流量 · 真实 IP 归属地', TEAL)
s += [AR(214, 115, 282, 115), AR(470, 115, 538, 115)]
s += [f'<path d="M121 230 L121 298 L656 298 L656 252" fill="none" stroke="{MUTED}" '
      'stroke-width="1.5" stroke-dasharray="5 4" marker-end="url(#ar)"/>',
      T(388, 286, '也可跳过 CDN 直连', 11, MUTED)]
s += strip(16, 310, 768, 44,
           '外部访问者 · remotePort / stcp visitor   ←→   隧道内字节原样往返',
           '#fff7e8', '#e8c98a', 12)
s += [f'<rect x="20" y="374" width="16" height="3" rx="1.5" fill="{BLUE}"/>',
      T(44, 379, '必经路径', 10.5, MUTED, 400, 'start'),
      f'<rect x="120" y="374" width="16" height="3" rx="1.5" fill="{MUTED}" stroke-dasharray="3 2"/>',
      T(144, 379, '可选 / 直连', 10.5, MUTED, 400, 'start'),
      f'<rect x="240" y="374" width="16" height="3" rx="1.5" fill="{TEAL}"/>',
      T(264, 379, '服务端增强（WSS 无需 nginx）', 10.5, MUTED, 400, 'start')]
s.append('</svg>')
(OUT / 'overview.svg').write_text('\n'.join(s), encoding='utf-8')

# ---------------- 图 2：两级嗅探 ----------------
s = head(800, 390, 'bindPort 同端口两级嗅探', '为什么 wss 能省掉 nginx')
s += [R(16, 50, 370, 260, '#eef5fc', '#c9dcf0', 12, 1.2),
      R(402, 50, 382, 260, '#e8f5f0', '#b8ddd0', 12, 1.2)]
s += [T(30, 72, '第 1 级 · TCP 明文层', 12, BLUE, 600, 'start'),
      T(416, 72, '第 2 级 · TLS 解密后（本项目新增）', 12, TEAL, 600, 'start')]
s += strip(30, 84, 342, 40, '新连接到达 bindPort 7000', '#ffffff', BLUE, 12.5)
for i, (a, b) in enumerate([
    ('带 Upgrade: websocket', '→ 明文 WebSocket 升级（路径不限）'),
    ('普通 HTTP 请求', '→ 回最小 HTTP 200（CDN 健康检查）'),
    ('其它字节', '→ 保留字节，进入 TLS 分支'),
]):
    s += row(30, 138 + i * 52, 342, i + 1, a, b, BLUE, 198, 48)
s += strip(416, 84, 354, 40, '0x16 标准 TLS / 0x17 frp 自定义首字节', '#ffffff', TEAL, 12)
for i, (a, b, c) in enumerate([
    ('仍是 WS 升级请求', '→ WSS 直连（官方 frps 不支持）', TEAL),
    ('不是 WS 升级请求', '→ 字节无损回放，frp over TLS / TCP', TEAL),
    ('官方 frps 只做第 1 级', '→ wss 必须前置 nginx 终结 TLS', AMBER),
]):
    s += row(416, 138 + i * 52, 354, i + 1, a, b, c, 596, 434)
s += [AR(386, 180, 400, 180), T(393, 170, '否', 10, MUTED)]
s += strip(16, 322, 768, 46,
           '四条路径最终汇入同一套 yamux + frp 控制流程；判据是「合法 WS 升级请求」，安全由 token 把守，不靠路径保密',
           '#f6f8fa', LINE, 11.5)
s.append('</svg>')
(OUT / 'port-sniff.svg').write_text('\n'.join(s), encoding='utf-8')

# ---------------- 图 3：IP4P ----------------
s = head(800, 270, 'IP4P 解析顺序', '无需任何新增配置键')
items = [
    ('IP 字面量?', '是 → 直接拨号', '零 DNS 开销', BLUE),
    ('查 TXT', 'base64 或明文', 'IP:端口', BLUE),
    ('查 AAAA', 'IP4P 编码 IPv6', '2001:0000::/16', TEAL),
    ('都未命中', '回退原地址', '与官方行为一致', TEAL),
]
for i, (t, a, b, c) in enumerate(items):
    x = 16 + i * 197
    s += [R(x, 54, 180, 118, '#f6f8fa', c, 10, 1.3),
          f'<rect x="{x}" y="54" width="4" height="118" rx="2" fill="{c}"/>']
    s += badge(x + 24, 76, i + 1, c)
    s += [T(x + 90, 81, t, 13.5, INK, 600),
          T(x + 90, 108, a, 11.5, MUTED),
          T(x + 90, 128, b, 11, c, 500)]
    if i == 1:
        s += [T(x + 90, 152, '203.0.113.7:18246', 10.5, MUTED)]
    if i < 3:
        s += [AR(x + 181, 113, x + 195, 113)]
s += strip(16, 190, 768, 42,
           '命中即覆盖 serverPort；dnsServer 可指定查询服务器；命中缓存 10 秒、未命中 60 秒',
           '#f6f8fa', LINE, 11.5)
s += [T(400, 252, 'serverAddr 为 IP 字面量时完全跳过 DNS 探测，零额外开销', 11, MUTED)]
s.append('</svg>')
(OUT / 'ip4p-flow.svg').write_text('\n'.join(s), encoding='utf-8')

for f in sorted(OUT.glob('*.svg')):
    print(f.name, f.stat().st_size, 'bytes')
