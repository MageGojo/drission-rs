//! 公开 API 离线集成测试(不开浏览器,CI 可跑)。
//!
//! 与 `src/**` 里的 inline 单测互补:这里只通过 crate 的**对外公开导出**(`prelude` 等)
//! 验证契约,确保发布给用户的 API 表面行为稳定。全部确定性、无网络、无浏览器。

use std::collections::HashMap;

use drission::codec::{FrameDecoder, encode_frame};
use drission::prelude::*;

#[test]
fn locator_prelude_parses_dp_syntax() {
    assert!(matches!(parse_locator("#kw"), Query::Css(_)));
    assert!(matches!(parse_locator("css:div.box"), Query::Css(_)));
    assert!(matches!(parse_locator("@id:kw"), Query::Xpath(_)));
    assert!(matches!(parse_locator("登录"), Query::Xpath(_)));
    assert_eq!(parse_locator("tag: li").as_str(), "li");
    assert!(parse_locator("xpath://a").is_xpath());
}

#[test]
fn codec_roundtrip_via_public_api() {
    let mut stream = Vec::new();
    stream.extend_from_slice(&encode_frame(b"{\"a\":1}"));
    stream.extend_from_slice(&encode_frame(b"{\"b\":2}"));

    let mut d = FrameDecoder::new();
    d.push(&stream);
    assert_eq!(d.next_frame().unwrap(), b"{\"a\":1}");
    assert_eq!(d.next_frame().unwrap(), b"{\"b\":2}");
    assert!(d.next_frame().is_none());
}

#[test]
fn scrape_exports_csv_and_json() {
    let rows = vec![
        vec!["name".to_string(), "price".to_string()],
        vec!["苹果".to_string(), "3,5".to_string()],
    ];
    let csv = rows_to_csv(&rows);
    assert!(csv.contains("\"3,5\""), "含逗号字段应被引号包裹: {csv:?}");
    assert!(csv.ends_with("\r\n"));

    let mut rec = HashMap::new();
    rec.insert("k".to_string(), "v".to_string());
    let json = records_to_json(&[rec]);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed[0]["k"], "v");
}

#[tokio::test]
async fn scrape_write_csv_roundtrip() {
    let dir = std::env::temp_dir().join(format!("drission_it_{}", std::process::id()));
    let csv_path = dir.join("out.csv");
    let rows = vec![vec!["a".to_string(), "b".to_string()]];

    write_csv(&csv_path, &rows).await.expect("write csv");
    let back = tokio::fs::read_to_string(&csv_path)
        .await
        .expect("read csv");
    assert_eq!(back, "a,b\r\n");

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[test]
fn browser_options_builder_is_public() {
    // 仅验证公开 builder 可链式构造(不启动浏览器)。
    let _opts = BrowserOptions::new().headless(true);
}
