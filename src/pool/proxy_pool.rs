//! 代理池:在一组代理之间按策略轮换,供并发池为每个标签/任务分配不同出口。
//!
//! 除轮换外还支持**健康检查 + 出口地理探测 + IP↔指纹一致性**(见 [`health`](super::health)):
//! [`check_health`](ProxyPool::check_health) 并发探测每个代理的连通/延迟/出口地理,
//! [`next_healthy`](ProxyPool::next_healthy) 轮换时跳过不健康者,
//! [`next_coherent`](ProxyPool::next_coherent) 直接给出"代理 + 与其出口地理自洽的指纹覆盖"。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::browser::ContextOverride;
use crate::launcher::Proxy;

use super::health::{DEFAULT_CHECK_URL, ProxyHealth, probe_proxy};
use super::rotate::{RotateStrategy, Rotator};

/// 一组可轮换的代理。克隆代价低(内部 `Arc`),可在多任务并发取用,游标共享。
///
/// ```
/// use drission::prelude::*;
/// let pool = ProxyPool::new(vec![
///     Proxy::new("socks5://127.0.0.1:1080"),
///     Proxy::new("http://127.0.0.1:8888"),
/// ]).strategy(RotateStrategy::RoundRobin);
/// let _p = pool.next();              // 轮询取下一个
/// let _q = pool.for_key("acct-1");   // 粘性:同 key 固定出口(策略须为 Sticky 才稳定)
/// ```
#[derive(Clone)]
pub struct ProxyPool {
    proxies: Arc<Vec<Proxy>>,
    rotator: Arc<Rotator>,
    /// 每个代理的健康状态(与 `proxies` 同序、同长);由 `check_health`/`mark_bad` 更新。
    health: Arc<Mutex<Vec<ProxyHealth>>>,
}

impl ProxyPool {
    /// 用一组代理新建池,默认 [`RotateStrategy::RoundRobin`]。
    pub fn new(proxies: Vec<Proxy>) -> Self {
        Self::with_strategy(proxies, RotateStrategy::RoundRobin)
    }

    /// 用一组代理 + 指定策略新建池。
    pub fn with_strategy(proxies: Vec<Proxy>, strategy: RotateStrategy) -> Self {
        let n = proxies.len();
        Self {
            proxies: Arc::new(proxies),
            rotator: Arc::new(Rotator::new(strategy)),
            health: Arc::new(Mutex::new(vec![ProxyHealth::default(); n])),
        }
    }

    /// 链式设置轮换策略(返回新句柄,游标重置;健康状态保留)。
    pub fn strategy(self, strategy: RotateStrategy) -> Self {
        Self {
            proxies: self.proxies,
            rotator: Arc::new(Rotator::new(strategy)),
            health: self.health,
        }
    }

