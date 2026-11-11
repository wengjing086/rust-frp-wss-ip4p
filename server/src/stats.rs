//! 官方 frps 面板所需的按代理统计与客户端会话档案。
//!
//! # 为什么不用 `observability` 里那几个全局指标
//!
//! 官方面板（v0.72 的 Vue 前端）要求**每个代理**有自己的：
//!
//! * 今日流量（`todayTrafficIn/Out`）与最近 7 天按天的流量曲线
//!   （`/api/traffic/{name}`、`/api/v2/proxies/{name}/traffic`）；
//! * 当前连接数（`curConns`）；
//! * 最近一次启动 / 关闭时间（`lastStartTime` / `lastCloseTime`）。
//!
//! 全局 gauge/counter 表达不了"按代理"这一维，所以在 [`Registry`] 上
//! 挂一张 `HashMap<代理名, Arc<ProxyStat>>`。
//!
//! # 方向语义（与官方 `server/proxy/proxy.go` 对齐）
//!
//! 官方 `libio.Join(workConn, userConn)` 的返回值里：
//!
//! * **TrafficIn** = workConn → userConn（frpc 流向访问者）；
//! * **TrafficOut** = userConn → workConn（访问者流向 frpc）。
//!
//! 本仓库 `relay_between(user, work)` 返回 `(user→work, work→user)`，
//! 因此挂钩处写 `add_traffic(name, /*in=*/ down, /*out=*/ up)`。
//!
//! # 天槽（DailyRing）
//!
//! 官方用 `metric.NewDateCounter(7)`（按天分桶、保留 7 天）。这里用
//! 7 个"天 → 字节"槽位实现：写入时按"自纪元起的天数"找槽，没有今天
//! 的槽就复用最旧的。读取时对请求的天范围逐天查槽，缺的天补 0。

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Mutex, RwLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// 保留 7 天，与官方 `ReserveDays` 一致。
pub const RESERVE_DAYS: usize = 7;

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn today_day() -> i64 {
    unix_now().div_euclid(86_400)
}

/// 给外部模块用的"今天"（自纪元起的天数）。
pub fn today_day_pub() -> i64 {
    today_day()
}

/// 把"自纪元起的天数"格式化成 `YYYY-MM-DD`（UTC）。
pub fn day_to_date(day: i64) -> String {
    // civil_from_days（Howard Hinnant 算法），不引入 chrono。
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// 7 天按天的字节环形账本。
#[derive(Clone, Default)]
pub struct DailyRing {
    days: [i64; RESERVE_DAYS],
    vals: [u64; RESERVE_DAYS],
}

impl DailyRing {
    fn add(&mut self, v: u64) {
        if v == 0 {
            return;
        }
        let today = today_day();
        if let Some(i) = (0..RESERVE_DAYS).find(|&i| self.days[i] == today) {
            self.vals[i] = self.vals[i].saturating_add(v);
            return;
        }
        // 没有今天的槽：找最旧的槽覆盖。天数最小（或还是 0 初值）的即最旧。
        let oldest = (0..RESERVE_DAYS).min_by_key(|&i| self.days[i]).unwrap_or(0);
        self.days[oldest] = today;
        self.vals[oldest] = v;
    }

    /// 最近 `n` 天（含今天）按天的值，**旧 → 新**。
    fn last_days(&self, n: usize) -> Vec<u64> {
        let today = today_day();
        (0..n)
            .map(|i| {
                let day = today - (n as i64 - 1) + i as i64;
                (0..RESERVE_DAYS)
                    .find(|&s| self.days[s] == day)
                    .map(|s| self.vals[s])
                    .unwrap_or(0)
            })
            .collect()
    }

    fn today(&self) -> u64 {
        let today = today_day();
        (0..RESERVE_DAYS)
            .find(|&i| self.days[i] == today)
            .map(|i| self.vals[i])
            .unwrap_or(0)
    }
}

/// 一个代理的统计账本。
pub struct ProxyStat {
    pub name: String,
    pub proxy_type: String,
    pub user: String,
    pub client_id: String,
    /// 注册时从 `NewProxy` 抓的配置摘要（`/api/proxy/*` 的 `conf` 字段）。
    pub conf: serde_json::Value,
    /// 官方语义：代理当前**已注册**（客户端在线挂着它）。
    online: AtomicBool,
    ring_in: Mutex<DailyRing>,
    ring_out: Mutex<DailyRing>,
    cur_conns: AtomicI64,
    last_start: AtomicI64,
    last_close: AtomicI64,
}

impl ProxyStat {
    pub fn today_in_out(&self) -> (u64, u64) {
        (
            self.ring_in.lock().unwrap_or_else(|e| e.into_inner()).today(),
            self.ring_out
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .today(),
        )
    }

    pub fn last_days_in_out(&self, n: usize) -> (Vec<u64>, Vec<u64>) {
        (
            self.ring_in
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_days(n),
            self.ring_out
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_days(n),
        )
    }

    pub fn cur_conns(&self) -> i64 {
        self.cur_conns.load(Ordering::Relaxed)
    }

    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// 秒级 Unix 时间戳（0 = 尚未发生过）。
    pub fn last_start(&self) -> i64 {
        self.last_start.load(Ordering::Relaxed)
    }

    pub fn last_close(&self) -> i64 {
        self.last_close.load(Ordering::Relaxed)
    }

    fn conn_open(&self) {
        self.cur_conns.fetch_add(1, Ordering::Relaxed);
    }

    fn conn_close(&self) {
        self.cur_conns.fetch_sub(1, Ordering::Relaxed);
    }

    fn add_traffic(&self, inbound: u64, outbound: u64) {
        self.ring_in
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add(inbound);
        self.ring_out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add(outbound);
    }

    fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::Relaxed);
    }
}

