//! 把预生成的 Windows 资源对象（VERSIONINFO + 应用清单）链接进 frpc.exe。
//! 说明见 `server/build.rs` 与 `docs/ANTIVIRUS.md`。

use std::path::PathBuf;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
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
        .join(format!("frpc_{suffix}.res"));
    if !res.exists() {
        println!("cargo:warning=未找到 {}，跳过版本资源嵌入", res.display());
        return;
    }
    println!("cargo:rerun-if-changed={}", res.display());
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
