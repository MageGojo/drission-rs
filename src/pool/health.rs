//! 代理健康检查 + 出口 IP 地理探测 + **IP↔指纹一致性**(反检测深化)。
//!
//! 思路:住宅代理轮换时,**最强的反爬信号之一是"出口 IP 的地理位置 与 浏览器指纹(时区/语言/定位)
//! 不一致"**。本模块探测每个代理的出口 IP 及其国家/时区/经纬度,据此生成**与该 IP 相符**的
//! [`ContextOverride`](crate::browser::ContextOverride)(时区 + 定位 + 语言),让每个标签的指纹与其代理
//! 出口地理自洽。同时记录连通性/延迟,供 [`ProxyPool`](super::ProxyPool) 跳过不健康代理。
//!
//! 探测默认打 `http://ip-api.com/json`(免费、返回 `query`/`countryCode`/`timezone`/`lat`/`lon`);
//! 可换任何返回同名字段的端点。网络部分走 `reqwest`(库已依赖;socks5 代理需启用其 `socks` 特性)。

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::browser::ContextOverride;
use crate::launcher::Proxy;

/// 代理出口的地理信息(由探测填充)。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxyGeo {
    /// 出口公网 IP。
    pub ip: Option<String>,
    /// 国家代码(ISO,如 `US`/`CN`/`JP`)。
    pub country_code: Option<String>,
    /// IANA 时区(如 `America/New_York`)。
    pub timezone: Option<String>,
    /// 纬度。
    pub latitude: Option<f64>,
    /// 经度。
    pub longitude: Option<f64>,
}

impl ProxyGeo {
    /// 从探测端点 JSON(ip-api.com 风格)解析地理信息。容忍字段缺失。
    pub fn from_ipapi_json(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let f = |k: &str| v.get(k).and_then(Value::as_f64);
        Self {
            ip: s("query").or_else(|| s("ip")),
            country_code: s("countryCode").or_else(|| s("country_code")),
            timezone: s("timezone"),
            latitude: f("lat").or_else(|| f("latitude")),
            longitude: f("lon").or_else(|| f("longitude")),
        }
    }

    /// 据此地理生成**自洽**的上下文覆盖(时区 + 定位 + 语言),用于 `Browser::new_tab_with`。
    pub fn coherent_override(&self) -> ContextOverride {
        let mut ov = ContextOverride::new();
        if let Some(tz) = &self.timezone {
            ov = ov.timezone(tz.clone());
        }
        if let (Some(lat), Some(lon)) = (self.latitude, self.longitude) {
            ov = ov.geolocation(lat, lon);
        }
        if let Some(loc) = self.country_code.as_deref().and_then(locale_for_country) {
            ov = ov.locale(loc);
        }
        ov
    }
}

/// 一个代理的健康状态(由 [`ProxyPool::check_health`](super::ProxyPool::check_health) 填充)。
#[derive(Debug, Clone, Default)]
pub struct ProxyHealth {
    /// 是否连通可用:`None`=未检测,`Some(true/false)`=检测结果。
    pub healthy: Option<bool>,
    /// 探测往返延迟(毫秒)。
    pub latency_ms: Option<u64>,
    /// 出口地理(连通时填充)。
    pub geo: ProxyGeo,
    /// 失败原因(不健康时)。
    pub error: Option<String>,
}

impl ProxyHealth {
    /// 是否**未被显式判定为坏**(健康或未检测都算可用,用于轮换择优)。
    pub fn usable(&self) -> bool {
        self.healthy != Some(false)
    }
}

/// 国家代码 → 一个合理的默认 `locale`(覆盖常见地区;未知返回 `None`)。
///
/// 用于让浏览器 `navigator.language` 与出口 IP 国家相符(粗粒度,够降低"语言-IP 不符"风险)。
pub fn locale_for_country(cc: &str) -> Option<&'static str> {
    let v = match cc.to_ascii_uppercase().as_str() {
        "US" => "en-US",
        "GB" | "UK" => "en-GB",
        "CA" => "en-CA",
        "AU" => "en-AU",
        "CN" => "zh-CN",
        "TW" => "zh-TW",
        "HK" => "zh-HK",
        "JP" => "ja-JP",
        "KR" => "ko-KR",
        "DE" => "de-DE",
        "FR" => "fr-FR",
        "ES" => "es-ES",
        "IT" => "it-IT",
        "RU" => "ru-RU",
        "BR" => "pt-BR",
        "PT" => "pt-PT",
        "NL" => "nl-NL",
        "SE" => "sv-SE",
        "PL" => "pl-PL",
        "TR" => "tr-TR",
        "IN" => "en-IN",
        "ID" => "id-ID",
        "VN" => "vi-VN",
        "TH" => "th-TH",
        "MY" => "ms-MY",
        "SG" => "en-SG",
        "MX" => "es-MX",
        "AR" => "es-AR",
        "SA" => "ar-SA",
        "AE" => "ar-AE",
        "UA" => "uk-UA",
        _ => return None,
    };
    Some(v)
}

