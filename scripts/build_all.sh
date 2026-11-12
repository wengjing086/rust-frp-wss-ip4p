#!/usr/bin/env bash
# 一次构建全部发布架构。产物统一放到 dist/ 下，文件名遵循
# <frps|frpc>_<平台>_<架构>[.exe]，不使用压缩包。
#
# 工具链默认放在仓库**上级目录**的 _tools/（与 .gh_token 同级，都在仓库外），
# 可以用 TOOLS_DIR 环境变量覆盖到任意位置，脚本里不再写死某个用户的绝对路径。
#
# 必选平台（windows amd64 / linux amd64 / linux arm64）任一失败即整体失败；
# 只有 windows arm64 允许跳过（本机可能缺 clang），但会明确提示「本版未提供」。
# 以前这里对三个平台都写「构建失败，跳过」，结果会拿旧的 dist/ 产物去发版。
set -euo pipefail

cd "$(dirname "$0")/.." || exit 1   # 回到仓库根：target/ 与 dist/ 都在这一层
ROOT="$(pwd)"
TOOLS_DIR="${TOOLS_DIR:-$(cd "$ROOT/.." && pwd)/_tools}"

if command -v cygpath >/dev/null 2>&1; then
  TOOLS_WIN="$(cygpath -w "$TOOLS_DIR")"
else
  TOOLS_WIN="${TOOLS_WIN:-$TOOLS_DIR}"
fi
MINGW="$TOOLS_DIR/mingw/mingw64/bin"
ZIGDIR="$TOOLS_WIN\\zigcc"

if [ ! -d "$MINGW" ]; then
  echo "!! 找不到 mingw 工具链: $MINGW（用 TOOLS_DIR 指定）" >&2
  exit 1
fi

export PATH="$MINGW:$PATH"
export CC_x86_64_pc_windows_gnu="$TOOLS_WIN/mingw/mingw64/bin/gcc.exe"
export CARGO_HTTP_PROXY=""

mkdir -p dist
step() { echo; echo "########## $* ##########"; }

# 复制并校验：产物必须存在且非空，否则立刻失败
emit() {
  local src="$1" dst="$2"
  if [ ! -s "$src" ]; then
    echo "!! 构建产物不存在或为空: $src" >&2
    exit 1
  fi
  cp "$src" "dist/$dst"
  test -s "dist/$dst" || { echo "!! 复制失败: $dst" >&2; exit 1; }
  echo "   -> dist/$dst ($(stat -c %s "dist/$dst") bytes)"
}

# ---------------------------------------------------------------- Windows x64
# 必须用 rust-lld：GNU ld 不会正确处理版本资源里的重定位，
# 结果就是资源段进了二进制但 Windows 读不出来（详情见 docs/ANTIVIRUS.md）。
step "windows amd64 (x86_64-pc-windows-gnu + rust-lld)"
cargo +stable-x86_64-pc-windows-gnu rustc --release -p nfrp-server --bin nfrp-server -- -C linker=rust-lld
cargo +stable-x86_64-pc-windows-gnu rustc --release -p nfrp-client --bin nfrp-client -- -C linker=rust-lld
emit target/release/nfrp-server.exe frps_windows_amd64.exe
emit target/release/nfrp-client.exe frpc_windows_amd64.exe

# ---------------------------------------------------------------- Windows arm64（可选）
step "windows arm64 (aarch64-pc-windows-gnullvm) — 可选"
export CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER=rust-lld
export CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C link-self-contained=yes"
if cargo +stable-x86_64-pc-windows-gnu build --release --target aarch64-pc-windows-gnullvm -p nfrp-server -p nfrp-client; then
  emit target/aarch64-pc-windows-gnullvm/release/nfrp-server.exe frps_windows_arm64.exe
  emit target/aarch64-pc-windows-gnullvm/release/nfrp-client.exe frpc_windows_arm64.exe
else
  echo "!! windows arm64 构建失败 —— 本版将不提供该平台（通常是缺 clang）"
  rm -f dist/frps_windows_arm64.exe dist/frpc_windows_arm64.exe
fi

# ---------------------------------------------------------------- Linux x64 (musl)
step "linux amd64 (x86_64-unknown-linux-musl)"
export CC_x86_64_unknown_linux_musl="$ZIGDIR\\cc_x86_64_linux_musl.bat"
export AR_x86_64_unknown_linux_musl="$ZIGDIR\\ar_x86_64_linux_musl.bat"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$ZIGDIR\\cc_x86_64_linux_musl.bat"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-self-contained=no"
cargo +stable-x86_64-pc-windows-gnu build --release --target x86_64-unknown-linux-musl -p nfrp-server -p nfrp-client
emit target/x86_64-unknown-linux-musl/release/nfrp-server frps_linux_amd64
emit target/x86_64-unknown-linux-musl/release/nfrp-client frpc_linux_amd64

# ---------------------------------------------------------------- Linux arm64 (musl)
step "linux arm64 (aarch64-unknown-linux-musl)"
export CC_aarch64_unknown_linux_musl="$ZIGDIR\\cc_aarch64_linux_musl.bat"
export AR_aarch64_unknown_linux_musl="$ZIGDIR\\ar_aarch64_linux_musl.bat"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$ZIGDIR\\cc_aarch64_linux_musl.bat"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-self-contained=no"
cargo +stable-x86_64-pc-windows-gnu build --release --target aarch64-unknown-linux-musl -p nfrp-server -p nfrp-client
emit target/aarch64-unknown-linux-musl/release/nfrp-server frps_linux_arm64
emit target/aarch64-unknown-linux-musl/release/nfrp-client frpc_linux_arm64

# ---------------------------------------------------------------- 校验值
step "SHA256SUMS.txt"
(
  cd dist
  # shellcheck disable=SC2046
  sha256sum $(ls frps_* frpc_* 2>/dev/null | sort) > SHA256SUMS.txt
  cat SHA256SUMS.txt
)

step "产物"
ls -la dist/ | grep -E "frps_|frpc_|SHA256SUMS"
