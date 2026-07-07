use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use base64::Engine;
use drission::prelude::FingerprintProbe;
use serde_json::{Value, json};

use crate::paths;
use crate::protocol::{AxFormat, BackendKind, PacketSummary};

pub enum BackendBrowser {
    #[cfg(feature = "cdp")]
    Cdp(drission::cdp::ChromiumBrowser),
    #[cfg(feature = "camoufox")]
    Camoufox(drission::browser::Browser),
}

#[derive(Clone)]
pub enum BackendTab {
    #[cfg(feature = "cdp")]
    Cdp(drission::cdp::ChromiumTab),
    #[cfg(feature = "camoufox")]
    Camoufox(drission::browser::Tab),
}

impl BackendBrowser {
    pub async fn launch(
        backend: BackendKind,
        headless: bool,
        user_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        match backend {
            BackendKind::Cdp => launch_cdp(headless, user_data_dir).await,
            BackendKind::Camoufox => launch_camoufox(headless, user_data_dir).await,
        }
    }

    pub async fn new_tab(&self, url: Option<&str>) -> Result<BackendTab> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => Ok(BackendTab::Cdp(browser.new_tab(url).await?)),
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(browser) => {
                Ok(BackendTab::Camoufox(browser.new_tab(url).await?))
            }
        }
    }

    pub async fn quit(&self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => browser.quit().await?,
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(browser) => browser.quit().await?,
        }
        Ok(())
    }
}

#[cfg(feature = "cdp")]
async fn launch_cdp(headless: bool, user_data_dir: Option<PathBuf>) -> Result<BackendBrowser> {
    let mut opts = drission::cdp::ChromiumOptions::new().headless(headless);
    if let Some(dir) = user_data_dir {
        opts = opts.user_data_dir(dir);
    }
    Ok(BackendBrowser::Cdp(
        drission::cdp::ChromiumBrowser::launch(opts).await?,
    ))
}

#[cfg(not(feature = "cdp"))]
async fn launch_cdp(_headless: bool, _user_data_dir: Option<PathBuf>) -> Result<BackendBrowser> {
    anyhow::bail!("backend cdp is not compiled; rebuild drs with feature `cdp`")
}

#[cfg(feature = "camoufox")]
async fn launch_camoufox(headless: bool, user_data_dir: Option<PathBuf>) -> Result<BackendBrowser> {
    let mut opts = drission::launcher::BrowserOptions::new().headless(headless);
    if let Some(dir) = user_data_dir {
        opts = opts.user_data_dir(dir);
    }
    Ok(BackendBrowser::Camoufox(
        drission::browser::Browser::launch(opts).await?,
    ))
}

#[cfg(not(feature = "camoufox"))]
async fn launch_camoufox(
    _headless: bool,
    _user_data_dir: Option<PathBuf>,
) -> Result<BackendBrowser> {
    anyhow::bail!("backend camoufox is not compiled; rebuild drs with feature `camoufox`")
}