    /// 池中代理数量。
    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    /// 是否为空池。
    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }

    /// 按策略取下一个代理;空池返回 `None`。
    #[allow(clippy::should_implement_trait)] // 故意用 `next` 命名以贴近"取下一个"的直觉
    pub fn next(&self) -> Option<Proxy> {
        self.rotator
            .pick(self.proxies.len(), None)
            .map(|i| self.proxies[i].clone())
    }

    /// 按 key 粘性取代理(策略为 [`RotateStrategy::Sticky`] 时同 key 稳定命中同一个);空池返回 `None`。
    pub fn for_key(&self, key: &str) -> Option<Proxy> {
        self.rotator
            .pick(self.proxies.len(), Some(key))
            .map(|i| self.proxies[i].clone())
    }

    // ---- 健康检查 + 出口地理探测 + IP↔指纹一致性(反检测深化)----

    /// 并发探测**所有**代理的连通/延迟/出口地理(用默认端点 [`DEFAULT_CHECK_URL`] + 10s 超时),
    /// 结果写入内部健康表;返回健康(连通)代理数。
    pub async fn check_health(&self) -> usize {
        self.check_health_with(DEFAULT_CHECK_URL, Duration::from_secs(10))
            .await
    }

    /// 同 [`check_health`](Self::check_health),自定义探测端点与超时。
    pub async fn check_health_with(&self, check_url: &str, timeout: Duration) -> usize {
        let proxies = self.proxies.clone();
        let results = futures_util::future::join_all(
            proxies.iter().map(|p| probe_proxy(p, check_url, timeout)),
        )
        .await;
        let healthy = results.iter().filter(|h| h.healthy == Some(true)).count();
        if let Ok(mut guard) = self.health.lock() {
            *guard = results;
        }
        healthy
    }

    /// 轮换取**健康(或未检测)**的代理:从轮换游标起线性跳过被判定为坏的;全坏则返回游标处那个。
    /// 空池返回 `None`。未做过 [`check_health`](Self::check_health) 时所有代理视为可用(等同 `next`)。
    pub fn next_healthy(&self) -> Option<Proxy> {
        let len = self.proxies.len();
        let start = self.rotator.pick(len, None)?;
        if let Ok(guard) = self.health.lock() {
            for off in 0..len {
                let i = (start + off) % len;
                if guard.get(i).map(ProxyHealth::usable).unwrap_or(true) {
                    return Some(self.proxies[i].clone());
                }
            }
        }
        Some(self.proxies[start].clone())
    }

    /// 取一个健康代理 + **与其出口地理自洽**的上下文覆盖(时区/语言/定位 + 该代理),
    /// 直接喂给 [`Browser::new_tab_with`](crate::browser::Browser::new_tab_with)。空池返回 `None`。
    ///
    /// 这是"住宅代理轮换 + 反检测一致性"的一把梭:每个标签都拿到自洽的 IP+指纹。
    pub fn next_coherent(&self) -> Option<ContextOverride> {
        let p = self.next_healthy()?;
        Some(self.coherent_override_for(&p))
    }

    /// 据某代理已探测到的出口地理,生成自洽的上下文覆盖(含该代理本身);未探测到地理则仅含代理。
    pub fn coherent_override_for(&self, proxy: &Proxy) -> ContextOverride {
        let geo = self
            .index_of(&proxy.server)
            .and_then(|i| self.health.lock().ok().map(|g| g[i].geo.clone()))
            .unwrap_or_default();
        geo.coherent_override().proxy(proxy.clone())
    }

    /// 把某代理(按 `server` 匹配)标记为不健康——失败后据此轮换走它。
    pub fn mark_bad(&self, server: &str) {
        if let Some(i) = self.index_of(server)
            && let Ok(mut guard) = self.health.lock()
            && let Some(h) = guard.get_mut(i)
        {
            h.healthy = Some(false);
        }
    }

    /// 当前被判定为健康(连通)的代理数。
    pub fn healthy_count(&self) -> usize {
        self.health
            .lock()
            .map(|g| g.iter().filter(|h| h.healthy == Some(true)).count())
            .unwrap_or(0)
    }

    /// 某代理(按 `server`)的健康快照;未知返回 `None`。
    pub fn health_of(&self, server: &str) -> Option<ProxyHealth> {
        let i = self.index_of(server)?;
        self.health.lock().ok().and_then(|g| g.get(i).cloned())
    }

    /// 全部代理的健康报告(代理 + 健康快照,与池同序)。
    pub fn report(&self) -> Vec<(Proxy, ProxyHealth)> {
        let guard = match self.health.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        self.proxies
            .iter()
            .cloned()
            .zip(guard.iter().cloned())
            .collect()
    }

    fn index_of(&self, server: &str) -> Option<usize> {
        self.proxies.iter().position(|p| p.server == server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> ProxyPool {
        ProxyPool::new(vec![
            Proxy::new("http://a:1"),
            Proxy::new("http://b:2"),
            Proxy::new("http://c:3"),
        ])
    }

    #[test]
    fn round_robin_cycles() {
        let p = pool();
        let got: Vec<String> = (0..4).map(|_| p.next().unwrap().server).collect();
        assert_eq!(
            got,
            vec!["http://a:1", "http://b:2", "http://c:3", "http://a:1"]
        );
    }

    #[test]
    fn empty_pool_returns_none() {
        let p = ProxyPool::new(vec![]);
        assert!(p.is_empty());
        assert!(p.next().is_none());
        assert!(p.for_key("x").is_none());
    }

    #[test]
    fn sticky_same_key_same_proxy() {
        let p = pool().strategy(RotateStrategy::Sticky);
        let a = p.for_key("acct-7").unwrap().server;
        let b = p.for_key("acct-7").unwrap().server;
        assert_eq!(a, b);
    }

    #[test]
    fn clones_share_cursor() {
        // 克隆共享同一游标:交替取应继续轮询序列,而非各自从头。
        let p = pool();
        let q = p.clone();
        assert_eq!(p.next().unwrap().server, "http://a:1");
        assert_eq!(q.next().unwrap().server, "http://b:2");
        assert_eq!(p.next().unwrap().server, "http://c:3");
    }

    #[test]
    fn mark_bad_then_next_healthy_skips_it() {
        let p = pool();
        p.mark_bad("http://a:1");
        // 连取多次都不应再拿到被标坏的 a。
        let got: Vec<String> = (0..6).map(|_| p.next_healthy().unwrap().server).collect();
        assert!(
            !got.iter().any(|s| s == "http://a:1"),
            "应跳过坏代理,实得 {got:?}"
        );
        assert!(got.iter().any(|s| s == "http://b:2"));
        assert!(got.iter().any(|s| s == "http://c:3"));
    }

    #[test]
    fn next_healthy_before_check_behaves_like_next() {
        // 未做健康检查时所有代理可用:next_healthy 仍能取到。
        let p = pool();
        assert!(p.next_healthy().is_some());
        assert_eq!(p.healthy_count(), 0); // 还没探测过,健康计数为 0
    }

    #[test]
    fn all_bad_falls_back_to_cursor() {
        let p = pool();
        for s in ["http://a:1", "http://b:2", "http://c:3"] {
            p.mark_bad(s);
        }
        // 全坏也要给一个(best-effort),不返回 None。
        assert!(p.next_healthy().is_some());
    }

    #[test]
    fn coherent_override_includes_proxy() {
        let p = pool();
        let proxy = Proxy::new("http://b:2");
        let ov = p.coherent_override_for(&proxy);
        assert_eq!(
            ov.proxy.as_ref().map(|p| p.server.as_str()),
            Some("http://b:2")
        );
        // 未探测地理:无时区覆盖。
        assert!(ov.timezone_id.is_none());
    }

    #[test]
    fn report_has_one_entry_per_proxy() {
        let p = pool();
        let rep = p.report();
        assert_eq!(rep.len(), 3);
        assert!(rep.iter().all(|(_, h)| h.healthy.is_none())); // 未检测
    }
}