/// 全体代理的统计表。
#[derive(Default)]
pub struct ProxyStats {
    map: RwLock<HashMap<String, std::sync::Arc<ProxyStat>>>,
}

impl ProxyStats {
    /// 代理注册成功（`NewProxy`）时调用：不存在则建账，已存在则刷新
    /// 类型 / 归属 / conf 并置为在线（重连场景保住历史流量）。
    pub fn open(
        &self,
        name: &str,
        proxy_type: &str,
        user: &str,
        client_id: &str,
        conf: serde_json::Value,
    ) {
        let mut map = self.map.write().unwrap_or_else(|e| e.into_inner());
        let now = unix_now();
        let (ring_in, ring_out, cur_conns, last_close) = match map.get(name) {
            // 重连：保住历史流量与连接计数，只刷新身份与时间。
            Some(st) => (
                // `.clone()` 经 Deref 解到 DailyRing 上 —— 拿到的是账本值的克隆。
                Mutex::new(st.ring_in.lock().unwrap_or_else(|e| e.into_inner()).clone()),
                Mutex::new(
                    st.ring_out
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone(),
                ),
                st.cur_conns.load(Ordering::Relaxed),
                st.last_close.load(Ordering::Relaxed),
            ),
            None => (
                Mutex::new(DailyRing::default()),
                Mutex::new(DailyRing::default()),
                0i64,
                0i64,
            ),
        };
        map.insert(
            name.to_string(),
            std::sync::Arc::new(ProxyStat {
                name: name.to_string(),
                proxy_type: proxy_type.to_string(),
                user: user.to_string(),
                client_id: client_id.to_string(),
                conf,
                online: AtomicBool::new(true),
                ring_in,
                ring_out,
                cur_conns: AtomicI64::new(cur_conns),
                last_start: AtomicI64::new(now),
                last_close: AtomicI64::new(last_close),
            }),
        );
    }

