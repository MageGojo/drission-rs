//! 读取浏览器**实时指纹快照**(后端无关):一次性 dump 当前页面暴露给 JS 的核心指纹信号
//! —— UA / platform / 语言 / 时区 / 屏幕 / `devicePixelRatio` / 硬件并发 / 设备内存 /
//! WebGL `UNMASKED_RENDERER` / canvas 像素哈希。
//!
//! 与 `cdp::fingerprint` / `pool::fingerprint`(**设定 / 伪装**指纹)相对:本模块只**读取**指纹结果,
//! 用于"验证指纹确实换了"、有头/无头 diff、多画像对比等。两后端(CDP [`ChromiumTab`](crate::cdp::ChromiumTab) /
//! Camoufox [`Tab`](crate::browser::Tab))共用同一段探针 JS,经各自的 `run_js` 求值。
//!
//! ```no_run
//! # async fn f(tab: &(impl drission::prelude::FingerprintProbe + Sync)) -> drission::Result<()> {
//! use drission::prelude::*;
//! let fp = tab.fingerprint_snapshot().await?;          // 当前需已在某文档上(canvas/webgl 探针要 DOM)
//! println!("UA={} canvas#={} webgl={}", fp.ua, fp.canvas_hash, fp.webgl_renderer);
//! # Ok(()) }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;

/// 浏览器实时指纹快照(由 [`FingerprintProbe::fingerprint_snapshot`] 采集)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintSnapshot {
    /// `navigator.userAgent`。
    pub ua: String,
    /// `navigator.platform`。
    pub platform: String,
    /// `navigator.languages`(逗号连接)。
    pub languages: String,
    /// `navigator.hardwareConcurrency`(逻辑核数;未知为 0)。
    pub hardware_concurrency: u32,
    /// `navigator.deviceMemory`(GB;浏览器未暴露为 0)。
    pub device_memory: f64,
    /// 屏幕分辨率 `"宽x高"`。
    pub screen: String,
    /// `window.devicePixelRatio`。
    pub device_pixel_ratio: f64,
    /// IANA 时区(`Intl.DateTimeFormat().resolvedOptions().timeZone`)。
    pub timezone: String,
    /// WebGL `UNMASKED_RENDERER_WEBGL`(无 WebGL 为 `none`、出错为 `err`)。
    pub webgl_renderer: String,
    /// canvas 渲染像素哈希(8 位 hex;同机同浏览器稳定、跨指纹应不同)。
    pub canvas_hash: String,
}

impl FingerprintSnapshot {
    /// 从探针返回值解析(探针用 `JSON.stringify` 返回字符串;也兼容后端直接返回对象)。纯函数,便于测试。
    fn from_probe(v: &Value) -> Self {
        let parsed;
        let o: &Value = if let Some(s) = v.as_str() {
            parsed = serde_json::from_str(s).unwrap_or(Value::Null);
            &parsed
        } else {
            v
        };
        FingerprintSnapshot {
            ua: o["ua"].as_str().unwrap_or_default().to_string(),
            platform: o["platform"].as_str().unwrap_or_default().to_string(),
            languages: o["languages"].as_str().unwrap_or_default().to_string(),
            hardware_concurrency: o["hardwareConcurrency"].as_u64().unwrap_or(0) as u32,
            device_memory: o["deviceMemory"].as_f64().unwrap_or(0.0),
            screen: o["screen"].as_str().unwrap_or_default().to_string(),
            device_pixel_ratio: o["devicePixelRatio"].as_f64().unwrap_or(0.0),
            timezone: o["timezone"].as_str().unwrap_or_default().to_string(),
            webgl_renderer: o["webglRenderer"].as_str().unwrap_or_default().to_string(),
            canvas_hash: o["canvasHash"].as_str().unwrap_or_default().to_string(),
        }
    }
}

