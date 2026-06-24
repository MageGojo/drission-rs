//! 文件上传三种写法端到端自验证(对标 DrissionPage 的上传能力)。
//!
//! 覆盖开发者会用到的全部上传姿势,看本文件即可照抄,无需读源码:
//!   1) **自然上传 · 分步**(DP `tab.set.upload_files` → 点击 → `tab.wait.upload_paths_inputted`):
//!      用于"点一个按钮→弹系统文件框"、没有可直接定位 `<input type=file>` 的场景。
//!   2) **自然上传 · 一步**(DP `ele.click.to_upload`):上面三步的便捷封装。
//!   3) **传统直接赋值**(DP 对文件输入框 `input` → 本库 `ele.set_files`):已能拿到那个
//!      `<input type=file>` 时最省事。
//!
//! 全程本地 `file://` 页,不依赖网络,确定性强。
//!
//! 运行:`cargo run --example file_upload --no-default-features --features camoufox`(默认 headless;`HL=0 cargo run --example file_upload --no-default-features --features camoufox` 看界面)
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use drission::prelude::*;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>upload</title></head>
<body>
  <h1>upload demo</h1>

  <!-- 隐藏的真正 input,由按钮代理唤起:这就是最常见的"自然上传"场景 -->
  <input id="real" type="file" style="display:none">
  <button id="pick">选择文件</button>
  <div id="picked">none</div>

  <!-- 可直接定位的 input:传统写法用 set_files 直接赋值 -->
  <input id="direct" type="file">
  <div id="direct_name">none</div>

  <script>
    const real = document.getElementById('real');
    document.getElementById('pick').addEventListener('click', () => {
      real.click();
    });
    real.addEventListener('change', () => {
      document.getElementById('picked').textContent =
        Array.from(real.files).map(f => f.name).join(',') || 'none';
    });
    const direct = document.getElementById('direct');
    direct.addEventListener('change', () => {
      document.getElementById('direct_name').textContent =
        Array.from(direct.files).map(f => f.name).join(',') || 'none';
    });
  </script>
</body></html>"#;

/// 读 `#id` 文件输入框当前第一个文件名(取不到返回空串)。
async fn file_name(tab: &Tab, id: &str) -> drission::Result<String> {
    let v = tab
        .run_js(&format!(
            "(document.getElementById('{id}').files[0]||{{}}).name||''"
        ))
        .await?;
    Ok(v.as_str().unwrap_or_default().to_string())
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 准备本地页面与待上传文件。写到项目 target 目录(用户 home 下)——Camoufox/Firefox 的 macOS
    // 内容进程沙箱拒读 `/var/folders` 系统临时目录,放那儿会让 file:// 加载成空白文档。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-upload");
    tokio::fs::create_dir_all(&dir).await?;
    let page_path = dir.join("page.html");
    tokio::fs::write(&page_path, PAGE).await?;
    let file_a = dir.join("alpha.txt");
    let file_b = dir.join("bravo.txt");
    let file_c = dir.join("charlie.txt");
    tokio::fs::write(&file_a, b"alpha").await?;
    tokio::fs::write(&file_b, b"bravo").await?;
    tokio::fs::write(&file_c, b"charlie").await?;
    let (pa, pb, pc) = (
        file_a.to_string_lossy().to_string(),
        file_b.to_string_lossy().to_string(),
        file_c.to_string_lossy().to_string(),
    );
    let url = format!("file://{}", page_path.display());

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Camoufox(headless={headless})…");
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    let ok_get = tab.get(&url).await?;
    println!("[*] get({url}) ok={ok_get}");

    // 等到目标页真正成为当前文档(导航后执行上下文有切换窗口,首个 html 可能还是 about:blank)。
    let mut loaded = false;
    for _ in 0..20 {
        if tab.html().await?.len() >= 80 {
            loaded = true;
            break;
        }
        tab.wait().secs(0.2).await;
    }
    if !loaded {
        return Err(drission::Error::msg(format!(
            "页面未正确加载(疑似沙箱拒读):{url}"
        )));
    }

    // ---------- 写法 1:自然上传 · 分步(set_upload_files → click → wait_upload_paths_inputted) ----------
    // 1. 先武装(记下要上传的文件 + 注入一次性 hook,拦掉原生系统文件框)
    tab.set().upload_files(&[pa.as_str()]).await?;
    // 2. 点击会唤起文件框的按钮(这里按钮内部又 `real.click()` 程序化点了隐藏的 <input type=file>)
    tab.ele("#pick").await?.click().await?;
    // 3. 等待文件路径被填入(超时返回 false,不报错)
    let inputted = tab
        .wait()
        .upload_paths_inputted(Some(std::time::Duration::from_secs(5)))
        .await?;
    tab.wait().secs(0.2).await; // 给 change 事件一点点落地时间(只为读 #picked 展示)
    let name1 = file_name(&tab, "real").await?;
    let picked1 = tab.ele("#picked").await?.text().await?;
    let ok1 = inputted && name1 == "alpha.txt" && picked1 == "alpha.txt";
    println!(
        "[1] 自然上传·分步: inputted={inputted} files[0]={name1:?} #picked={picked1:?} (ok={ok1})"
    );

    // ---------- 写法 2:自然上传 · 一步(ele.click.to_upload) ----------
    // 一行搞定:武装 + 点击本元素 + 等待。会替换上一次的文件。
    let done2 = tab
        .ele("#pick")
        .await?
        .click_to_upload(&[pb.as_str()], None)
        .await?;
    tab.wait().secs(0.2).await;
    let name2 = file_name(&tab, "real").await?;
    let picked2 = tab.ele("#picked").await?.text().await?;
    let ok2 = done2 && name2 == "bravo.txt" && picked2 == "bravo.txt";
    println!("[2] 自然上传·一步: done={done2} files[0]={name2:?} #picked={picked2:?} (ok={ok2})");

    // ---------- 写法 3:传统直接赋值(ele.set_files,已能定位到 <input type=file>) ----------
    tab.ele("#direct").await?.set_files(&[pc.as_str()]).await?;
    tab.wait().secs(0.2).await;
    let name3 = file_name(&tab, "direct").await?;
    let dname3 = tab.ele("#direct_name").await?.text().await?;
    let ok3 = name3 == "charlie.txt" && dname3 == "charlie.txt";
    println!("[3] 传统直接赋值: files[0]={name3:?} #direct_name={dname3:?} (ok={ok3})");

    let pass = ok_get && ok1 && ok2 && ok3;
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("file_upload 自验证未通过"))
    }
}
