#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""往当前版本 Release 上传/替换/删除资产（手动补文件的应急工具）。

正式发版请优先用 scripts/make_release.py；本脚本只用于「补传一个产物」或
「把某个已上传的资产换掉」。

用法：
    python scripts/upload_assets.py [文件...]   上传（同名已存在则先删后传）
    python scripts/upload_assets.py --delete 名称
    python scripts/upload_assets.py --tag v0.8.6 [文件...]

TAG 默认从根 Cargo.toml 推导，不再写死某个历史版本。
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

OWNER = "wengjing086"
REPO = "rust-frp-wss-ip4p"
ROOT = Path(__file__).resolve().parent.parent
API = "https://api.github.com"


def default_tag():
    for line in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            return "v" + line.split('"')[1]
    raise SystemExit("Cargo.toml 里找不到 version = ...")


def get_token():
    for key in ("GITHUB_TOKEN", "GH_TOKEN"):
        val = os.environ.get(key, "").strip()
        if val:
            return val
    for candidate in (ROOT / ".gh_token", ROOT.parent / ".gh_token"):
        if candidate.exists():
            return candidate.read_text(encoding="utf-8").strip()
    raise SystemExit("找不到 GitHub token：请设置 GITHUB_TOKEN 环境变量")


TAG = default_tag()
TOKEN = ""  # 延迟到 main() 里读，便于 --help 之类的路径不依赖 token


def call(method, url, data=None, ctype="application/json", raw=False):
    headers = {
        "Authorization": "Bearer " + TOKEN,
        "Accept": "application/vnd.github+json",
        "User-Agent": "release-uploader",
    }
    body = None
    if data is not None:
        if raw:
            body = data
            headers["Content-Type"] = ctype
        else:
            body = json.dumps(data).encode()
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            txt = r.read().decode() or "{}"
            return r.status, json.loads(txt)
    except urllib.error.HTTPError as e:
        txt = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(txt)
        except Exception:  # noqa: BLE001
            return e.code, {"raw": txt}


def get_release():
    st, rel = call("GET", "%s/repos/%s/%s/releases/tags/%s" % (API, OWNER, REPO, TAG))
    if st != 200:
        raise SystemExit("拿不到 Release: %s %s" % (st, rel))
    return rel


def delete_asset(name):
    rel = get_release()
    for a in rel.get("assets", []):
        if a["name"] == name:
            st, res = call("DELETE", "%s/repos/%s/%s/releases/assets/%d" % (API, OWNER, REPO, a["id"]))
            print("  删除 %s -> %s" % (name, st))
            return st in (204, 200)
    return False


def upload(path):
    rel = get_release()
    name = path.name
    for a in rel.get("assets", []):
        if a["name"] == name:
            delete_asset(name)
            rel = get_release()
            break
    upload_url = rel["upload_url"].split("{")[0]
    data = path.read_bytes()
    url = "%s?name=%s" % (upload_url, urllib.parse.quote(name))
    st, res = call("POST", url, data, ctype="application/octet-stream", raw=True)
    if st in (200, 201):
        print("  上传 %-34s %9d bytes" % (name, len(data)))
        return True
    print("  上传失败 %s -> %s %s" % (name, st, res))
    return False


def main():
    global TOKEN, TAG
    args = sys.argv[1:]
    if args and args[0] == "--tag":
        if len(args) < 2:
            raise SystemExit("--tag 后面要跟版本，如 --tag v0.8.6")
        TAG = args[1]
        args = args[2:]
    TOKEN = get_token()
    if not args:
        rel = get_release()
        print("Release:", rel["html_url"])
        for a in rel.get("assets", []):
            print("  %-34s %9d" % (a["name"], a["size"]))
        return 0

    if args[0] == "--delete":
        for n in args[1:]:
            delete_asset(n)
        return 0

    ok = True
    for a in args:
        p = Path(a)
        if not p.exists():
            print("  跳过（不存在）:", a)
            ok = False
            continue
        ok = upload(p) and ok
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