/// 采集 [`FingerprintSnapshot`] 的探针 JS:建临时 canvas 取渲染像素哈希 + 读 WebGL UNMASKED renderer
/// + navigator/screen/Intl 信号,`JSON.stringify` 一次性返回。
const FINGERPRINT_JS: &str = r#"(function(){
  function canvasHash(){
    try{
      var c=document.createElement('canvas'); c.width=220; c.height=50;
      var x=c.getContext('2d');
      x.textBaseline='top'; x.font='14px Arial'; x.fillStyle='#069'; x.fillText('drission-fp-😀',2,2);
      x.fillStyle='rgba(102,200,0,0.7)'; x.fillText('drission-fp',4,17);
      var u=c.toDataURL(); var h=0;
      for(var i=0;i<u.length;i++){ h=(h*31+u.charCodeAt(i))>>>0; }
      return ('00000000'+h.toString(16)).slice(-8);
    }catch(e){ return 'err'; }
  }
  function webgl(){
    try{
      var c=document.createElement('canvas'); var gl=c.getContext('webgl')||c.getContext('experimental-webgl');
      if(!gl) return 'none';
      var dbg=gl.getExtension('WEBGL_debug_renderer_info');
      return dbg? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
    }catch(e){ return 'err'; }
  }
  return JSON.stringify({
    ua: navigator.userAgent,
    platform: navigator.platform,
    languages: (navigator.languages||[]).join(','),
    hardwareConcurrency: navigator.hardwareConcurrency||0,
    deviceMemory: navigator.deviceMemory||0,
    screen: screen.width+'x'+screen.height,
    devicePixelRatio: window.devicePixelRatio,
    timezone: (Intl.DateTimeFormat().resolvedOptions()||{}).timeZone||'',
    webglRenderer: webgl(),
    canvasHash: canvasHash()
  });
})()"#;

/// **实时指纹读取**能力(CDP / Camoufox 两后端实现;`use drission::prelude::*` 后挂到 `tab` 上)。
#[async_trait::async_trait]
pub trait FingerprintProbe {
    /// 底层求值(各后端委托给固有 `run_js`)。
    async fn fp_eval(&self, js: &str) -> Result<Value>;

    /// 读取当前页面的实时 [`FingerprintSnapshot`]。
    ///
    /// 需当前已在某个文档上(`canvas`/`webgl` 探针要 DOM;`about:blank` 也可,空白页同样能建 canvas)。
    async fn fingerprint_snapshot(&self) -> Result<FingerprintSnapshot> {
        let v = self.fp_eval(FINGERPRINT_JS).await?;
        Ok(FingerprintSnapshot::from_probe(&v))
    }
}

#[cfg(feature = "camoufox")]
#[async_trait::async_trait]
impl FingerprintProbe for crate::browser::Tab {
    async fn fp_eval(&self, js: &str) -> Result<Value> {
        self.run_js(js).await
    }
}

#[cfg(feature = "cdp")]
#[async_trait::async_trait]
impl FingerprintProbe for crate::cdp::ChromiumTab {
    async fn fp_eval(&self, js: &str) -> Result<Value> {
        self.run_js(js).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_json_string_from_probe() {
        // 探针实际返回的是 JSON.stringify 的字符串值。
        let raw = json!(
            r#"{"ua":"Mozilla/5.0 X","platform":"Win32","languages":"en-US,en","hardwareConcurrency":12,"deviceMemory":8,"screen":"2560x1440","devicePixelRatio":1.5,"timezone":"America/New_York","webglRenderer":"ANGLE (NVIDIA)","canvasHash":"a1b2c3d4"}"#
        );
        let fp = FingerprintSnapshot::from_probe(&raw);
        assert_eq!(fp.ua, "Mozilla/5.0 X");
        assert_eq!(fp.platform, "Win32");
        assert_eq!(fp.languages, "en-US,en");
        assert_eq!(fp.hardware_concurrency, 12);
        assert_eq!(fp.device_memory, 8.0);
        assert_eq!(fp.screen, "2560x1440");
        assert_eq!(fp.device_pixel_ratio, 1.5);
        assert_eq!(fp.timezone, "America/New_York");
        assert_eq!(fp.webgl_renderer, "ANGLE (NVIDIA)");
        assert_eq!(fp.canvas_hash, "a1b2c3d4");
    }

    #[test]
    fn parses_object_from_probe_and_defaults_missing() {
        // 兼容后端直接返回对象;缺字段走默认值,不 panic。
        let obj = json!({ "ua": "UA", "hardwareConcurrency": 4 });
        let fp = FingerprintSnapshot::from_probe(&obj);
        assert_eq!(fp.ua, "UA");
        assert_eq!(fp.hardware_concurrency, 4);
        assert_eq!(fp.device_memory, 0.0);
        assert_eq!(fp.canvas_hash, "");
        assert_eq!(fp.webgl_renderer, "");
    }
}
