//! 湖北省人民政府「政府文件」公开列表采集示例:
//! 浏览器自动化动态翻页,抽取列表标题/日期/链接,再进入每条内容页提取正文并导出 CSV。
//!
//! 运行(默认 CDP 后端 / Google Chrome,无需额外 feature):
//!   cargo run --example hubei_zfwj_csv
//!   cargo run --example hubei_zfwj_csv -- https://www.hubei.gov.cn/zfwj/list1.shtml 5 target/hubei_zfwj/zfwj.csv
//!   HL=0 cargo run --example hubei_zfwj_csv   # 有头观察页面
//!
//! 参数:
//!   1. 起始列表页,默认 https://www.hubei.gov.cn/zfwj/list1.shtml
//!   2. 最多抓多少页,默认 all/全部(分页 URL 从页面里的“下一页”动态发现)
//!   3. CSV 输出路径,默认 target/hubei_zfwj/zfwj.csv

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use drission::prelude::*;
use serde::Deserialize;
use tokio::time::sleep;

const DEFAULT_START: &str = "https://www.hubei.gov.cn/zfwj/list1.shtml";
const DEFAULT_OUT: &str = "target/hubei_zfwj/zfwj.csv";

#[derive(Debug, Clone)]
struct GovFile {
    list_page: usize,
    list_title: String,
    list_date: String,
    url: String,
    detail_title: String,
    publish_date: String,
    source: String,
    document_no: String,
    content: String,
    content_links: String,
}

#[derive(Debug, Deserialize)]
struct JsListPage {
    records: Vec<JsRecord>,
    next_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsRecord {
    title: String,
    date: String,
    url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DetailData {
    title: String,
    publish_date: String,
    source: String,
    document_no: String,
    content: String,
    content_links: String,
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn,drission::protocol::connection=off")
        .init();

    let mut args = std::env::args().skip(1);
    let start_url = args.next().unwrap_or_else(|| DEFAULT_START.to_string());
    let max_pages = args.next().and_then(|s| parse_max_pages(&s));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT));
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);

    println!(
        "==== 湖北省政府文件列表采集 ====\n起始页 = {start_url}\n最多页 = {}\n输出   = {}\nheadless = {headless}\n",
        max_pages
            .map(|n| n.to_string())
            .unwrap_or_else(|| "全部".to_string()),
        out.display()
    );

    let page = Page::with(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1365, 900)
            .locale("zh-CN")
            .timezone("Asia/Shanghai")
            .full_ua_metadata(true),
    )
    .await?;
    page.set_timeout(Duration::from_secs(35));

    let mut all = Vec::new();
    let mut seen = HashSet::new();
    let mut current_url = Some(start_url.clone());
    let mut page_no = 1usize;

    while let Some(url) = current_url.clone() {
        if max_pages.is_some_and(|limit| page_no > limit) {
            break;
        }
        let page_progress = max_pages
            .map(|limit| format!("{page_no}/{limit}"))
            .unwrap_or_else(|| format!("{page_no}/全部"));
        println!("[{page_progress}] 打开 {url}");
        page.get(&url).await?;

        let list_page = wait_list_page(&page, Duration::from_secs(25)).await?;
        if list_page.records.is_empty() {
            write_debug_snapshot(&page).await?;
            println!("  未抽到记录,停止。");
            break;
        }

        let before = all.len();
        let total = list_page.records.len();
        for (idx, r) in list_page.records.into_iter().enumerate() {
            let key = format!("{}|{}", r.title, r.url);
            if !seen.insert(key) {
                continue;
            }

            println!(
                "  详情 {}/{} {}",
                idx + 1,
                total,
                truncate_for_log(&r.title, 42)
            );
            page.get(&r.url).await?;
            let detail = wait_detail(&page, Duration::from_secs(25)).await?;
            all.push(GovFile {
                list_page: page_no,
                list_title: r.title,
                list_date: r.date,
                url: r.url,
                detail_title: detail.title,
                publish_date: detail.publish_date,
                source: detail.source,
                document_no: detail.document_no,
                content: detail.content,
                content_links: detail.content_links,
            });
            sleep(Duration::from_millis(250)).await;
        }
        println!("  本页新增 {} 条,累计 {} 条", all.len() - before, all.len());

        current_url = list_page.next_url;
        page_no += 1;
        sleep(Duration::from_millis(600)).await;
    }

    let rows = to_csv_rows(&all);
    drission::scrape::write_csv(&out, &rows).await?;
    println!("\nCSV 已写出:{} ({} 条记录)", out.display(), all.len());

    page.quit().await?;
    Ok(())
}

async fn wait_list_page(page: &Page, timeout: Duration) -> drission::Result<JsListPage> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut tried_reload = false;

    loop {
        let list_page = extract_list_page(page).await?;
        if !list_page.records.is_empty() {
            return Ok(list_page);
        }

        if tokio::time::Instant::now() >= deadline {
            return Ok(JsListPage {
                records: Vec::new(),
                next_url: None,
            });
        }

        let html = page.html().await.unwrap_or_default();
        if !tried_reload && looks_like_js_challenge(&html) {
            tried_reload = true;
            sleep(Duration::from_secs(2)).await;
            page.reload().await?;
        } else {
            sleep(Duration::from_millis(500)).await;
        }
    }
}

