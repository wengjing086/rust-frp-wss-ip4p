//! 内嵌的官方面板静态资源（v0.72 前端构建产物，`server/web/dist/`）。
//!
//! # 为什么直接 include_bytes 而不用 rust-embed / build script
//!
//! 只有 4 个文件（约 550 KB），手写展开零依赖、零宏魔法，产物路径里带
//! vite 内容哈希 —— 前端重新构建后只需更新本文件里的 3 个文件名。
//! [`ASSETS`] 表是唯一的查找入口，dashboard 路由按 `/static/<name>` 查它。
//!
//! # 重新构建前端
//!
//! ```sh
//! cd _frp_src/web/frps && npm install && npm run build-only
//! cp -r dist/* <repo>/server/web/dist/
//! # 然后把下面的哈希文件名同步成 dist 里的新名字
//! ```

/// `(路径后缀, 内容, Content-Type)`
pub static ASSETS: &[(&str, &[u8], &str)] = &[
    ("index.html", include_bytes!("../web/dist/index.html"), "text/html; charset=utf-8"),
    (
        "index-C-aGI3M_.js",
        include_bytes!("../web/dist/index-C-aGI3M_.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "index-B-mzYQ4X.css",
        include_bytes!("../web/dist/index-B-mzYQ4X.css"),
        "text/css; charset=utf-8",
    ),
    ("favicon.ico", include_bytes!("../web/dist/favicon.ico"), "image/x-icon"),
];

/// 按 `/static/` 之后的相对路径查资源。
pub fn lookup(rel: &str) -> Option<(&'static [u8], &'static str)> {
    ASSETS
        .iter()
        .find(|(name, _, _)| *name == rel)
        .map(|(_, data, ctype)| (*data, *ctype))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 内嵌资源必须齐全且非空() {
        assert!(lookup("index.html").is_some());
        assert!(lookup("favicon.ico").is_some());
        // 哈希文件名变更后忘记同步本表是最容易犯的错 —— 用"JS/CSS 至少
        // 各有一个非空条目"兜底检测。
        let js = ASSETS.iter().filter(|(n, d, t)| {
            t.contains("javascript") && n.starts_with("index-") && !d.is_empty()
        });
        let css = ASSETS.iter().filter(|(n, d, t)| {
            t.starts_with("text/css") && n.starts_with("index-") && !d.is_empty()
        });
        assert!(js.count() >= 1, "内嵌 JS 资源缺失（哈希名未同步？）");
        assert!(css.count() >= 1, "内嵌 CSS 资源缺失（哈希名未同步？）");
    }

    #[test]
    fn index_html_引用的文件名必须能在表里查到() {
        let html = String::from_utf8_lossy(lookup("index.html").unwrap().0);
        for (pos, _) in html.match_indices("index-") {
            let rest = &html[pos..];
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            let name = &rest[..end];
            assert!(
                lookup(name).is_some(),
                "index.html 引用了 {name}，但 ASSETS 表里没有 —— 前端重新构建后需同步本文件"
            );
        }
    }
}
