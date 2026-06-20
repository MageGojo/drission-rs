//! 纯函数基准:Juggler 帧编解码 / DP 定位解析 / 采集导出。
//!
//! 这些都不开浏览器、确定性、跑得快,适合做性能回归护栏(对标本库“高性能/高并发”定位)。
//! 运行:`cargo bench --bench parsing`。

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use drission::codec::{FrameDecoder, encode_frame};
use drission::locator::{parse, parse_static};
use drission::scrape::{records_to_json, rows_to_csv};

fn bench_locator(c: &mut Criterion) {
    let selectors = [
        "#kw",
        ".title.foo",
        "css:div.box",
        "tag:li",
        "@id:kw",
        "@class=project list",
        "@text():登录",
        "text:提交",
        "xpath://div[@id='a']",
        "提交",
    ];
    c.bench_function("locator::parse", |b| {
        b.iter(|| {
            for &s in &selectors {
                black_box(parse(black_box(s)));
            }
        })
    });
    c.bench_function("locator::parse_static", |b| {
        b.iter(|| {
            for &s in &selectors {
                black_box(parse_static(black_box(s)));
            }
        })
    });
}

fn bench_codec(c: &mut Criterion) {
    // 典型 Juggler 帧:一批 JSON 消息用 \0 分隔,模拟从管道读到的连续字节。
    let mut stream = Vec::new();
    for i in 0..64 {
        let msg = format!(
            r#"{{"id":{i},"method":"Runtime.evaluate","params":{{"expression":"1+{i}"}}}}"#
        );
        stream.extend_from_slice(&encode_frame(msg.as_bytes()));
    }
    c.bench_function("codec::decode_64_frames", |b| {
        b.iter(|| {
            let mut d = FrameDecoder::new();
            d.push(black_box(&stream));
            let mut n = 0;
            while let Some(f) = d.next_frame() {
                black_box(&f);
                n += 1;
            }
            black_box(n)
        })
    });
}

fn bench_scrape(c: &mut Criterion) {
    let mut rows: Vec<Vec<String>> = vec![vec!["name".into(), "price".into(), "note".into()]];
    for i in 0..200 {
        rows.push(vec![
            format!("商品{i}"),
            format!("{i}.99"),
            "含 \"引号\", 逗号\n换行".into(),
        ]);
    }
    c.bench_function("scrape::rows_to_csv_200", |b| {
        b.iter(|| black_box(rows_to_csv(black_box(&rows))))
    });

    let mut records: Vec<HashMap<String, String>> = Vec::with_capacity(200);
    for i in 0..200 {
        let mut m = HashMap::new();
        m.insert("name".to_string(), format!("商品{i}"));
        m.insert("price".to_string(), format!("{i}"));
        records.push(m);
    }
    c.bench_function("scrape::records_to_json_200", |b| {
        b.iter(|| black_box(records_to_json(black_box(&records))))
    });
}

criterion_group!(benches, bench_locator, bench_codec, bench_scrape);
criterion_main!(benches);
