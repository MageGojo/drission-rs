//! 采集导出助手:二维表 / 记录列表 → CSV / JSON。
//!
//! 配合 Camoufox 后端的表格提取(`StaticElement::table` / `Element::table`)与翻页(`Tab::paginate`),
//! 把抓到的结构化数据落成文件。CSV 自己按 RFC 4180 转义(零额外依赖);JSON 走 `serde_json`。
//!
//! ```ignore
//! let rows = tab.ele("tag:table").await?.table().await?;
//! drission::scrape::write_csv("out.csv", &rows).await?;
//! ```

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::Result;

/// 把二维表(行 × 列)编码为 CSV 文本。
///
/// RFC 4180 转义:字段含 `,` / `"` / 换行时用双引号包裹、内部 `"` 翻倍;行尾用 `\r\n`。
pub fn rows_to_csv(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row.iter().map(|f| csv_field(f)).collect();
        out.push_str(&line.join(","));
        out.push_str("\r\n");
    }
    out
}

/// 把记录列表按 `headers` 顺序编码为 CSV(首行为表头);记录缺某列则留空。
pub fn records_to_csv(records: &[HashMap<String, String>], headers: &[String]) -> String {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(records.len() + 1);
    rows.push(headers.to_vec());
    for rec in records {
        rows.push(
            headers
                .iter()
                .map(|h| rec.get(h).cloned().unwrap_or_default())
                .collect(),
        );
    }
    rows_to_csv(&rows)
}

/// 记录列表 → JSON 数组字符串(每条记录一个对象,值均为字符串)。
pub fn records_to_json(records: &[HashMap<String, String>]) -> String {
    let arr: Vec<Value> = records
        .iter()
        .map(|r| {
            let mut m = Map::new();
            for (k, v) in r {
                m.insert(k.clone(), Value::String(v.clone()));
            }
            Value::Object(m)
        })
        .collect();
    serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_else(|_| "[]".to_string())
}

/// 把二维表写成 CSV 文件(父目录自动创建)。
pub async fn write_csv(path: impl AsRef<Path>, rows: &[Vec<String>]) -> Result<()> {
    write_text(path, &rows_to_csv(rows)).await
}

/// 把记录列表写成 JSON 文件(父目录自动创建)。
pub async fn write_json(path: impl AsRef<Path>, records: &[HashMap<String, String>]) -> Result<()> {
    write_text(path, &records_to_json(records)).await
}

async fn write_text(path: impl AsRef<Path>, content: &str) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    Ok(())
}

/// 单个 CSV 字段转义。
fn csv_field(f: &str) -> String {
    if f.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escaping() {
        let rows = vec![
            vec!["a".into(), "b,c".into(), "d\"e".into()],
            vec!["x\ny".into(), "z".into(), "".into()],
        ];
        let csv = rows_to_csv(&rows);
        assert_eq!(csv, "a,\"b,c\",\"d\"\"e\"\r\n\"x\ny\",z,\r\n");
    }

    #[test]
    fn records_csv_orders_by_headers() {
        let mut r0 = HashMap::new();
        r0.insert("name".to_string(), "苹果".to_string());
        r0.insert("price".to_string(), "3".to_string());
        let headers = vec!["name".to_string(), "price".to_string()];
        let csv = records_to_csv(&[r0], &headers);
        assert_eq!(csv, "name,price\r\n苹果,3\r\n");
    }

    #[test]
    fn records_json_shape() {
        let mut r0 = HashMap::new();
        r0.insert("k".to_string(), "v".to_string());
        let j = records_to_json(&[r0]);
        let parsed: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(parsed[0]["k"], "v");
    }
}
