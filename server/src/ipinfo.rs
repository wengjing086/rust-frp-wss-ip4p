//! 面板 / API 用的「客户端 IP + 归属地」取值封装。
//!
//! 归属地是**在线查询 + 缓存**的（见 [`nfrp_common::ipgeo`]），这里只做三件事：
//!
//! 1. 把「取一次值」统一成 [`describe`]，避免面板和 API 各写一套；
//! 2. 顺手触发后台补查 —— 命中缓存立刻返回，没命中先返回裸 IP，
//!    **绝不阻塞** HTTP 响应（面板是每几秒轮询一次的，等 3 秒不如等下一轮）；
//! 3. 让"关闭归属地查询"只在一处生效。
//!
//! # 为什么要单独一个模块
//!
//! `dashboard.rs` 与 `api_v2.rs` 都要这份数据，而 `api_v2` 里再 `use crate::dashboard::…`
//! 会把两个本不相干的视图层绑在一起。这层薄封装让两边只依赖"取数"语义。

use std::net::IpAddr;

use nfrp_common::ipgeo;

/// 一条 IP 展示所需的全部形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpDisplay {
    /// 裸地址（`203.0.113.7`）。
    pub ip: String,
    /// 归属地短串（`浙江 杭州 阿里云`）；境外 / 内网 / 未查出时为空串。
    pub geo: String,
    /// 合并后的展示串（`203.0.113.7 (浙江 杭州 阿里云)`），表格里直接用这个。
    pub full: String,
}

/// 取一条 IP 的展示信息，并（在需要时）发起后台补查。
pub fn describe(ip: &IpAddr) -> IpDisplay {
    let raw = ip.to_string();
    let g = ipgeo::global();
    // 未命中就会在这里排队补查；命中则是纯内存读。
    g.request(&raw);
    let full = g.annotate(&raw);
    let geo = g.cached(&raw).unwrap_or_default();
    IpDisplay { ip: raw, geo, full }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 内网地址只给裸_ip() {
        let ip: IpAddr = "192.168.1.2".parse().unwrap();
        let d = describe(&ip);
        assert_eq!(d.ip, "192.168.1.2");
        assert_eq!(d.geo, "");
        assert_eq!(d.full, "192.168.1.2", "内网地址不该带归属地");
    }

    #[test]
    fn 公网地址未命中时先返回裸_ip() {
        // 不能依赖外网：这里只断言"不会 panic 且格式合理"。
        // 真查不到时 geo 为空，full 就是裸 IP —— 面板上表现为"归属地还没出来"。
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let d = describe(&ip);
        assert_eq!(d.ip, "203.0.113.7");
        assert!(
            d.full == "203.0.113.7" || d.full.starts_with("203.0.113.7 ("),
            "展示串格式不对：{}",
            d.full
        );
    }
}
