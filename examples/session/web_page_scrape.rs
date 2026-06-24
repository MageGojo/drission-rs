//! 阶段4 端到端自验证:WebPage 双模(Session↔Driver)+ cookie 同步 + 表格提取 + 翻页 + CSV/JSON 导出。
//!
//! 用**进程内极简 HTTP 服务**(localhost,完全离线)。每个响应带 `Set-Cookie: sid=web123` 并回显收到的
//! Cookie;`/?page=N` 返回该页表格 + 指向下一页的 `#next`(到第 3 页无 next)。
//!
//! 运行:`cargo run --example web_page_scrape --no-default-features --features camoufox`
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`。

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn page_html(page: u32, recv_cookie: &str) -> String {
    let next = if page < 3 {
        format!("<a id=\"next\" href=\"/?page={}\">下一页</a>", page + 1)
    } else {
        String::new() // 末页:无 next → paginate 停止
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>第{page}页</title></head><body>\
         <p id=\"echo\">cookie:{recv_cookie}</p>\
         <table><tr><th>id</th><th>名称</th></tr>\
         <tr><td>{page}1</td><td>项{page}A</td></tr>\
         <tr><td>{page}2</td><td>项{page}B</td></tr></table>{next}</body></html>"
    )
}

async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            let cookie = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("cookie:"))
                .map(|l| l[7..].trim().to_string())
                .unwrap_or_default();
            let page = path
                .split("page=")
                .nth(1)
                .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1);
            let body = page_html(page, &cookie);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Set-Cookie: sid=web123; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        });
    }
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(drission::Error::from)?;
    let port = listener.local_addr().map_err(drission::Error::from)?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}");
    println!("[*] 本地 HTTP 服务: {base}");

    // ---------- Session 模式:抓表格 + 收 cookie ----------
    let mut page = WebPage::new_session()?;
    let sess_mode = page.mode() == PageMode::Session;
    page.get(&format!("{base}/?page=1")).await?;
    let rows = page.s_ele("tag:table").await?.table()?;
    let sess_table_ok =
        rows.len() == 3 && rows[0] == vec!["id", "名称"] && rows[1] == vec!["11", "项1A"];
    let got_cookie = page
        .session()
        .cookies()
        .iter()
        .any(|c| c.name == "sid" && c.value == "web123");
    println!(
        "[1] Session: 表格行={} 首数据行={:?} cookie(sid)={} (mode_ok={sess_mode})",
        rows.len(),
        rows.get(1),
        got_cookie
    );

    // ---------- 导出 CSV / JSON ----------
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-scrape");
    let csv_path = dir.join("table.csv");
    drission::scrape::write_csv(&csv_path, &rows).await?;
    let csv_back = tokio::fs::read_to_string(&csv_path)
        .await
        .map_err(drission::Error::from)?;
    let records = page.s_ele("tag:table").await?.table_records()?;
    let json = drission::scrape::records_to_json(&records);
    let export_ok =
        csv_back.contains("id,名称") && csv_back.contains("11,项1A") && json.contains("\"名称\"");
    println!(
        "[2] 导出 CSV 首行含表头/数据={} JSON 含字段={} records={}",
        csv_back.contains("id,名称"),
        json.contains("\"名称\""),
        records.len()
    );

    // ---------- 切到 Driver:cookie 同步(服务端回显)+ 实时表格 ----------
    page.change_mode(PageMode::Driver).await?;
    let drv_mode = page.mode() == PageMode::Driver;
    page.get(&format!("{base}/?page=1")).await?;
    let echo = page.ele("#echo").await?.text().await?;
    let cookie_synced = echo.contains("web123"); // 浏览器请求带上了会话灌入的 cookie
    let live_rows = page.ele("tag:table").await?.table().await?;
    let drv_table_ok = live_rows.len() == 3 && live_rows[1] == vec!["11", "项1A"];
    println!(
        "[3] Driver: echo={echo:?} cookie 同步={cookie_synced} 实时表格行={} (mode_ok={drv_mode})",
        live_rows.len()
    );

    // ---------- 翻页:点 #next 翻到末页,收集每页首数据行 ----------
    let tab = page.tab().expect("driver 已启动").clone();
    let tab_cb = tab.clone();
    let collected = tab
        .paginate("#next", 5, move |_i| {
            let tab = tab_cb.clone();
            async move {
                let rows = tab.ele("tag:table").await?.table().await?;
                Ok::<String, drission::Error>(
                    rows.get(1)
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or_default(),
                )
            }
        })
        .await?;
    // 三页首数据行 id 应为 11 / 21 / 31。
    let paginate_ok = collected == vec!["11", "21", "31"];
    println!("[4] paginate 各页首 id={collected:?} (ok={paginate_ok})");

    let pass = sess_mode
        && sess_table_ok
        && got_cookie
        && export_ok
        && drv_mode
        && cookie_synced
        && drv_table_ok
        && paginate_ok;
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );
    page.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("web_page_scrape 自验证未通过"))
    }
}
