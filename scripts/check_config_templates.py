#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""校验仓库里的 TOML 配置模板：语法正确 + 顶层键没有被表头吞掉。

    python scripts/check_config_templates.py

为什么要查「表头归属」：TOML 里 `[acl]` / `[[roles]]` / `[[proxies]]` 之后的
裸键会归属于最近的那张表。把 `defaultRole = ""` 写在 `[[roles]]` 后面，
语法完全合法，但它会变成某个角色元素自己的字段 —— serde 静默忽略，
表现为「配置写了却不生效」。这类错误靠肉眼很难发现。
"""
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 这些键必须是顶层；出现在任何表头之后都算写错位置。
# 注意：`token` / `user` / `bindAddr` / `bindPort` / `serverPort` 这些键在
# `[auth]` / `[webServer]` / `[[visitors]]` / `[virtualNet]` 里都是合法字段，
# 不能放进白名单，否则会大量误报。这里只列**只能出现在顶层**的键。
TOP_LEVEL_KEYS = {
    "includes", "loginFailExit", "reconnectInterval", "hot_reload", "hotReload",
    "allowPorts", "maxPortsPerClient", "max_total_conns", "max_conns_per_client",
    "max_pending_per_client", "max_clients", "max_proxies_per_client",
    "defaultRole", "denyUnknown", "enablePrometheus", "enable_prometheus",
    "trusted_proxies", "ipgeo", "lb_policy", "vnet_port", "vnetPort",
    "p2p_port", "p2pPort", "p2p_enable", "xtcp_transport", "xtcpTransport",
    "quicBindPort", "kcpBindPort", "quic_bind_port", "kcp_bind_port",
    "transport_protocol", "tcp_mux", "tls_force", "heartbeat_timeout",
    "vhostHTTPPort", "vhostHTTPSPort", "tcpmuxHTTPConnectPort",
    "subdomainHost", "dashboard_port", "dashboard_user", "dashboard_pwd",
}

FENCE = re.compile(r"```toml\n(.*?)\n```", re.S)
KEY_LINE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\s*=")


def blocks_from_markdown(path):
    text = path.read_text(encoding="utf-8")
    return [(i, b) for i, b in enumerate(FENCE.findall(text), 1)]


def check_block(label, text):
    errors = []
    try:
        tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        errors.append("TOML 语法错误: %s" % e)
        return errors

    seen_header = False
    for lineno, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("["):
            seen_header = True
            continue
        m = KEY_LINE.match(stripped)
        if seen_header and m and m.group(1) in TOP_LEVEL_KEYS:
            errors.append(
                "第 %d 行: 顶层键 `%s` 写在表头之后，会被归入上一个表而不生效"
                % (lineno, m.group(1))
            )
    return errors


def main():
    targets = []
    for rel in ["README.md", "docs/CONFIG.md"]:
        targets.append((rel, blocks_from_markdown(ROOT / rel)))
    for rel in ["assets/frps.toml", "assets/frpc.toml"]:
        targets.append((rel, [(1, (ROOT / rel).read_text(encoding="utf-8"))]))

    total = bad = 0
    for rel, blocks in targets:
        for idx, block in blocks:
            total += 1
            errs = check_block("%s#%d" % (rel, idx), block)
            for e in errs:
                bad += 1
                print("  [FAIL] %s 模板块 %d: %s" % (rel, idx, e))
        print("%-24s %2d 个模板块" % (rel, len(blocks)))

    print("\n共 %d 个模板块，问题 %d 处" % (total, bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