async fn extract_list_page(page: &Page) -> drission::Result<JsListPage> {
    let value = page.run_js(EXTRACT_LIST_JS).await?;
    let Some(s) = value.as_str() else {
        return Ok(JsListPage {
            records: Vec::new(),
            next_url: None,
        });
    };
    Ok(serde_json::from_str(s)?)
}

async fn wait_detail(page: &Page, timeout: Duration) -> drission::Result<DetailData> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut tried_reload = false;

    loop {
        let detail = extract_detail(page).await?;
        if detail.content.chars().count() > 30 || !detail.document_no.is_empty() {
            return Ok(detail);
        }

        if tokio::time::Instant::now() >= deadline {
            return Ok(detail);
        }

        let html = page.html().await.unwrap_or_default();
        if !tried_reload && looks_like_js_challenge(&html) {
            tried_reload = true;
            sleep(Duration::from_secs(2)).await;
            page.reload().await?;
        } else {
            sleep(Duration::from_millis(500)).await;
        }
    }
}

async fn extract_detail(page: &Page) -> drission::Result<DetailData> {
    let value = page.run_js(EXTRACT_DETAIL_JS).await?;
    let Some(s) = value.as_str() else {
        return Ok(DetailData::default());
    };
    Ok(serde_json::from_str(s)?)
}

async fn write_debug_snapshot(page: &Page) -> drission::Result<()> {
    let dir = PathBuf::from("target/hubei_zfwj");
    tokio::fs::create_dir_all(&dir).await?;
    let html_path = dir.join("debug.html");
    let shot_path = dir.join("debug.png");
    let html = page.html().await.unwrap_or_default();
    let title = page.title().await.unwrap_or_default();
    let url = page.url().await.unwrap_or_default();
    tokio::fs::write(&html_path, html).await?;
    let _ = page.get_screenshot(&shot_path, true).await;
    println!(
        "  当前页面未匹配到列表:title={title:?} url={url}\n  诊断文件:{} / {}",
        html_path.display(),
        shot_path.display()
    );
    Ok(())
}

fn to_csv_rows(records: &[GovFile]) -> Vec<Vec<String>> {
    let mut rows = Vec::with_capacity(records.len() + 1);
    rows.push(vec![
        "list_page".to_string(),
        "list_date".to_string(),
        "list_title".to_string(),
        "url".to_string(),
        "detail_title".to_string(),
        "publish_date".to_string(),
        "source".to_string(),
        "document_no".to_string(),
        "content".to_string(),
        "content_links".to_string(),
    ]);
    rows.extend(records.iter().map(|r| {
        vec![
            r.list_page.to_string(),
            r.list_date.clone(),
            r.list_title.clone(),
            r.url.clone(),
            r.detail_title.clone(),
            r.publish_date.clone(),
            r.source.clone(),
            r.document_no.clone(),
            r.content.clone(),
            r.content_links.clone(),
        ]
    }));
    rows
}

fn looks_like_js_challenge(html: &str) -> bool {
    html.contains("$_ss") || html.contains("nsd=") || html.contains("Precondition Failed")
}

fn parse_max_pages(s: &str) -> Option<usize> {
    let trimmed = s.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("all")
        || trimmed.eq_ignore_ascii_case("全部")
    {
        return None;
    }
    trimmed.parse::<usize>().ok().filter(|n| *n > 0)
}

fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

const EXTRACT_LIST_JS: &str = r#"
(() => {
  const clean = (s) => (s || '').replace(/[\u200B-\u200D\uFEFF]/g, '').replace(/\s+/g, ' ').trim();
  const badTitle = /^(首页|政府信息公开|政策|文件|搜索|上一页|下一页|末页|尾页|更多|返回)$/;
  const badFileLink = /^(\[?解读\d*\]?|\[?图解\d*\]?|\[?附件\d*\]?)$/;
  const dateRe = /(\d{4}[-/.年]\d{1,2}[-/.月]\d{1,2}日?)/;
  const items = [];
  const seen = new Set();

  const listItems = [
    ...document.querySelectorAll('.hbgov-newslist-itemheight-18px > li'),
    ...document.querySelectorAll('.hbgov-list-block li')
  ];

  for (const li of listItems) {
    const scopeText = clean(li.innerText || li.textContent);
    const date = (scopeText.match(dateRe) || [])[1] || '';
    if (!date) {
      continue;
    }

    const a = [...li.querySelectorAll('a[href]')].find((link) => {
      const text = clean(link.innerText || link.textContent);
      const href = link.getAttribute('href') || '';
      return text.length >= 5 && !badTitle.test(text) && !badFileLink.test(text) && href && !href.startsWith('#') && !href.startsWith('javascript:');
    });
    if (!a) {
      continue;
    }

    const text = clean(a.innerText || a.textContent);
    const title = clean(a.getAttribute('title')) || text;
    const href = a.getAttribute('href') || '';
    if (title.length < 5 || badTitle.test(title) || badFileLink.test(title)) {
      continue;
    }

    let url = '';
    try {
      url = new URL(href, location.href).href;
    } catch (_) {
      continue;
    }

    const key = title + '|' + url;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    items.push({ title, date: date.replace(/[年月.\/]/g, '-').replace(/日$/, ''), url });
  }

  const nav = document.querySelector('#pages-nav, .hbgov-pagination');
  let nextUrl = null;
  if (nav) {
    const active = nav.querySelector('li.active');
    const nextA = active && active.nextElementSibling ? active.nextElementSibling.querySelector('a[href]') : null;
    const fallbackA = nav.querySelector('a[aria-label="Next"][href]');
    const a = nextA || fallbackA;
    if (a) {
      const href = a.getAttribute('href') || '';
      if (href && !href.startsWith('javascript:') && href !== '.' && href !== '#') {
        try {
          nextUrl = new URL(href, location.href).href;
        } catch (_) {}
      }
    }
  }

  return JSON.stringify({ records: items, next_url: nextUrl });
})()
"#;

