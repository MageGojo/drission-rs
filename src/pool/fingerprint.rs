//! 指纹池:在一组**轻量指纹**(per-context 可下发的字段)之间轮换。
//!
//! 只覆盖 Juggler **context 级**能改的项(UA / locale / 时区 / platform / OS / 地理 / 视口),
//! 因此能"同一浏览器进程内、每标签不同"。canvas/webgl/screen 等**深指纹是进程级**的,
//! 无法经此轮换——需要时给并发池里不同的浏览器 worker 配不同的 [`BrowserOptions`] 基线。

use std::sync::Arc;

use crate::browser::ContextOverride;
use crate::launcher::{Geolocation, OsType};

use super::rotate::{RotateStrategy, Rotator};

/// 一份轻量指纹画像(全部为 context 级可覆盖项,叠加到浏览器基线之上)。
#[derive(Debug, Clone, Default)]
pub struct FingerprintProfile {
    pub user_agent: Option<String>,
    pub locale: Option<String>,
    pub timezone_id: Option<String>,
    pub platform: Option<String>,
    pub os: Option<OsType>,
    pub geolocation: Option<Geolocation>,
    pub window_size: Option<(u32, u32)>,
}

impl FingerprintProfile {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone_id = Some(tz.into());
        self
    }
    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }
    pub fn os(mut self, os: OsType) -> Self {
        self.os = Some(os);
        self
    }
    pub fn geolocation(mut self, latitude: f64, longitude: f64) -> Self {
        self.geolocation = Some(Geolocation {
            latitude,
            longitude,
            accuracy: None,
        });
        self
    }
    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    /// 把本画像的字段叠加到一个 [`ContextOverride`] 上(保留其已设的代理等其它项)。
    pub fn apply_to(&self, mut ov: ContextOverride) -> ContextOverride {
        if let Some(ua) = &self.user_agent {
            ov.user_agent = Some(ua.clone());
        }
        if let Some(l) = &self.locale {
            ov.locale = Some(l.clone());
        }
        if let Some(tz) = &self.timezone_id {
            ov.timezone_id = Some(tz.clone());
        }
        if let Some(p) = &self.platform {
            ov.platform = Some(p.clone());
        }
        if let Some(os) = self.os {
            ov.os = Some(os);
        }
        if let Some(g) = self.geolocation {
            ov.geolocation = Some(g);
        }
        if let Some(ws) = self.window_size {
            ov.window_size = Some(ws);
        }
        ov
    }
}

/// 一组可轮换的轻量指纹。克隆代价低(`Arc`),游标在克隆间共享。
#[derive(Clone)]
pub struct FingerprintPool {
    profiles: Arc<Vec<FingerprintProfile>>,
    rotator: Arc<Rotator>,
}

impl FingerprintPool {
    /// 用一组画像新建池,默认 [`RotateStrategy::RoundRobin`]。
    pub fn new(profiles: Vec<FingerprintProfile>) -> Self {
        Self::with_strategy(profiles, RotateStrategy::RoundRobin)
    }

    /// 用一组画像 + 指定策略新建池。
    pub fn with_strategy(profiles: Vec<FingerprintProfile>, strategy: RotateStrategy) -> Self {
        Self {
            profiles: Arc::new(profiles),
            rotator: Arc::new(Rotator::new(strategy)),
        }
    }

    /// 内置一组**安全预设**:只轮换 locale + 时区 + 视口——这些彼此自洽、且不与 UA/平台冲突,
    /// 适合"多身份反关联"。**不**默认改 UA / platform / OS(那需与引擎版本、地区一致,易自相矛盾;
    /// 需要时自建画像或用 worker 级 [`BrowserOptions`])。
    pub fn presets() -> Self {
        Self::new(vec![
            FingerprintProfile::new()
                .locale("zh-CN")
                .timezone("Asia/Shanghai")
                .window_size(1920, 1080),
            FingerprintProfile::new()
                .locale("en-US")
                .timezone("America/New_York")
                .window_size(1536, 864),
            FingerprintProfile::new()
                .locale("en-GB")
                .timezone("Europe/London")
                .window_size(1366, 768),
            FingerprintProfile::new()
                .locale("ja-JP")
                .timezone("Asia/Tokyo")
                .window_size(1440, 900),
            FingerprintProfile::new()
                .locale("de-DE")
                .timezone("Europe/Berlin")
                .window_size(1920, 1080),
        ])
    }

    /// 链式设置轮换策略(返回新句柄,游标重置)。
    pub fn strategy(self, strategy: RotateStrategy) -> Self {
        Self {
            profiles: self.profiles,
            rotator: Arc::new(Rotator::new(strategy)),
        }
    }

    /// 画像数量。
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// 是否为空池。
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// 按策略取下一个画像;空池返回 `None`。
    #[allow(clippy::should_implement_trait)]
    pub fn next(&self) -> Option<FingerprintProfile> {
        self.rotator
            .pick(self.profiles.len(), None)
            .map(|i| self.profiles[i].clone())
    }

    /// 按 key 粘性取画像(策略为 [`RotateStrategy::Sticky`] 时同 key 稳定命中同一个);空池返回 `None`。
    pub fn for_key(&self, key: &str) -> Option<FingerprintProfile> {
        self.rotator
            .pick(self.profiles.len(), Some(key))
            .map(|i| self.profiles[i].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_to_sets_fields_and_keeps_proxy() {
        use crate::launcher::Proxy;
        let ov = ContextOverride::new().proxy(Proxy::new("http://x:1"));
        let prof = FingerprintProfile::new()
            .locale("zh-CN")
            .timezone("Asia/Shanghai")
            .window_size(1280, 800);
        let merged = prof.apply_to(ov);
        assert_eq!(merged.locale.as_deref(), Some("zh-CN"));
        assert_eq!(merged.timezone_id.as_deref(), Some("Asia/Shanghai"));
        assert_eq!(merged.window_size, Some((1280, 800)));
        assert!(merged.proxy.is_some(), "应保留已设的代理");
    }

    #[test]
    fn presets_rotate_locales() {
        let p = FingerprintPool::presets();
        assert_eq!(p.len(), 5);
        let first = p.next().unwrap().locale;
        let second = p.next().unwrap().locale;
        assert_ne!(first, second, "轮询应取到不同画像");
    }

    #[test]
    fn empty_pool_returns_none() {
        let p = FingerprintPool::new(vec![]);
        assert!(p.is_empty());
        assert!(p.next().is_none());
    }
}
