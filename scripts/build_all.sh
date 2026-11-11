#!/usr/bin/env bash
# 一次构建全部发布架构。产物统一放到 dist/ 下，文件名遵循
# <frps|frpc>_<平台>_<架构>[.exe]，不使用压缩包。
set -u

cd "$(dirname "$0")/.." || exit 1   # 回到仓库根：target/ 与 dist/ 都在这一层

export PATH="/c/Users/xnj/WorkBuddy/2026-11-11-10-04-35/_tools/mingw/mingw64/bin:$PATH"
export CC_x86_64_pc_windows_gnu="C:/Users/xnj/WorkBuddy/2026-11-11-10-04-35/_tools/mingw/mingw64/bin/gcc.exe"
export CARGO_HTTP_PROXY=""
ZIGDIR='C:\Users\xnj\WorkBuddy\2026-11-11-10-04-35\_tools\zigcc'

mkdir -p dist

step() { echo; echo "########## $* ##########"; }

# ---------------------------------------------------------------- Windows x64
# 必须用 rust-lld：GNU ld 不会正确处理版本资源里的重定位，
# 结果就是资源段进了二进制但 Windows 读不出来（详情见 docs/ANTIVIRUS.md）。
step "windows amd64 (x86_64-pc-windows-gnu + rust-lld)"
cargo +stable-x86_64-pc-windows-gnu rustc --release -p nfrp-server --bin nfrp-server -- -C linker=rust-lld || exit 1
cargo +stable-x86_64-pc-windows-gnu rustc --release -p nfrp-client --bin nfrp-client -- -C linker=rust-lld || exit 1
cp target/release/nfrp-server.exe dist/frps_windows_amd64.exe
cp target/release/nfrp-client.exe dist/frpc_windows_amd64.exe

# ---------------------------------------------------------------- Windows arm64
step "windows arm64 (aarch64-pc-windows-gnullvm)"
export CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER=rust-lld
export CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C link-self-contained=yes"
if cargo +stable-x86_64-pc-windows-gnu build --release --target aarch64-pc-windows-gnullvm -p nfrp-server -p nfrp-client; then
  cp target/aarch64-pc-windows-gnullvm/release/nfrp-server.exe dist/frps_windows_arm64.exe
  cp target/aarch64-pc-windows-gnullvm/release/nfrp-client.exe dist/frpc_windows_arm64.exe
else
  echo "!! windows arm64 构建失败，跳过"
fi

# ---------------------------------------------------------------- Linux x64 (musl)
step "linux amd64 (x86_64-unknown-linux-musl)"
export CC_x86_64_unknown_linux_musl="$ZIGDIR\\cc_x86_64_linux_musl.bat"
export AR_x86_64_unknown_linux_musl="$ZIGDIR\\ar_x86_64_linux_musl.bat"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$ZIGDIR\\cc_x86_64_linux_musl.bat"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-self-contained=no"
if cargo +stable-x86_64-pc-windows-gnu build --release --target x86_64-unknown-linux-musl -p nfrp-server -p nfrp-client; then
  cp target/x86_64-unknown-linux-musl/release/nfrp-server dist/frps_linux_amd64
  cp target/x86_64-unknown-linux-musl/release/nfrp-client dist/frpc_linux_amd64
else
  echo "!! linux amd64 构建失败，跳过"
fi

# ---------------------------------------------------------------- Linux arm64 (musl)
step "linux arm64 (aarch64-unknown-linux-musl)"
export CC_aarch64_unknown_linux_musl="$ZIGDIR\\cc_aarch64_linux_musl.bat"
export AR_aarch64_unknown_linux_musl="$ZIGDIR\\ar_aarch64_linux_musl.bat"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$ZIGDIR\\cc_aarch64_linux_musl.bat"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-self-contained=no"
if cargo +stable-x86_64-pc-windows-gnu build --release --target aarch64-unknown-linux-musl -p nfrp-server -p nfrp-client; then
  cp target/aarch64-unknown-linux-musl/release/nfrp-server dist/frps_linux_arm64
  cp target/aarch64-unknown-linux-musl/release/nfrp-client dist/frpc_linux_arm64
else
  echo "!! linux arm64 构建失败，跳过"
fi

step "产物"
ls -la dist/ | grep -E "frps_|frpc_"
