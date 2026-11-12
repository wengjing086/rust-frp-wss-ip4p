# CI / Release 工作流

`ci/ci.yml` 与 `ci/release.yml` 是**随源码入库**的完整配置。
`.github/workflows/` 里放着逐字节相同的副本，但**目前 GitHub 上尚未启用**（见下）。

| 文件 | 触发 | 作用 |
|---|---|---|
| `ci.yml` | push / PR / 手动 | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`；Linux / Windows / macOS 三平台 `cargo build --release` + 可执行性 smoke |
| `release.yml` | 推 `v*` tag / 手动 | **可复现性校验构建**：产出与正式资产同名的独立可执行文件 + `SHA256SUMS.txt`，上传为 Actions artifacts，**不写 GitHub Release** |

## ⚠️ 当前状态：配置有了，但还没生效

创建仓库时用的 Personal Access Token 只有 `repo` 权限，没有 `workflow` scope。
GitHub 会拒绝任何在 `.github/workflows/` 下新增/修改文件的 push：

```
! [remote rejected] main -> main (refusing to allow a Personal Access Token to
  create or update workflow `.github/workflows/ci.yml` without `workflow` scope)
```

REST Contents API 写同一路径也会返回 404（写普通文件则 201，说明是路径级限制，
不是拼写或目录不存在）。所以这个目录被 `.gitignore` 忽略，配置以模板形式留在 `ci/`。

### 启用方式（二选一）

1. **给令牌加权限（推荐，一条命令即可）**：GitHub → Settings → Developer
   settings → Personal access tokens → 给该 PAT 勾上 `workflow`，然后：

   ```bash
   # 删掉 .gitignore 里 `/.github/workflows/` 那行
   git add .github/workflows/ .gitignore
   git commit -m "ci: enable workflows"
   git push origin main
   ```

2. **网页端操作**：仓库页面 → Add file → Create new file，分别新建
   `.github/workflows/ci.yml` 与 `.github/workflows/release.yml`，把 `ci/` 下
   对应文件的内容粘进去提交。网页登录会话不受 PAT scope 限制。

启用后，每次 push / PR 自动跑质量门禁；推 `v*` tag 会触发校验构建。
**改配置时两边要同步**（或改完 `cp .github/workflows/*.yml ci/`）。

## 正式 Release 资产怎么来的

由维护者在本机用 `scripts/build_all.sh` 交叉编译（gnu / musl + zig，Windows 版带
VERSIONINFO 版本资源），产物落在 `dist/`，再用 `scripts/make_release.py` 创建 Release
并上传，同时自动生成 `dist/SHA256SUMS.txt`。

为什么不让 Actions 直接发版：

1. Windows 交付物用 `x86_64-pc-windows-gnu` + `rust-lld`，GNU ld 会破坏版本资源重定位；
   Actions runner 上的 `msvc` 产物虽可同名，但二进制不同，不能互相替代。
2. Linux arm64 静态 musl 用本地 zig 工具链交叉编译；Windows arm64 需要 clang，
   缺工具链时该平台本版就不提供。
3. 两边同时上传同名资产会互相覆盖，可能出现「本地验证过的产物」和
   「用户实际下载到的产物」不是同一个文件的严重问题。

所以 `release.yml` 只作为**旁证**：证明公开源码在干净环境里也能构建出同名可执行文件，
并给出哈希供人工比对；分发仍以 `dist/` 产物为准。

## 本地对照命令

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python _interop/e2e_all.py all
python _interop/compat_go.py          # Rust ↔ 官方 Go frp 跨实现
```
