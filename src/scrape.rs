//! 采集导出助手:二维表 / 记录列表 → CSV / JSON。
//!
//! 配合 Camoufox 后端的表格提取(`StaticElement::table` / `Element::table`)与翻页(`Tab::paginate`),
//! 把抓到的结构化数据落成文件或打印到终端。CSV 自己按 RFC 4180 转义(零额外依赖);
//! JSON 走 `serde_json`;终端表格走内置轻量格式化。
//!
//! ```ignore
//! let rows = tab.ele("tag:table").await?.table().await?;
//! drission::scrape::print_table(&rows)?;
//! drission::scrape::write_csv("out.csv", &rows).await?;
//! ```

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;

use serde_json::{Map, Value};

use crate::Result;

/// 终端表格打印选项。
#[derive(Debug, Clone, Copy)]
pub struct TableOptions {
    /// 每列最大显示宽度。超过会截断,避免长标题把终端撑爆。
    pub max_col_width: usize,
    /// 是否把第一行当表头,在其后画一条分隔线。
    pub header: bool,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self {
            max_col_width: 40,
            header: true,
        }
    }
}

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

/// 把二维表渲染为适合终端查看的 Unicode 表格。
///
/// 默认把第一行视为表头,并把单列最大宽度限制为 40。只想快速看数据时可直接配合
/// [`print_table`] 使用;需要自定义列宽/表头行为时用 [`rows_to_table_with_options`]。
pub fn rows_to_table(rows: &[Vec<String>]) -> String {
    rows_to_table_with_options(rows, &TableOptions::default())
}

/// 按自定义选项把二维表渲染为终端表格。
pub fn rows_to_table_with_options(rows: &[Vec<String>], options: &TableOptions) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let col_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if col_count == 0 {
        return String::new();
    }

    let max_col_width = options.max_col_width.max(1);
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            (0..col_count)
                .map(|i| {
                    row.get(i)
                        .map(|s| table_cell(s, max_col_width))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();

    let mut widths = vec![1usize; col_count];
    for row in &table {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(display_width(cell));
        }
    }

    let mut out = String::new();
    out.push_str(&table_border('┌', '┬', '┐', &widths));
    for (i, row) in table.iter().enumerate() {
        out.push_str(&table_row(row, &widths));
        if options.header && i == 0 && table.len() > 1 {
            out.push_str(&table_border('├', '┼', '┤', &widths));
        }
    }
    out.push_str(&table_border('└', '┴', '┘', &widths));
    out
}

/// 把二维表直接打印到标准输出。
pub fn print_table(rows: &[Vec<String>]) -> Result<()> {
    print_table_with_options(rows, &TableOptions::default())
}

/// 按自定义选项把二维表打印到标准输出。
pub fn print_table_with_options(rows: &[Vec<String>], options: &TableOptions) -> Result<()> {
    let table = rows_to_table_with_options(rows, options);
    let mut stdout = io::stdout().lock();
    stdout.write_all(table.as_bytes())?;
    stdout.flush()?;
    Ok(())
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

fn table_cell(cell: &str, max_width: usize) -> String {
    let clean = cell.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_width(&clean, max_width)
}

fn table_border(left: char, mid: char, right: char, widths: &[usize]) -> String {
    let mut out = String::new();
    out.push(left);
    for (i, width) in widths.iter().enumerate() {
        if i > 0 {
            out.push(mid);
        }
        out.push_str(&"─".repeat(width + 2));
    }
    out.push(right);
    out.push('\n');
    out
}

fn table_row(row: &[String], widths: &[usize]) -> String {
    let mut out = String::new();
    out.push('│');
    for (cell, width) in row.iter().zip(widths) {
        out.push(' ');
        out.push_str(cell);
        out.push_str(&" ".repeat(width.saturating_sub(display_width(cell)) + 1));
        out.push('│');
    }
    out.push('\n');
    out
}

fn truncate_width(s: &str, max_width: usize) -> String {
    if display_width(s) <= max_width {
        return s.to_string();
    }

    let marker = if max_width >= 2 { "…" } else { "" };
    let marker_width = display_width(marker);
    let limit = max_width.saturating_sub(marker_width);
    let mut out = String::new();
    let mut width = 0usize;

    for ch in s.chars() {
        let w = char_width(ch);
        if width + w > limit {
            break;
        }
        out.push(ch);
        width += w;
    }
    out.push_str(marker);
    out
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn char_width(ch: char) -> usize {
    if ch.is_control() {
        0
    } else if ch.is_ascii() {
        1
    } else {
        2
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

    #[test]
    fn rows_table_shape() {
        let rows = vec![
            vec!["name".into(), "price".into()],
            vec!["Apple".into(), "3".into()],
        ];
        let table = rows_to_table(&rows);
        assert!(table.contains("│ name  │ price │"));
        assert!(table.contains("│ Apple │ 3     │"));
    }

    #[test]
    fn rows_table_handles_cjk_width_and_truncation() {
        let rows = vec![
            vec!["标题".into(), "价格".into()],
            vec!["非常非常长的中文标题".into(), "12".into()],
        ];
        let table = rows_to_table_with_options(
            &rows,
            &TableOptions {
                max_col_width: 8,
                header: true,
            },
        );
        assert!(table.contains("非常非…"));
        assert!(table.contains("│ 标题     │ 价格 │"));
    }
}