const EXTRACT_DETAIL_JS: &str = r#"
(() => {
  const clean = (s) => (s || '').replace(/[\u200B-\u200D\uFEFF]/g, '').replace(/\s+/g, ' ').trim();
  const cleanContent = (s) => clean(s)
    .replace(/分享到\s*微信.*$/g, '')
    .replace(/扫一扫在手机上查看当前页面/g, '')
    .replace(/关闭打印/g, '')
    .trim();
  const pickMeta = (names) => {
    for (const name of names) {
      const el = document.querySelector(`meta[name="${name}"], meta[property="${name}"]`);
      const val = clean(el && el.getAttribute('content'));
      if (val) return val;
    }
    return '';
  };
  const firstText = (selectors) => {
    for (const sel of selectors) {
      const el = document.querySelector(sel);
      const val = clean(el && (el.innerText || el.textContent));
      if (val) return val;
    }
    return '';
  };
  const bodyText = clean(document.body ? document.body.innerText : '');
  const dateRe = /(\d{4}[-/.年]\d{1,2}[-/.月]\d{1,2}日?)/;
  const normalizeDate = (s) => clean(s).replace(/[年月.\/]/g, '-').replace(/日$/, '');
  const title = pickMeta(['ArticleTitle', 'title', 'og:title'])
    || firstText(['h1', '.hbgov-article-title', '.article-title', '.detail-title', '.title'])
    || clean(document.title).replace(/\s*-\s*湖北省人民政府门户网站\s*$/, '');
  const publishDate = normalizeDate(
    pickMeta(['PubDate', 'publishdate', 'publishDate'])
    || ((bodyText.match(/(?:发布时间|发布日期|时间|日期)[：:\s]*(\d{4}[-/.年]\d{1,2}[-/.月]\d{1,2}日?)/) || [])[1] || '')
    || ((bodyText.match(dateRe) || [])[1] || '')
  );
  const source = pickMeta(['ContentSource', 'source', 'Source'])
    || ((bodyText.match(/(?:来源|信息来源)[：:\s]*([^\s　|]+)/) || [])[1] || '');
  const documentNo = ((bodyText.match(/([\u4e00-\u9fa5]{1,6}政(?:办)?(?:发|函|令|文)〔\d{4}〕\d+号)/) || [])[1] || '');

  const contentSelectors = [
    '#zoom',
    '.TRS_Editor',
    '.hbgov-article-content',
    '.hbgov-detail-content',
    '.article-content',
    '.detail-content',
    '.content',
    'article'
  ];
  let contentEl = null;
  let content = '';
  for (const sel of contentSelectors) {
    for (const el of document.querySelectorAll(sel)) {
      const text = cleanContent(el.innerText || el.textContent);
      if (text.length > content.length) {
        content = text;
        contentEl = el;
      }
    }
  }
  if (!content) {
    for (const el of document.querySelectorAll('main, .container, .hbgov-bfc-block, section, div')) {
      const text = cleanContent(el.innerText || el.textContent);
      if (text.length > content.length && text.length < bodyText.length * 0.9) {
        content = text;
        contentEl = el;
      }
    }
  }
  if (title && content.startsWith(title)) {
    content = cleanContent(content.slice(title.length));
  }

  const linkScope = contentEl || document.body;
  const links = [];
  const seen = new Set();
  for (const a of linkScope.querySelectorAll('a[href]')) {
    const text = clean(a.innerText || a.textContent || a.getAttribute('title'));
    const href = a.getAttribute('href') || '';
    if (!href || href.startsWith('#') || href.startsWith('javascript:')) continue;
    let url = '';
    try {
      url = new URL(href, location.href).href;
    } catch (_) {
      continue;
    }
    const key = text + '|' + url;
    if (seen.has(key)) continue;
    seen.add(key);
    links.push({ text, url });
  }

  return JSON.stringify({
    title,
    publish_date: publishDate,
    source,
    document_no: documentNo,
    content,
    content_links: JSON.stringify(links)
  });
})()
"#;
