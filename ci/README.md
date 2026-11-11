# CI / Release 工作流模板

这两个文件就是本项目的 GitHub Actions 配置：

| 文件 | 作用 |
|---|---|
| `ci.yml` | push / PR 时跑 `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test --workspace`，并在 Linux / Windows / macOS 做 release 构建 |
| `release.yml` | 推送 `v*` tag 时构建 Windows x86_64(MSVC) / Linux x86_64(musl) / macOS arm64，打包并生成 `SHA256SUMS.txt`，上传到 Release |

## 为什么它们不在 `.github/workflows/`

创建本仓库时使用的 Personal Access Token 只有 `repo` 权限，没有 `workflow`
权限，GitHub 会拒绝任何在 `.github/workflows/` 下新增/修改文件的 push 或 API
写入（报 "refusing to allow a Personal Access Token to create or update
workflow ... without `workflow` scope"）。因此源码里先以模板形式保留。

## 怎么启用（二选一）

1. **给令牌加权限**：在 GitHub → Settings → Developer settings → Personal
   access tokens 里给该 PAT 勾上 `workflow`，然后：

   ```bash
   mkdir -p .github/workflows
   cp ci/ci.yml ci/release.yml .github/workflows/
   # 从 .gitignore 里删掉 /.github/workflows/ 一行
   git add .github/workflows/ .gitignore && git commit -m "ci: enable workflows" && git push
   ```

2. **网页端操作**：在 GitHub 仓库页面用 "Add file → Create new file" 新建
   `.github/workflows/ci.yml` 与 `.github/workflows/release.yml`，把模板内容
   粘进去提交即可（网页登录会话不受 PAT scope 限制）。

启用后推送 `v0.8.0` 之类的 tag 就会自动补齐多平台 Release 产物。
