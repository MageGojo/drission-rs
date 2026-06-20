//! 端到端案例:用库打开 apizero 登录页 → 填账号/密码 + **OCR 识别验证码并填入** → 点登录 →
//! 读站点反馈判断**验证码是否识别通过**。不求登录成功(用假账号);只要反馈不是"验证码错误"
//! (而是账号/密码错误)即说明 OCR 识别对了。需 `--features ocr`。
//!
//! 运行:`HL=0 N=5 cargo run --example apizero_login --no-default-features --features camoufox,ocr`

use std::time::Duration;

use drission::prelude::*;
use serde_json::json;
use tokio::time::sleep;

const URL: &str = "https://apizero.cn/login";
const CAPTCHA_IMG: &str = "xpath:/html/body/main/div/main/div/form/div[3]/button/img";
const EMAIL: &str = "input[type=email]";
const PASSWORD: &str = "input[type=password]";
const CAPTCHA_INPUT: &str = "input[placeholder='请输入图中字符']";
const LOGIN_BTN: &str = "button[type=submit]";

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let n: u32 = std::env::var("N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let email = std::env::var("EMAIL").unwrap_or_else(|_| "test@example.com".into());
    let password = std::env::var("PASSWORD").unwrap_or_else(|_| "Test123456".into());

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    tab.apply_pointer_stealth().await?;
    println!("[*] 打开 {URL}");
    tab.get(URL).await?;
    sleep(Duration::from_secs(3)).await;

    let mut captcha_pass = 0u32;
    for k in 1..=n {
        let _ = tab
            .wait()
            .ele_displayed(CAPTCHA_IMG, Some(Duration::from_secs(8)))
            .await;
        // React 兼容方式填邮箱/密码。
        let okm = fill(&tab, EMAIL, &email).await;
        let okp = fill(&tab, PASSWORD, &password).await;
        // OCR 识别验证码 → 填入(大写匹配图中显示;验证码多大小写不敏感)。
        let code = match tab.ocr_image(CAPTCHA_IMG).await {
            Ok(c) => c.to_uppercase(),
            Err(e) => {
                println!("[!] #{k}:OCR 失败 {e}");
                refresh_captcha(&tab).await;
                continue;
            }
        };
        let okc = fill(&tab, CAPTCHA_INPUT, &code).await;
        sleep(Duration::from_millis(250)).await;
        // 点登录。
        let _ = tab
            .run_js(&format!(
                "(function(){{var e=document.querySelector({s}); if(e)e.click();}})()",
                s = json!(LOGIN_BTN)
            ))
            .await;
        // 轮询读站点结果短语。
        let mut msg = String::new();
        for _ in 0..16 {
            sleep(Duration::from_millis(400)).await;
            msg = read_result(&tab).await;
            if !msg.is_empty() {
                break;
            }
        }
        let (pass, verdict) = classify(&msg);
        if pass {
            captcha_pass += 1;
        }
        println!(
            "[*] #{k}:填表(邮箱{} 密码{} 验证码{})  OCR={code:<6} 站点反馈={msg:?} → {verdict}",
            yn(okm),
            yn(okp),
            yn(okc)
        );
        refresh_captcha(&tab).await;
        sleep(Duration::from_millis(500)).await;
    }

    println!(
        "\n==== apizero 登录案例:验证码识别通过 {captcha_pass}/{n} 次(账号密码假的,故不会登录成功)===="
    );
    if !headless {
        sleep(Duration::from_secs(3)).await;
    }
    browser.quit().await?;
    Ok(())
}

fn yn(b: bool) -> &'static str {
    if b { "✓" } else { "✗" }
}

/// React 兼容填值:用原生 value setter 赋值 + 派发 input/change 事件,返回是否填入成功(回读校验)。
async fn fill(tab: &Tab, css: &str, value: &str) -> bool {
    let js = format!(
        r#"(function(){{var e=document.querySelector({sel}); if(!e)return false;
  var proto=e.tagName==='TEXTAREA'?window.HTMLTextAreaElement.prototype:window.HTMLInputElement.prototype;
  var set=Object.getOwnPropertyDescriptor(proto,'value').set; set.call(e,{val});
  e.dispatchEvent(new Event('input',{{bubbles:true}})); e.dispatchEvent(new Event('change',{{bubbles:true}}));
  return e.value==={val};}})()"#,
        sel = json!(css),
        val = json!(value)
    );
    tab.run_js(&js)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 从页面正文扫描结果短语(站点把"账号或密码错误/验证码错误"等渲染在表单里,非 toast)。
async fn read_result(tab: &Tab) -> String {
    tab.run_js(
        r#"(function(){var t=document.body.innerText||'';
  var pats=['验证码错误','验证码不正确','验证码已过期','图形验证码错误','请输入验证码','验证码有误',
            '账号或密码错误','邮箱或密码错误','用户名或密码错误','密码错误','用户不存在','账号不存在','邮箱未注册',
            '登录成功','欢迎回来您已登录'];
  for(var i=0;i<pats.length;i++){if(t.indexOf(pats[i])>=0)return pats[i];}
  return '';})()"#,
    )
    .await
    .ok()
    .and_then(|v| v.as_str().map(|s| s.to_string()))
    .unwrap_or_default()
}

/// 判断验证码是否通过。含"验证码…"→ 未过;只报账号/密码/邮箱错 → 已过(OCR 对);成功 → 已过。
fn classify(msg: &str) -> (bool, &'static str) {
    if msg.contains("成功") {
        (true, "验证码通过 ✅(疑似登录成功)")
    } else if msg.contains("验证码") {
        (false, "验证码未通过 ❌(站点报验证码错)")
    } else if msg.contains("密码")
        || msg.contains("账号")
        || msg.contains("邮箱")
        || msg.contains("用户")
        || msg.contains("不存在")
    {
        (true, "验证码通过 ✅(站点只报账号/密码错 → OCR 对了)")
    } else if msg.is_empty() {
        (false, "无反馈(未判定)")
    } else {
        (true, "验证码通过?(非验证码错误)")
    }
}

/// 刷新验证码(点验证码图所在按钮)。
async fn refresh_captcha(tab: &Tab) {
    if let Ok(img) = tab.ele(CAPTCHA_IMG).await {
        let _ = img.click().await;
    }
    sleep(Duration::from_millis(900)).await;
}