/// 探测单个代理:经它请求 `check_url`,测连通/延迟,并解析出口地理。**不**修改任何状态。
pub async fn probe_proxy(proxy: &Proxy, check_url: &str, timeout: Duration) -> ProxyHealth {
    let started = Instant::now();
    match probe_inner(proxy, check_url, timeout).await {
        Ok(geo) => ProxyHealth {
            healthy: Some(true),
            latency_ms: Some(started.elapsed().as_millis() as u64),
            geo,
            error: None,
        },
        Err(e) => ProxyHealth {
            healthy: Some(false),
            latency_ms: None,
            geo: ProxyGeo::default(),
            error: Some(e),
        },
    }
}

async fn probe_inner(
    proxy: &Proxy,
    check_url: &str,
    timeout: Duration,
) -> Result<ProxyGeo, String> {
    let mut rp = reqwest::Proxy::all(&proxy.server).map_err(|e| format!("代理地址非法: {e}"))?;
    if let (Some(u), Some(p)) = (&proxy.username, &proxy.password) {
        rp = rp.basic_auth(u, p);
    }
    let client = reqwest::Client::builder()
        .proxy(rp)
        .timeout(timeout)
        .build()
        .map_err(|e| format!("构建客户端失败(socks5 需启用 reqwest 的 socks 特性): {e}"))?;
    let resp = client
        .get(check_url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("响应非 JSON: {e}"))?;
    // ip-api.com 失败时返回 {"status":"fail","message":...}。
    if v.get("status").and_then(Value::as_str) == Some("fail") {
        return Err(v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("探测端点返回 fail")
            .to_string());
    }
    Ok(ProxyGeo::from_ipapi_json(&v))
}

/// 默认探测端点(免费、无需 key;返回 `query`/`countryCode`/`timezone`/`lat`/`lon`)。
pub const DEFAULT_CHECK_URL: &str = "http://ip-api.com/json";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn locale_mapping_common() {
        assert_eq!(locale_for_country("US"), Some("en-US"));
        assert_eq!(locale_for_country("cn"), Some("zh-CN")); // 大小写不敏感
        assert_eq!(locale_for_country("JP"), Some("ja-JP"));
        assert_eq!(locale_for_country("ZZ"), None);
    }

    #[test]
    fn parse_ipapi_geo() {
        let v = json!({
            "status": "success", "query": "1.2.3.4", "countryCode": "US",
            "timezone": "America/New_York", "lat": 40.71, "lon": -74.0
        });
        let g = ProxyGeo::from_ipapi_json(&v);
        assert_eq!(g.ip.as_deref(), Some("1.2.3.4"));
        assert_eq!(g.country_code.as_deref(), Some("US"));
        assert_eq!(g.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(g.latitude, Some(40.71));
        assert_eq!(g.longitude, Some(-74.0));
    }

    #[test]
    fn coherent_override_from_geo() {
        let g = ProxyGeo {
            ip: Some("1.2.3.4".into()),
            country_code: Some("JP".into()),
            timezone: Some("Asia/Tokyo".into()),
            latitude: Some(35.68),
            longitude: Some(139.69),
        };
        let ov = g.coherent_override();
        assert_eq!(ov.timezone_id.as_deref(), Some("Asia/Tokyo"));
        assert_eq!(ov.locale.as_deref(), Some("ja-JP"));
        assert!(ov.geolocation.is_some());
    }

    #[test]
    fn health_usable_semantics() {
        let mut h = ProxyHealth::default();
        assert!(h.usable()); // 未检测 = 可用
        h.healthy = Some(true);
        assert!(h.usable());
        h.healthy = Some(false);
        assert!(!h.usable()); // 显式坏 = 不可用
    }
}