impl BackendTab {
    pub async fn close(&self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.close().await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.close().await?,
        }
        Ok(())
    }

    pub async fn title(&self) -> Result<String> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.title().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.title().await?),
        }
    }

    pub async fn url(&self) -> Result<String> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.url().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.url().await?),
        }
    }

    pub async fn html(&self) -> Result<String> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.html().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.html().await?),
        }
    }

    pub async fn text(&self, selector: Option<&str>) -> Result<String> {
        match (self, selector) {
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), Some(selector)) => Ok(tab.ele(selector).await?.text().await?),
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), Some(selector)) => {
                Ok(tab.ele(selector).await?.text().await?)
            }
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), None) => Ok(tab
                .run_js("document.body ? document.body.innerText : ''")
                .await?
                .as_str()
                .unwrap_or_default()
                .to_string()),
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), None) => Ok(tab
                .run_js("document.body ? document.body.innerText : ''")
                .await?
                .as_str()
                .unwrap_or_default()
                .to_string()),
        }
    }

    pub async fn eval(&self, js: &str) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.run_js(js).await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.run_js(js).await?),
        }
    }

    pub async fn ax(&self, format: AxFormat) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let tree = tab.ax_snapshot().await?;
                Ok(match format {
                    AxFormat::Outline => json!({ "outline": tree.to_outline() }),
                    AxFormat::Json => serde_json::to_value(tree)?,
                })
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let tree = tab.ax_snapshot().await?;
                Ok(match format {
                    AxFormat::Outline => json!({ "outline": tree.to_outline() }),
                    AxFormat::Json => serde_json::to_value(tree)?,
                })
            }
        }
    }

    pub async fn screenshot(
        &self,
        out: Option<PathBuf>,
        full: bool,
        inline: bool,
    ) -> Result<Value> {
        let bytes = match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                if full {
                    tab.screenshot_full_bytes().await?
                } else {
                    tab.screenshot_bytes().await?
                }
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.screenshot_bytes(full).await?,
        };

        let path = match out {
            Some(path) => path,
            None => default_screenshot_path().await?,
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &bytes).await?;

        let mut data = json!({
            "path": path,
            "bytes": bytes.len(),
            "mimeType": "image/png",
            "full": full,
        });
        if inline {
            data["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
        Ok(data)
    }

    pub async fn click(&self, selector: &str) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.click(selector).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.click(selector).await?,
        }
        Ok(())
    }

    pub async fn type_text(&self, selector: &str, text: &str) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.input(selector, text).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.input(selector, text).await?,
        }
        Ok(())
    }

    pub async fn press(&self, key: &str, selector: Option<&str>) -> Result<()> {
        match (self, selector) {
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), Some(selector)) => {
                tab.ele(selector)
                    .await?
                    .input_keys(&[drission::keys::KeyInput::key(key)])
                    .await?
            }
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), Some(selector)) => {
                tab.ele(selector)
                    .await?
                    .input_keys(&[drission::keys::KeyInput::key(key)])
                    .await?
            }
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), None) => tab.press_key(key).await?,
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), None) => tab.press_key(key).await?,
        }
        Ok(())
    }

    pub async fn wait(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.wait().ele_displayed(selector, timeout).await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.wait().ele_displayed(selector, timeout).await?),
        }
    }

    pub async fn listen_start(&self, keywords: &[String], xhr_only: bool) -> Result<Value> {
        let refs: Vec<&str> = keywords.iter().map(String::as_str).collect();
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                if xhr_only {
                    tab.listen().start_xhr(&refs).await?;
                } else {
                    tab.listen().start(&refs).await?;
                }
                Ok(json!({ "listening": true, "xhrOnly": xhr_only }))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                tab.listen().start(&refs).await?;
                Ok(json!({
                    "listening": true,
                    "xhrOnly": false,
                    "warning": if xhr_only {
                        Some("camoufox listener ignores xhr_only; URL filters still apply")
                    } else {
                        None
                    }
                }))
            }
        }
    }

    pub async fn listen_wait(&self, count: usize, timeout: Option<Duration>) -> Result<Value> {
        let count = count.max(1);
        let packets: Vec<PacketSummary> = match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab
                .listen()
                .wait_count(count, timeout)
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab
                .listen()
                .wait_count(count, timeout)
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
        };
        Ok(json!({ "count": packets.len(), "packets": packets }))
    }

    pub async fn listen_stop(&self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.listen().stop().await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.listen().stop().await?,
        }
        Ok(())
    }

    pub async fn pass_cf(&self, timeout: Duration) -> Result<bool> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.pass_cloudflare(timeout).await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.pass_cloudflare(timeout).await?),
        }
    }

    pub async fn fingerprint_snapshot(&self) -> Result<drission::fingerprint::FingerprintSnapshot> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.fingerprint_snapshot().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.fingerprint_snapshot().await?),
        }
    }

    pub async fn identity_report(&self) -> Result<drission::fingerprint::IdentityReport> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.identity_report().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.identity_report().await?),
        }
    }
}

async fn default_screenshot_path() -> Result<PathBuf> {
    let dir = paths::screenshots_dir()?;
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir.join(format!("shot-{}.png", now_ms())))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn ensure_backend_available(kind: BackendKind) -> Result<()> {
    match kind {
        BackendKind::Cdp => {
            if cfg!(feature = "cdp") {
                Ok(())
            } else {
                Err(anyhow!(
                    "backend cdp is not compiled; rebuild drs with feature `cdp`"
                ))
            }
        }
        BackendKind::Camoufox => {
            if cfg!(feature = "camoufox") {
                Ok(())
            } else {
                Err(anyhow!(
                    "backend camoufox is not compiled; rebuild drs with feature `camoufox`"
                ))
            }
        }
    }
}
