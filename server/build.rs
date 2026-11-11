//! 把预生成的 Windows 资源对象（VERSIONINFO + 应用清单）链接进 frps.exe。
//!
//! 资源文件由 `scripts/gen_winres.py` 生成并随仓库提交，因此构建过程
//! **不需要 rc.exe / Windows SDK / 任何外部工具**，也不在编译期启动子进程。
//! 只在 `*-pc-windows-msvc` 且已备好对应架构的 .res 时生效；其它目标
//! （Linux / macOS / musl / Windows GNU / ARM64）安静跳过，构建行为不变。
//!
//! 这么做是为了降低安全软件误报：带完整版本信息、产品名、版权、
//! `asInvoker` 清单的可执行文件，在 Defender / 国内杀软的信誉与启发式
//! 评分里明显优于「三无」二进制。详见 `docs/ANTIVIRUS.md`。

use std::path::PathBuf;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // MSVC 与 MinGW(GNU) 的链接器都接受 COFF `.res` 对象；其余平台跳过。
    if os != "windows" || (env != "msvc" && env != "gnu") {
        return;
    }
    let suffix = match arch.as_str() {
        "x86_64" => "x86_64",
        "x86" => "i686",
        _ => return,
    };
    let res = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("assets")
        .join("winres")
        .join(format!("frps_{suffix}.res"));
    if !res.exists() {
        println!("cargo:warning=未找到 {}，跳过版本资源嵌入", res.display());
        return;
    }
    println!("cargo:rerun-if-changed={}", res.display());
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