    /// 代理注销（`CloseProxy` / 客户端退出清理）时调用。
    pub fn mark_closed(&self, name: &str) {
        if let Some(st) = self.get(name) {
            st.set_online(false);
            st.last_close
                .store(unix_now(), std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 客户端会话**整体回收**时调用：只标记仍归属该客户端的代理。
    ///
    /// # 为什么不能直接复用 [`mark_closed`]
    ///
    /// 客户端断线重连的时序是「新会话先登录并注册同名代理」→「旧会话的
    /// 清理任务才被调度」。若按名字无条件置离线，旧会话的清理会把**新会话
    /// 刚注册的**那条代理一起标成 offline，面板上"在线"与否就取决于两个
    /// 任务谁先跑 —— 表现为随机闪烁，比不更新更难排查。
    ///
    /// 这里比对 `client_id`：同名的代理若已经换了主人，就跳过；
    /// 同一台机器换配置重连（client_id 相同）时仍然正确置离线。
    pub fn mark_closed_of(&self, name: &str, client_id: &str) {
        if let Some(st) = self.get(name) {
            if st.client_id == client_id {
                st.set_online(false);
                st.last_close
                    .store(unix_now(), std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<std::sync::Arc<ProxyStat>> {
        self.map
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    pub fn all(&self) -> Vec<std::sync::Arc<ProxyStat>> {
        self.map
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn by_type(&self, proxy_type: &str) -> Vec<std::sync::Arc<ProxyStat>> {
        self.all()
            .into_iter()
            .filter(|s| s.proxy_type == proxy_type)
            .collect()
    }

    pub fn type_counts(&self) -> HashMap<String, i64> {
        let mut out: HashMap<String, i64> = HashMap::new();
        for st in self.all() {
            *out.entry(st.proxy_type.clone()).or_insert(0) += 1;
        }
        out
    }

    pub fn conn_open(&self, name: &str) {
        if let Some(st) = self.get(name) {
            st.conn_open();
        }
    }

    pub fn conn_close(&self, name: &str) {
        if let Some(st) = self.get(name) {
            st.conn_close();
        }
    }

    /// 记一笔流量：`inbound` = frpc → 访问者，`outbound` = 访问者 → frpc。
    pub fn add_traffic(&self, name: &str, inbound: u64, outbound: u64) {
        if let Some(st) = self.get(name) {
            st.add_traffic(inbound, outbound);
        }
    }

    /// 全局当前连接数（`/api/serverinfo` 的 `curConns`）。
    pub fn total_cur_conns(&self) -> i64 {
        self.all().iter().map(|s| s.cur_conns()).sum()
    }

    /// 今日全局流量（`/api/serverinfo` 的 `totalTrafficIn/Out`，官方口径是
    /// **今日**值 —— `mem.StatsCollector.GetServer().TotalTrafficIn.TodayCount()`）。
    pub fn total_today_in_out(&self) -> (u64, u64) {
        let mut tin = 0u64;
        let mut tout = 0u64;
        for st in self.all() {
            let (i, o) = st.today_in_out();
            tin += i;
            tout += o;
        }
        (tin, tout)
    }

    /// 清掉所有离线代理的统计（`DELETE /api/proxies?status=offline`）。
    pub fn clear_offline(&self) -> (usize, usize) {
        let mut map = self.map.write().unwrap_or_else(|e| e.into_inner());
        let before = map.len();
        map.retain(|_, st| st.is_online());
        (before - map.len(), map.len())
    }
}

/// 一个客户端会话的档案（登录/断开历史）。
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub client_id: String,
    pub user: String,
    pub run_id: String,
    pub version: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub ip: String,
    pub first_connected: i64,
    pub last_connected: i64,
    pub disconnected: i64,
    pub online: bool,
}

/// 客户端会话档案表：key = client_id。
///
/// # 容量上界
///
/// 官方只在内存里保留当前客户端；这里为了"断开的客户端也在列表里可见"
/// 保留历史，但封顶 [`MAX_SESSIONS`]，超出淘汰**最早断开**的离线条目，
/// 防止攻击者用海量 `client_id` 反复登录把内存撑爆。
#[derive(Default)]
pub struct SessionTable {
    map: RwLock<HashMap<String, SessionMeta>>,
}

/// 会话档案上限（在线 + 离线历史合计）。
pub const MAX_SESSIONS: usize = 512;

impl SessionTable {
    /// 登录即建档（重连刷新 run_id / 时间）。
    ///
    /// 9 个参数是官方 `/api/clients` 契约的完整字段集 —— 与其拆一个
    /// 只用一次的参数对象，不如收下 clippy 的豁免并保持调用点一眼可读。
    #[allow(clippy::too_many_arguments)]
    pub fn mark_login(
        &self,
        client_id: &str,
        user: &str,
        run_id: &str,
        version: &str,
        hostname: &str,
        os: &str,
        arch: &str,
        ip: &str,
    ) {
        let mut map = self.map.write().unwrap_or_else(|e| e.into_inner());
        let now = unix_now();
        // 超限先淘汰最早的离线会话，再落新条目（必须在 entry() 之前做，
        // 否则 or_insert_with 闭包里再借 map 会撞 E0499）。
        if !map.contains_key(client_id) && map.len() >= MAX_SESSIONS {
            let oldest = map
                .iter()
                .filter(|(_, m)| !m.online)
                .min_by_key(|(_, m)| m.disconnected)
                .map(|(k, _)| k.clone());
            if let Some(o) = oldest {
                map.remove(&o);
            }
        }
        let entry = map.entry(client_id.to_string()).or_insert_with(|| {
            SessionMeta {
                client_id: client_id.to_string(),
                user: String::new(),
                run_id: String::new(),
                version: String::new(),
                hostname: String::new(),
                os: String::new(),
                arch: String::new(),
                ip: String::new(),
                first_connected: now,
                last_connected: now,
                disconnected: 0,
                online: false,
            }
        });
        entry.user = user.to_string();
        entry.run_id = run_id.to_string();
        entry.version = version.to_string();
        entry.hostname = hostname.to_string();
        entry.os = os.to_string();
        entry.arch = arch.to_string();
        entry.ip = ip.to_string();
        entry.last_connected = now;
        entry.online = true;
        entry.disconnected = 0;
    }

    pub fn mark_offline(&self, client_id: &str) {
        if let Some(m) = self
            .map
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(client_id)
        {
            m.online = false;
            m.disconnected = unix_now();
        }
    }

    pub fn list(&self) -> Vec<SessionMeta> {
        self.map
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn get(&self, client_id: &str) -> Option<SessionMeta> {
        self.map
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(client_id)
            .cloned()
    }

    /// 在线客户端数（与在线档案数一致；断线未清理的也以档案为准）。
    pub fn online_count(&self) -> i64 {
        self.list().iter().filter(|m| m.online).count() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_rollover_keeps_only_7_days() {
        let mut ring = DailyRing::default();
        let today = today_day();
        // 伪造 10 个历史天的写入：7 个槽只留**最后 7 天**（today-3..=today）。
        for d in (today - 9)..=today {
            // 直接操纵内部状态模拟"当天写入"。
            let oldest = (0..RESERVE_DAYS).min_by_key(|&i| ring.days[i]).unwrap_or(0);
            ring.days[oldest] = d;
            ring.vals[oldest] = 100;
        }
        // 7 个槽按"最旧覆盖"滚动：连写 10 天后，留下的恰是**最近 7 天**。
        let days = ring.last_days(7);
        assert_eq!(days.len(), 7);
        assert_eq!(days, vec![100; 7], "应保留最近 7 天，更早的已被滚动挤出");
    }

    #[test]
    fn today_sums_into_one_slot() {
        let mut ring = DailyRing::default();
        ring.add(1);
        ring.add(2);
        ring.add(0);
        assert_eq!(ring.today(), 3);
        let days = ring.last_days(3);
        assert_eq!(*days.last().unwrap(), 3);
    }

    #[test]
    fn day_to_date_is_utc_ymd() {
        // 20000 天 = 1970-01-01 + 20000 天 = 2024-10-04。
        assert_eq!(day_to_date(20_000), "2024-10-04");
        assert_eq!(day_to_date(0), "1970-01-01");
        // 已知锚点：2026-01-01 = 20454 天（1970→2026 共 56 年，含 14 个闰日）。
        assert_eq!(day_to_date(20_454), "2026-01-01");
    }

    #[test]
    fn proxy_stats_lifecycle() {
        let s = ProxyStats::default();
        s.open("alice.web", "tcp", "alice", "c1", serde_json::json!({}));
        assert!(s.get("alice.web").unwrap().is_online());
        s.conn_open("alice.web");
        s.add_traffic("alice.web", 10, 20);
        assert_eq!(s.get("alice.web").unwrap().cur_conns(), 1);
        s.conn_close("alice.web");
        assert_eq!(s.get("alice.web").unwrap().cur_conns(), 0);
        let (tin, tout) = s.total_today_in_out();
        assert_eq!((tin, tout), (10, 20));
        s.mark_closed("alice.web");
        assert!(!s.get("alice.web").unwrap().is_online());
        let (cleared, total) = s.clear_offline();
        assert_eq!((cleared, total), (1, 0));
    }

    #[test]
    fn reopen_keeps_traffic_history() {
        let s = ProxyStats::default();
        s.open("p", "stcp", "u", "c1", serde_json::json!({}));
        s.add_traffic("p", 5, 5);
        // 重连：重新 open。
        s.open("p", "stcp", "u", "c1", serde_json::json!({}));
        let (tin, _) = s.total_today_in_out();
        assert_eq!(tin, 5, "重连不应清零历史流量");
    }

    #[test]
    fn session_lifecycle() {
        let t = SessionTable::default();
        t.mark_login("c1", "u", "r1", "0.8.1", "host", "windows", "amd64", "1.2.3.4");
        assert_eq!(t.online_count(), 1);
        let m = t.get("c1").unwrap();
        assert_eq!(m.hostname, "host");
        assert!(m.online);
        t.mark_offline("c1");
        assert_eq!(t.online_count(), 0);
        assert!(t.get("c1").unwrap().disconnected > 0);
        // 重新登录恢复在线。
        t.mark_login("c1", "u", "r2", "0.8.1", "host", "windows", "amd64", "1.2.3.4");
        assert_eq!(t.online_count(), 1);
        assert_eq!(t.get("c1").unwrap().run_id, "r2");
    }

    /// v0.8.5 回归：客户端被强杀（发不出 `CloseProxy`）后，代理必须能被标离线。
    ///
    /// 修复前 `mark_closed` 在生产代码里**没有任何调用点**，`open()` 置的
    /// `online=true` 永远没人复位 —— 面板上的代理一直是"在线"，
    /// 连"清理离线代理"也清不掉（它按 `online == false` 筛）。
    #[test]
    fn mark_closed_of_按归属置离线() {
        let s = ProxyStats::default();
        s.open("p1", "tcp", "u", "client-a", serde_json::json!({}));
        assert!(s.get("p1").unwrap().is_online());

        s.mark_closed_of("p1", "client-a");
        assert!(!s.get("p1").unwrap().is_online(), "归属匹配时必须置离线");
        assert!(
            s.get("p1").unwrap().last_close() > 0,
            "关闭时间要写下来，面板靠它显示最后离线时刻"
        );
    }

    /// 重连时序：新会话已经注册了同名代理，旧会话的清理**不能**把它标离线。
    ///
    /// 否则"在线与否"取决于两个清理任务谁先跑，面板随机闪烁。
    #[test]
    fn mark_closed_of_不误伤新会话的同名代理() {
        let s = ProxyStats::default();
        s.open("p1", "tcp", "u", "client-old", serde_json::json!({}));
        // 新会话（同一台机器重连，client_id 换了）抢到同名代理。
        s.open("p1", "tcp", "u", "client-new", serde_json::json!({}));

        s.mark_closed_of("p1", "client-old");
        assert!(
            s.get("p1").unwrap().is_online(),
            "旧会话的清理不该把新会话刚注册的代理标离线"
        );

        s.mark_closed_of("p1", "client-new");
        assert!(!s.get("p1").unwrap().is_online(), "真正的主人下线才置离线");
    }

    /// 重连后历史流量与连接计数要保住（`open` 的既有语义，顺带锁住）。
    #[test]
    fn 离线后再上线_流量不清零() {
        let s = ProxyStats::default();
        s.open("p1", "tcp", "u", "c1", serde_json::json!({}));
        s.add_traffic("p1", 100, 200);
        s.mark_closed_of("p1", "c1");
        assert!(!s.get("p1").unwrap().is_online());

        s.open("p1", "tcp", "u", "c1", serde_json::json!({}));
        let st = s.get("p1").unwrap();
        assert!(st.is_online(), "重新注册要恢复在线");
        let (tin, tout) = st.today_in_out();
        assert_eq!((tin, tout), (100, 200), "重连不该清零历史流量");
    }
}
