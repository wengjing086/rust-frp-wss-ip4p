#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""创建/更新 GitHub Release 并上传 dist/ 下的资产。

用法：
    python scripts/make_release.py                       # TAG 从 Cargo.toml 推
    python scripts/make_release.py v0.8.7 "发布标题"     # 显式指定
    python scripts/make_release.py --check               # 只校验产物，不联网

设计约束（踩过坑，别改回去）：
  * 不硬编码本机绝对路径：token 优先读 GITHUB_TOKEN / GH_TOKEN 环境变量，
    回退到仓库**上级目录**的 .gh_token（该目录在仓库外，不会被 git 跟踪）。
  * 不硬编码 TAG：默认从根 Cargo.toml 的 version 推导，也可命令行覆盖。
  * 必选产物缺失必须报错退出，不能静默跳过 —— 否则会发出一个
    「文档写了 8 个文件、实际只有 6 个」的 Release。
  * SHA256SUMS.txt 由脚本按 dist/ 里**实际存在**的产物生成，保证三者一致。
  * Windows ARM64 是可选平台（本机可能缺 clang），缺了不算失败，
    但必须在输出里明确提示「本版未提供」。
"""

import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

OWNER = "wengjing086"
REPO = "rust-frp-wss-ip4p"
ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
NOTES = ROOT / "docs" / "RELEASE_NOTES.md"
API = "https://api.github.com"

# 必选：这三个平台缺任何一个都不能发版
REQUIRED = [
    "frps_windows_amd64.exe",
    "frpc_windows_amd64.exe",
    "frps_linux_amd64",
    "frpc_linux_amd64",
    "frps_linux_arm64",
    "frpc_linux_arm64",
]
# 可选：Windows ARM64 需要 clang + gnullvm，缺工具链时本版跳过
OPTIONAL = [
    "frps_windows_arm64.exe",
    "frpc_windows_arm64.exe",
]


def cargo_version():
    for line in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    raise SystemExit("Cargo.toml 里找不到 version = ...")


def get_token():
    for key in ("GITHUB_TOKEN", "GH_TOKEN"):
        val = os.environ.get(key, "").strip()
        if val:
            return val
    # 仓库外的工作区根目录，历史位置；不在 git 跟踪范围内
    for candidate in (ROOT / ".gh_token", ROOT.parent / ".gh_token"):
        if candidate.exists():
            return candidate.read_text(encoding="utf-8").strip()
    raise SystemExit(
        "找不到 GitHub token：请设置环境变量 GITHUB_TOKEN，"
        "或把 token 放到 %s" % (ROOT.parent / ".gh_token")
    )


TOKEN = ""  # 延迟到真正需要联网时才读，便于 --check 离线运行


def req(method, url, data=None, content_type="application/json", raw=False):
    headers = {
        "Authorization": "Bearer " + TOKEN,
        "Accept": "application/vnd.github+json",
        "User-Agent": "release-script",
    }
    body = None
    if data is not None:
        if raw:
            body = data
            headers["Content-Type"] = content_type
        else:
            body = json.dumps(data).encode("utf-8")
            headers["Content-Type"] = "application/json"
    r = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=120) as resp:
            return resp.status, json.loads(resp.read().decode("utf-8") or "{}")
    except urllib.error.HTTPError as e:
        txt = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(txt)
        except Exception:  # noqa: BLE001
            return e.code, {"raw": txt}


def release_body(notes_text, tag):
    """只取 changelog 里属于本 tag 的那一段作为 Release 正文。

    以前直接把整份 `docs/RELEASE_NOTES.md` 塞进 body，结果是**每个 Release
    都重复携带此前所有版本的说明**（v0.8.7 页面 12.8k 字符里只有开头的
    一部分是当前版本），而且文件末尾那张描述「当前产物」的下载表也被复制
    进了历史 Release，出现「v0.8.2 页面在讲 v0.8.7 缺 Windows ARM64」这种错位。
    历史 changelog 仍完整留在 docs/ 里，body 只放本版 + 一个指回仓库的链接。
    """
    ver = tag.lstrip("v")
    lines = notes_text.splitlines()
    start = next(
        (i for i, line in enumerate(lines) if line.startswith("## v" + ver)), None
    )
    if start is None:
        raise SystemExit(
            "RELEASE_NOTES.md 里找不到 %s 段落，拒绝发布空/全文 body" % tag
        )
    end = next(
        (j for j in range(start + 1, len(lines)) if lines[j].startswith("## ")),
        len(lines),
    )
    body = "\n".join(lines[start:end]).strip()
    body = re.sub(r"\n+---\s*$", "", body)  # 去掉段落末尾的分隔线
    # 链接固定指 main：changelog 是累积文档，tag 里那份是发布当时的快照，
    # 可能还没经过后续修订；main 上才是最新且完整的版本。
    body += (
        "\n\n---\n\n下载见本 Release 资产与 `SHA256SUMS.txt`；完整更新日志："
        "https://github.com/%s/%s/blob/main/docs/RELEASE_NOTES.md" % (OWNER, REPO)
    )
    return body


def collect_assets():
    missing = [n for n in REQUIRED if not (DIST / n).is_file()]
    if missing:
        raise SystemExit("必选产物缺失，拒绝发版: " + ", ".join(missing))
    present = list(REQUIRED) + [n for n in OPTIONAL if (DIST / n).is_file()]
    skipped = [n for n in OPTIONAL if n not in present]
    for n in present:
        size = (DIST / n).stat().st_size
        if size < 1024 * 1024:
            raise SystemExit("产物异常过小（%s = %d bytes），疑似构建失败" % (n, size))
        print("  %-32s %8d bytes" % (n, size))
    if skipped:
        print("  本版未提供（构建链缺 clang）: " + ", ".join(skipped))
    return present, skipped


def write_checksums(names):
    lines = []
    for n in names:
        h = hashlib.sha256((DIST / n).read_bytes()).hexdigest()
        lines.append("%s *%s" % (h, n))
    (DIST / "SHA256SUMS.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("  已生成 SHA256SUMS.txt（%d 项）" % len(lines))


def main(argv):
    global TOKEN
    if "--check" in argv:
        argv = [a for a in argv if a != "--check"]
        print("离线校验产物（不联网）")
        present, _ = collect_assets()
        write_checksums(present)
        tag = "v" + cargo_version()
        body = release_body(NOTES.read_text(encoding="utf-8"), tag)
        print("  %s 正文 = %d 字符（仅本版本段落）" % (tag, len(body)))
        return 0

    tag = argv[0] if argv else "v" + cargo_version()
    name = argv[1] if len(argv) > 1 else tag
    print("TAG =", tag, " 版本 =", cargo_version())
    if not (ROOT / "Cargo.toml").read_text(encoding="utf-8").count(
        'version = "%s"' % tag.lstrip("v")
    ):
        print("!! 警告: TAG 与 Cargo.toml 版本不一致，请确认")

    print("1) 校验产物")
    present, _ = collect_assets()
    write_checksums(present)

    TOKEN = get_token()
    body = release_body(NOTES.read_text(encoding="utf-8"), tag)
    print("   Release 正文 = %d 字符（仅 %s 段落）" % (len(body), tag))

    status, rel = req("GET", "%s/repos/%s/%s/releases/tags/%s" % (API, OWNER, REPO, tag))
    if status == 200:
        print("2) 已存在 Release:", rel.get("html_url"))
        # 幂等重跑时把正文同步成最新 changelog（修正过长的旧 body 也靠这一步）
        st, res = req(
            "PATCH",
            "%s/repos/%s/%s/releases/%d" % (API, OWNER, REPO, rel["id"]),
            {"body": body},
        )
        print("   正文已同步:", "OK" if st == 200 else "失败 %s %s" % (st, res))
        if st != 200:
            return 1
    else:
        status, rel = req(
            "POST",
            "%s/repos/%s/%s/releases" % (API, OWNER, REPO),
            {
                "tag_name": tag,
                "name": name,
                "body": body,
                "draft": False,
                "prerelease": False,
            },
        )
        if status not in (200, 201):
            print("创建 Release 失败:", status, rel)
            return 1
        print("2) 已创建 Release:", rel.get("html_url"))

    upload_url = rel["upload_url"].split("{")[0]
    existing = {a["name"] for a in rel.get("assets", [])}
    for n in present + ["SHA256SUMS.txt"]:
        path = DIST / n
        if n in existing:
            print("   跳过（已存在）:", n)
            continue
        data = path.read_bytes()
        url = "%s?name=%s" % (upload_url, urllib.parse.quote(n))
        status, res = req(
            "POST", url, data, content_type="application/octet-stream", raw=True
        )
        if status in (200, 201):
            print("   已上传: %s (%d bytes)" % (n, len(data)))
        else:
            print("   上传失败: %s -> %s %s" % (n, status, res))
            return 1

    status, rel = req("GET", "%s/repos/%s/%s/releases/tags/%s" % (API, OWNER, REPO, tag))
    print("\nRelease 页面:", rel.get("html_url"))
    for a in rel.get("assets", []):
        print("  - %-32s %8d bytes" % (a["name"], a["size"]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
