//! 代理池健康检查 + 出口地理探测 + IP↔指纹一致性(反检测深化)自验证。
//!
//! 默认**完全离线**:用假代理验证"标坏→轮换跳过""一致性覆盖(时区/语言/定位)从地理生成"等纯逻辑。
//! 若设环境变量 `DRISSION_PROXY=socks5://user:pass@host:port`(或 http://...),则额外做一次**真实**
//! 健康/地理探测并打印报告 + `next_coherent()` 给出的自洽覆盖(socks5 需库以 `socks` 特性编译,已默认开)。
//!
//! 运行:`cargo run --example proxy_health`
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`。

use std::time::Duration;

use drission::pool::ProxyGeo;
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // ---------- 离线:轮换 + 标坏跳过 ----------
    let pool = ProxyPool::new(vec![
        Proxy::new("socks5://10.0.0.1:1080"),
        Proxy::new("socks5://10.0.0.2:1080"),
        Proxy::new("socks5://10.0.0.3:1080"),
    ]);
    pool.mark_bad("socks5://10.0.0.2:1080");
    let picks: Vec<String> = (0..6)
        .map(|_| pool.next_healthy().unwrap().server)
        .collect();
    let skip_ok = !picks.iter().any(|s| s.ends_with("10.0.0.2:1080"));
    println!("[1] 标坏 .2 后轮换={picks:?} 跳过坏代理={skip_ok}");

    // ---------- 离线:从出口地理生成自洽覆盖 ----------
    let geo = ProxyGeo {
        ip: Some("203.0.113.7".into()),
        country_code: Some("JP".into()),
        timezone: Some("Asia/Tokyo".into()),
        latitude: Some(35.68),
        longitude: Some(139.69),
    };
    let ov = geo.coherent_override();
    let coherent_ok = ov.timezone_id.as_deref() == Some("Asia/Tokyo")
        && ov.locale.as_deref() == Some("ja-JP")
        && ov.geolocation.is_some();
    println!(
        "[2] JP 出口→覆盖: tz={:?} locale={:?} geo={} (ok={coherent_ok})",
        ov.timezone_id,
        ov.locale,
        ov.geolocation.is_some()
    );

    // ---------- 离线:coherent_override_for 必带该代理 ----------
    let proxy = Proxy::new("socks5://10.0.0.1:1080");
    let ov2 = pool.coherent_override_for(&proxy);
    let proxy_attached = ov2.proxy.as_ref().map(|p| p.server.clone()) == Some(proxy.server.clone());
    println!("[3] coherent_override_for 带代理={proxy_attached}");

    let offline_pass = skip_ok && coherent_ok && proxy_attached;

    // ---------- 可选:真实代理探测 ----------
    if let Ok(server) = std::env::var("DRISSION_PROXY") {
        println!("\n[*] 检测到 DRISSION_PROXY,做真实健康/地理探测…");
        let real = ProxyPool::new(vec![Proxy::new(server)]);
        let healthy = real
            .check_health_with("http://ip-api.com/json", Duration::from_secs(15))
            .await;
        println!("[real] 健康代理数={healthy}");
        for (p, h) in real.report() {
            println!(
                "[real] {} healthy={:?} latency={:?}ms ip={:?} country={:?} tz={:?} err={:?}",
                p.server,
                h.healthy,
                h.latency_ms,
                h.geo.ip,
                h.geo.country_code,
                h.geo.timezone,
                h.error
            );
        }
        if let Some(ov) = real.next_coherent() {
            println!(
                "[real] next_coherent → proxy={:?} tz={:?} locale={:?}",
                ov.proxy.as_ref().map(|p| &p.server),
                ov.timezone_id,
                ov.locale
            );
        }
    } else {
        println!("\n(提示:设 DRISSION_PROXY=socks5://host:port 可额外跑一次真实探测)");
    }

    println!(
        "\n==== {} ====",
        if offline_pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );
    if offline_pass {
        Ok(())
    } else {
        Err(drission::Error::msg("proxy_health 离线校验未通过"))
    }
}
