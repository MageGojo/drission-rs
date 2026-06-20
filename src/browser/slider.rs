//! 通用**滑块验证码**求解(drission-rs 库能力,不限极验)。
//!
//! 滑块类验证码的共性:一张**带缺口的底图** + 一块**拼图**,把拼图水平拖到缺口即过。难点是
//! ①算准"拼图要移多远"②拟人地把把手拖到位。本模块把这两件事做成**与厂商无关**的通用能力:
//!
//! - [`Tab::slider_gap`]:纯视觉,按 [`SliderConfig`] 读图、自动选法算出位移 → [`SliderGap`]。
//! - [`Tab::solve_slider`]:一把梭(弹出→匹配→闭环拟人拖动→判定→换图重试)→ [`SliderResult`]。
//! - 预设:[`SliderConfig::geetest_v4`] + 便捷方法 [`Tab::solve_geetest_slide`] / [`Tab::geetest_slide_gap`]。
//!
//! ## 缺口算法(按可得素材自动选)
//! - **双图法** [`GapMethod::TwoImage`](有 `full_bg` + `piece`,最准,极验即此):拼图真实颜色对
//!   完整底图落点最小色差(`by_color`)+ 拼图形状 alpha 对 `bg`-vs-`full` diff 幅度图最大重叠
//!   (`by_shape`);两法互校,近则取颜色法、分歧大取形状法([`choose_displacement`])。
//! - **拼图模板法** [`GapMethod::PieceTemplate`](只有 `piece`、无 `full`,多数非极验滑块):把拼图
//!   **轮廓**在底图**边缘幅度图**上滑动、最大化重叠(对齐缺口外框)。
//! - **缺口探测** [`GapMethod::Notch`](只有 `bg`):取底图纵向边缘最强的列当缺口(best-effort)。
//!
//! 关键:不靠任何单侧"边缘检测"(缺口低对比沿会被漏、拼图辉光会外扩),而是**整块形状/颜色对齐**,
//! 误差互相抵消——这正是此前极验"过不了"的根因(旧法缺口边缘−拼图边缘系统性偏移)被修正之处。
//!
//! 图源支持 `<canvas>` 与 `<img>`(见 [`ImageSource`]);拼图会按其相对底图的真实位置画到底图坐标系,
//! 故 D 即"拼图要移动的距离"。拖动用 minimum-jerk 拟人轨迹;给了 `piece` 即**闭环纠偏**
//! (标定把手:拼图位移比 + 读真实位置校正),否则按 `track_ratio`(默认 1.0)开环。
//!
//! > 反检测:建议**导航前**调用 [`Tab::apply_pointer_stealth`](crate::browser::Tab::apply_pointer_stealth)。
//!
//! 端到端示例:`examples/geetest_slide`(极验);`examples/slider_local`(离线合成 img 滑块自验证)。
//! 缺口诊断 + 叠加验证图:`examples/geetest_diag`。

use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::sleep;

use super::Tab;
use crate::util::base64_encode;
use crate::{Error, Result};

/// 图源:CSS 选择器指向的 `<canvas>` / `<img>` / 截图块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// 一个 `<canvas>` 元素(直接 `getImageData`)。
    Canvas(String),
    /// 一个 `<img>` 元素(按自然尺寸离屏绘制后取像素;跨域无 CORS 会 taint 不可读)。
    Img(String),
    /// 一个**跨域 taint 锁死**的 `<img>`(`getImageData` 读不到像素,如顶象拼图)。库会**浏览器级
    /// 截图**(taint-proof)该元素、注入隐藏 img 后做内容匹配([`GapMethod::ContentNcc`])。
    Shot(String),
}

impl ImageSource {
    /// `<canvas>` 图源。
    pub fn canvas(sel: impl Into<String>) -> Self {
        ImageSource::Canvas(sel.into())
    }
    /// `<img>` 图源。
    pub fn img(sel: impl Into<String>) -> Self {
        ImageSource::Img(sel.into())
    }
    /// 截图图源(跨域 taint 锁死的 `<img>`,如顶象拼图;库截图后内容匹配)。
    pub fn shot(sel: impl Into<String>) -> Self {
        ImageSource::Shot(sel.into())
    }
    fn kind(&self) -> &'static str {
        match self {
            ImageSource::Canvas(_) => "canvas",
            ImageSource::Img(_) => "img",
            ImageSource::Shot(_) => "shot",
        }
    }
    fn sel(&self) -> &str {
        match self {
            ImageSource::Canvas(s) | ImageSource::Img(s) | ImageSource::Shot(s) => s,
        }
    }
    fn to_cfg(&self) -> Value {
        json!({ "k": self.kind(), "s": self.sel() })
    }
}

/// 选用的缺口算法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapMethod {
    /// 双图法(`bg` + `full_bg` + `piece`)。
    TwoImage,
    /// 拼图模板法(`bg` + `piece`,无 `full_bg`)。
    PieceTemplate,
    /// 缺口探测(仅 `bg`)。
    Notch,
    /// 内容相关法(`bg` 可读 canvas + [`ImageSource::Shot`] 截图拼图):绿环掩膜 + 彩色内容 NCC +
    /// 暗度门控 + 描边对齐微调,纹理太弱(夜景纯黑缺口)时退暗度+描边兜底。用于顶象等
    /// **繁杂实拍图 + 同形诱饵 + 重度压暗 + 拼图跨域 taint** 的场景。
    ContentNcc,
}

impl GapMethod {
    fn parse(s: &str) -> Self {
        match s {
            "two_image" => GapMethod::TwoImage,
            "piece_template" => GapMethod::PieceTemplate,
            "content_ncc" => GapMethod::ContentNcc,
            _ => GapMethod::Notch,
        }
    }
}

/// 怎么判定"验证通过"。
#[derive(Debug, Clone)]
pub enum SuccessCheck {
    /// 某选择器对应元素可见即通过(如极验 `.geetest_success_radar_tip_content`)。
    Visible(String),
    /// 自定义 JS 表达式(应返回 `true`/`false`)。
    Js(String),
    /// 不做服务端判定:拖动完成即视为成功(`align_error` 仍会给出对齐误差)。
    None,
}

/// 一次缺口计算结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SliderGap {
    /// 拼图需要水平移动的距离(CSS 像素)——拖动闭环的目标。
    pub displace: f64,
    /// 实际选用的算法。
    pub method: GapMethod,
    /// 形状法位移(CSS px;双图法有效,其余等于 `displace`)。
    pub by_shape: f64,
    /// 颜色法位移(CSS px;双图法有效,其余等于 `displace`)。
    pub by_color: f64,
    /// 粗略置信度 0~1(越高越可信)。
    pub confidence: f64,
}

/// 通用滑块求解配置。用 [`SliderConfig::new`] 起步链式设置,或用预设 [`SliderConfig::geetest_v4`]。
#[derive(Debug, Clone)]
pub struct SliderConfig {
    /// 带缺口的底图(必填)。
    pub bg: ImageSource,
    /// 完整底图(可选;有则用最准的双图法)。
    pub full_bg: Option<ImageSource>,
    /// 拼图块(可选;有则可做拼图模板法 + 拖动闭环纠偏)。
    pub piece: Option<ImageSource>,
    /// 要拖动的把手元素选择器(必填)。
    pub handle: String,
    /// 触发弹出验证的按钮选择器(可选;把手不可见时点它)。
    pub open: Option<String>,
    /// 换一张验证图的按钮选择器(可选;非通过时点它重试,支持逗号多选取首个命中)。
    pub refresh: Option<String>,
    /// 通过判定方式。默认 [`SuccessCheck::None`]。
    pub success: SuccessCheck,
    /// 把手:拼图 位移比(把手移 1,拼图移 `track_ratio`)。`None`=有 `piece` 时闭环标定、
    /// 无 `piece` 时按 1.0。给定值则开环按该比。
    pub track_ratio: Option<f64>,
    /// 最多尝试次数(非通过自动换图重试)。默认 6。
    pub max_attempts: u32,
}

impl SliderConfig {
    /// 起步:底图 + 把手(其余可选项链式设置)。
    pub fn new(bg: ImageSource, handle: impl Into<String>) -> Self {
        Self {
            bg,
            full_bg: None,
            piece: None,
            handle: handle.into(),
            open: None,
            refresh: None,
            success: SuccessCheck::None,
            track_ratio: None,
            max_attempts: 6,
        }
    }

    /// 极验 v4 滑块预设(canvas 三图 + 标准类名,成功判定兼容 float / custom 等模式)。
    pub fn geetest_v4() -> Self {
        Self {
            bg: ImageSource::canvas(".geetest_canvas_bg"),
            full_bg: Some(ImageSource::canvas(".geetest_canvas_fullbg")),
            piece: Some(ImageSource::canvas(".geetest_canvas_slice")),
            handle: ".geetest_slider_button".into(),
            open: Some(".geetest_radar_btn".into()),
            refresh: Some(".geetest_refresh_1,.geetest_refresh,.geetest_reset".into()),
            success: SuccessCheck::Js(GEETEST_SUCCESS_JS.into()),
            track_ratio: None,
            max_attempts: 6,
        }
    }

    /// 顶象(Dingxiang)滑块预设(实例后缀 `i`):底图可读 canvas + 截图拼图([`ImageSource::Shot`],
    /// 拼图跨域 taint)→ 走 [`GapMethod::ContentNcc`];换图键 = 该实例的刷新键;成功判定看成功条可见。
    /// 弹出式再链式 `.open("#btn-popup")`。页面多个验证码时 `i` 用动态识别(见 `examples/dx_slide`)。
    pub fn dingxiang(i: u32) -> Self {
        Self {
            bg: ImageSource::canvas(format!("#dx_captcha_basic_bg_{i} canvas")),
            full_bg: None,
            piece: Some(ImageSource::shot(format!("#dx_captcha_basic_sub-slider_{i} img"))),
            handle: format!("#dx_captcha_basic_slider_{i}"),
            open: None,
            refresh: Some(format!("#dx_captcha_basic_btn-refresh_{i}")),
            success: SuccessCheck::Js(DINGXIANG_SUCCESS_JS.into()),
            track_ratio: None,
            max_attempts: 6,
        }
    }

    /// 设置完整底图(开启双图法)。
    pub fn full_bg(mut self, src: ImageSource) -> Self {
        self.full_bg = Some(src);
        self
    }
    /// 设置拼图块。
    pub fn piece(mut self, src: ImageSource) -> Self {
        self.piece = Some(src);
        self
    }
    /// 设置弹出按钮。
    pub fn open(mut self, sel: impl Into<String>) -> Self {
        self.open = Some(sel.into());
        self
    }
    /// 设置换图按钮。
    pub fn refresh(mut self, sel: impl Into<String>) -> Self {
        self.refresh = Some(sel.into());
        self
    }
    /// 设置通过判定方式。
    pub fn success(mut self, s: SuccessCheck) -> Self {
        self.success = s;
        self
    }
    /// 设置把手:拼图位移比(开环)。
    pub fn track_ratio(mut self, r: f64) -> Self {
        self.track_ratio = Some(r);
        self
    }
    /// 设置最多尝试次数。
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n;
        self
    }
}

/// 求解结果。
#[derive(Debug, Clone)]
pub struct SliderResult {
    /// 是否通过([`SuccessCheck::None`] 时表示是否确实完成拖动)。
    pub passed: bool,
    /// 实际尝试次数。
    pub attempts: u32,
    /// 最佳一次的对齐误差(CSS px;无 `piece`/无法闭环时为 -1)。
    pub align_error: f64,
}

/// 极验成功判定(兼容 float/custom 等模式):成功提示元素可见,或雷达提示文本含"成功/通过"。
/// 故意不看 `.geetest_success_box/show`(它们是常驻容器,非通过也可见)。是 IIFE 表达式,可嵌进 `!!(…)`。
const GEETEST_SUCCESS_JS: &str = r#"(function(){
  function vis(s){var e=document.querySelector(s); return !!(e && e.getBoundingClientRect().width>0);}
  function txt(s){var e=document.querySelector(s); return e?(e.textContent||''):'';}
  if(vis('.geetest_success_radar_tip_content')) return true;
  if(vis('.geetest_success_animate')) return true;
  return /成功|通过/.test(txt('.geetest_radar_tip_content')+txt('.geetest_success_radar_tip_content'));
})()"#;

/// 顶象成功判定:成功条/成功提示元素**可见**才算过(顶象成功元素常驻 DOM、默认隐藏,只看文本会假阳)。
const DINGXIANG_SUCCESS_JS: &str = r#"(function(){var e=document.querySelectorAll('.dx_captcha_basic_bar-success,.dx_captcha_basic_success'); for(var i=0;i<e.length;i++){var r=e[i].getBoundingClientRect(); if(r.width>0&&r.height>0)return true;} return false;})()"#;

/// 缺口匹配 JS 模板:`__CFG__` 处注入 `{bg, full, piece}` 图源描述。像素重活(读图 + argmin/argmax)
/// 都在页面内做,只回传 `{da, db, method, scale, confidence}` 几个标量。
const MATCH_TEMPLATE: &str = r#"(function(){
  var CFG=__CFG__;
  function q(s){return document.querySelector(s);}
  function bgData(d){ var e=q(d.s); if(!e) return null;
    if(d.k==='canvas'){ try{ return {W:e.width,H:e.height, data:e.getContext('2d').getImageData(0,0,e.width,e.height).data, rectW:e.getBoundingClientRect().width}; }catch(x){return {err:String(x)};} }
    var W=e.naturalWidth||e.width, H=e.naturalHeight||e.height; if(!W||!H) return null;
    var c=document.createElement('canvas'); c.width=W;c.height=H;
    try{ var cx=c.getContext('2d'); cx.drawImage(e,0,0,W,H); return {W:W,H:H,data:cx.getImageData(0,0,W,H).data, rectW:e.getBoundingClientRect().width}; }catch(x){return {err:String(x)};}
  }
  function sameDim(d,W,H){ var e=q(d.s); if(!e) return null; var c=document.createElement('canvas'); c.width=W;c.height=H;
    try{ var cx=c.getContext('2d'); cx.drawImage(e,0,0,W,H); return cx.getImageData(0,0,W,H).data; }catch(x){return null;} }
  function pieceToBg(d,W,H,bgRect){ var e=q(d.s); if(!e) return null; var c=document.createElement('canvas'); c.width=W;c.height=H; var cx=c.getContext('2d');
    var pr=e.getBoundingClientRect(); var sx=W/bgRect.width, sy=H/bgRect.height;
    var dx=(pr.left-bgRect.left)*sx, dy=(pr.top-bgRect.top)*sy;
    try{ cx.drawImage(e, dx, dy, Math.max(1,pr.width*sx), Math.max(1,pr.height*sy)); return cx.getImageData(0,0,W,H).data; }catch(x){return null;} }

  var bgE=q(CFG.bg.s); if(!bgE) return JSON.stringify({ok:false,reason:'no bg element'});
  var bgRect=bgE.getBoundingClientRect();
  var BG=bgData(CFG.bg); if(!BG) return JSON.stringify({ok:false,reason:'no bg'}); if(BG.err) return JSON.stringify({ok:false,reason:'bg '+BG.err});
  var W=BG.W,H=BG.H,b=BG.data, scale=W?BG.rectW/W:1;
    var full = CFG.full ? sameDim(CFG.full,W,H) : null;
    // 拼图块几何(即使像素跨域 taint 不可读,getBoundingClientRect 仍可读):宽度 + 当前左缘(底图像素)。
    var pieceEl = CFG.piece ? q(CFG.piece.s) : null;
    var pw=0, pieceCurX=0;
    if(pieceEl){ var pr0=pieceEl.getBoundingClientRect(); var sxg=W/bgRect.width;
      pw=Math.round(pr0.width*sxg); pieceCurX=Math.round((pr0.left-bgRect.left)*sxg); }
    var piece = CFG.piece ? pieceToBg(CFG.piece,W,H,bgRect) : null;

    var pts=null, px1=0;
  if(piece){ pts=[]; for(var y=0;y<H;y++)for(var x=0;x<W;x++){var o=(y*W+x)*4;var a=piece[o+3]; if(a>30){pts.push([x,y,a,piece[o],piece[o+1],piece[o+2]]); if(x>px1)px1=x;}} if(pts.length<15)pts=null; }

  function out(method,da,db,conf){ return JSON.stringify({ok:true, method:method, da:da, db:db, scale:scale, confidence:Math.max(0,Math.min(1,conf))}); }

  // 双图法。
  if(full && pts){
    var dm=new Float64Array(W*H);
    for(var i2=0;i2<W*H;i2++){var o2=i2*4; dm[i2]=Math.abs(b[o2]-full[o2])+Math.abs(b[o2+1]-full[o2+1])+Math.abs(b[o2+2]-full[o2+2]);}
    var maxD=W-px1-1; if(maxD<1) return JSON.stringify({ok:false,reason:'no room'});
    var da=-1,bestA=-1; for(var D=0;D<=maxD;D++){var sc=0; for(var k=0;k<pts.length;k++){var p=pts[k]; sc+=dm[p[1]*W+(p[0]+D)]*p[2];} if(sc>bestA){bestA=sc;da=D;}}
    var db=-1,bestB=1e18,secB=1e18; for(var D2=0;D2<=maxD;D2++){var er=0; for(var k2=0;k2<pts.length;k2++){var qq=pts[k2];var j=(qq[1]*W+(qq[0]+D2))*4; er+=Math.abs(qq[3]-full[j])+Math.abs(qq[4]-full[j+1])+Math.abs(qq[5]-full[j+2]);} if(er<bestB){secB=bestB;bestB=er;db=D2;} else if(er<secB){secB=er;}}
    var conf=secB>0?1-bestB/secB:1;
    return out('two_image',da,db,conf);
  }

  // 拼图模板法(轮廓对底图边缘)。
  if(pts){
    function g(d,o){return (d[o]+d[o+1]+d[o+2])/3;}
    var be=new Float64Array(W*H);
    for(var y3=1;y3<H-1;y3++)for(var x3=1;x3<W-1;x3++){var o3=(y3*W+x3)*4;
      be[y3*W+x3]=Math.abs(g(b,o3+4)-g(b,o3-4))+Math.abs(g(b,((y3+1)*W+x3)*4)-g(b,((y3-1)*W+x3)*4));}
    var bnd=[], pmaxx=0;
    for(var k3=0;k3<pts.length;k3++){var p3=pts[k3];var xx=p3[0],yy=p3[1];
      var lf=xx>0?piece[(yy*W+xx-1)*4+3]:0, rt=xx<W-1?piece[(yy*W+xx+1)*4+3]:0;
      var up=yy>0?piece[((yy-1)*W+xx)*4+3]:0, dn=yy<H-1?piece[((yy+1)*W+xx)*4+3]:0;
      if(lf<=30||rt<=30||up<=30||dn<=30){bnd.push([xx,yy]); if(xx>pmaxx)pmaxx=xx;}}
    if(bnd.length<8) return JSON.stringify({ok:false,reason:'no piece edge'});
    var maxD3=W-pmaxx-1; var bd=-1,bb=-1,sec=-1;
    for(var D3=0;D3<=maxD3;D3++){var s3=0; for(var m=0;m<bnd.length;m++){var bp=bnd[m]; s3+=be[bp[1]*W+(bp[0]+D3)];} if(s3>bb){sec=bb;bb=s3;bd=D3;} else if(s3>sec){sec=s3;}}
    var conf3=bb>0?1-(sec>0?sec/bb:0):0;
    return out('piece_template',bd,bd,conf3);
  }

  // 缺口探测(仅底图):找"缺口方框"——左右两条竖边相距约拼图宽 `pw`(拼图像素 taint 时仍可用其几何宽度)。
  // 比"取最强单列"稳:缺口的左右框边成对,纹理杂边一般不成对。跳过拼图自身起始区域的强边。
  function g2(d,o){return (d[o]+d[o+1]+d[o+2])/3;}
  var col=new Float64Array(W);
  for(var x4=1;x4<W-1;x4++){var s4=0; for(var y4=1;y4<H-1;y4++){var o4=(y4*W+x4)*4; s4+=Math.abs(g2(b,o4+4)-g2(b,o4-4));} col[x4]=s4;}
  var skipL = pieceCurX>0 ? (pieceCurX+(pw||Math.round(W*0.14))+6) : Math.floor(W*0.12);
  var widths = pw>5 ? [pw, pw+3, pw-3] : [Math.round(W*0.12),Math.round(W*0.15),Math.round(W*0.18),Math.round(W*0.22)];
  var bL=-1,bS=-1,b2=-1;
  for(var wi=0;wi<widths.length;wi++){ var w=widths[wi]; if(w<8)continue;
    for(var L=skipL; L+w<W-1; L++){ var sc=col[L]+col[L+w];
      if(sc>bS){b2=bS;bS=sc;bL=L;} else if(sc>b2){b2=sc;} } }
  if(bL<0){ // 退化:取最强单列。
    var nx=-1,nb=-1,start=Math.floor(W*0.12); for(var x5=start;x5<W;x5++){ if(col[x5]>nb){nb=col[x5];nx=x5;} }
    if(nx<0) return JSON.stringify({ok:false,reason:'no notch'});
    return out('notch', pieceCurX>0?(nx-pieceCurX):nx, pieceCurX>0?(nx-pieceCurX):nx, 0.25);
  }
  var D = pieceCurX>0 ? (bL - pieceCurX) : bL;     // 位移 = 缺口左缘 - 拼图当前左缘
  var conf = bS>0 ? Math.max(0.2, 1-(b2>0?b2/bS:0)) : 0.3;
  return out('notch', D, D, conf);
})()"#;

impl Tab {
    /// **纯视觉**:按 [`SliderConfig`] 读图、自动选缺口算法,算出拼图需要水平移动的距离([`SliderGap`])。
    /// 要求验证图已显示。读图失败 / 无有效结果返回 `Err`。
    pub async fn slider_gap(&self, cfg: &SliderConfig) -> Result<SliderGap> {
        // 截图拼图(跨域 taint)→ 内容相关法(顶象等)。
        if matches!(cfg.piece, Some(ImageSource::Shot(_))) {
            return self.slider_gap_content_ncc(cfg).await;
        }
        let mut c = json!({ "bg": cfg.bg.to_cfg() });
        if let Some(f) = &cfg.full_bg {
            c["full"] = f.to_cfg();
        }
        if let Some(p) = &cfg.piece {
            c["piece"] = p.to_cfg();
        }
        let js = MATCH_TEMPLATE.replace("__CFG__", &c.to_string());
        let r = self.run_js(&js).await?;
        let v: Value = serde_json::from_str(r.as_str().unwrap_or("null")).unwrap_or_default();
        if v["ok"].as_bool() != Some(true) {
            let reason = v["reason"].as_str().unwrap_or("缺口不可读");
            return Err(Error::msg(format!("滑块缺口检测失败: {reason}")));
        }
        let da = v["da"].as_f64().unwrap_or(-1.0);
        let db = v["db"].as_f64().unwrap_or(-1.0);
        if da < 0.0 || db < 0.0 {
            return Err(Error::msg("滑块缺口检测失败: 无有效位移"));
        }
        let scale = v["scale"].as_f64().unwrap_or(1.0);
        let method = GapMethod::parse(v["method"].as_str().unwrap_or("notch"));
        let chosen = if method == GapMethod::TwoImage { choose_displacement(da, db) } else { da };
        // 双图法的置信度看两法一致程度(da==db→1.0,这是最可信的情形);其余法用 JS 给的峰值比。
        let confidence = if method == GapMethod::TwoImage {
            (1.0 - (da - db).abs().min(20.0) / 20.0).clamp(0.0, 1.0)
        } else {
            v["confidence"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0)
        };
        Ok(SliderGap {
            displace: chosen * scale,
            method,
            by_shape: da * scale,
            by_color: db * scale,
            confidence,
        })
    }

    /// 内容相关法([`GapMethod::ContentNcc`]):拼图跨域 taint 不可读,故**浏览器级截图**(taint-proof)
    /// 该元素 → 注入隐藏 img → 绿环掩膜 + 彩色内容 NCC + 暗度门控 + 描边对齐 / 暗度兜底。要求 `bg` 为
    /// 可读 canvas、`piece` 为 [`ImageSource::Shot`]。
    async fn slider_gap_content_ncc(&self, cfg: &SliderConfig) -> Result<SliderGap> {
        let piece_sel = match &cfg.piece {
            Some(ImageSource::Shot(s)) => s.clone(),
            _ => return Err(Error::msg("内容相关法需要 Shot 截图拼图源")),
        };
        let bg_sel = cfg.bg.sel().to_string();
        // 浏览器级截图拼图(绕过 canvas taint),注入到离 widget 远的 body 末尾隐藏 img。
        let bytes = self.ele(&format!("css:{piece_sel}")).await?.screenshot_bytes().await?;
        let data_url = format!("data:image/png;base64,{}", base64_encode(&bytes));
        self.run_js(&format!(
            "(function(){{var im=document.getElementById('__drission_shot'); if(!im){{im=document.createElement('img'); im.id='__drission_shot'; im.style.display='none'; document.body.appendChild(im);}} im.src='{data_url}';}})()"
        ))
        .await?;
        sleep(Duration::from_millis(450)).await;
        let r = self.run_js(&content_ncc_js(&bg_sel, &piece_sel)).await;
        // 匹配后立即移除注入的 img(零残留)。
        let _ = self
            .run_js("(function(){var e=document.getElementById('__drission_shot'); if(e)e.remove();})()")
            .await;
        let r = r?;
        let v: Value = serde_json::from_str(r.as_str().unwrap_or("null")).unwrap_or_default();
        if v["ok"].as_bool() != Some(true) {
            let reason = v["reason"].as_str().unwrap_or("缺口不可读");
            return Err(Error::msg(format!("顶象缺口检测失败: {reason}")));
        }
        let displace = v["displace"].as_f64().unwrap_or(-1.0);
        if displace < 0.0 {
            return Err(Error::msg("顶象缺口检测失败: 无有效位移"));
        }
        let confidence = v["conf"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
        Ok(SliderGap {
            displace,
            method: GapMethod::ContentNcc,
            by_shape: displace,
            by_color: displace,
            confidence,
        })
    }

    /// **一把梭**:弹出→匹配→闭环拟人拖动→判定→非通过换图重试。返回 [`SliderResult`]。
    pub async fn solve_slider(&self, cfg: &SliderConfig) -> Result<SliderResult> {
        let mut best_err = f64::INFINITY;
        let mut attempts = 0u32;
        for _ in 0..cfg.max_attempts.max(1) {
            attempts += 1;
            ensure_visible(self, cfg).await?;
            let gap = match self.slider_gap(cfg).await {
                Ok(g) => g,
                Err(_) => {
                    nudge_refresh(self, cfg).await;
                    continue;
                }
            };
            let err = slide_drag(self, cfg, gap.displace).await?;
            if err.is_finite() {
                best_err = best_err.min(err.abs());
            }
            match &cfg.success {
                SuccessCheck::None => {
                    return Ok(SliderResult {
                        passed: true,
                        attempts,
                        align_error: if best_err.is_finite() { best_err } else { -1.0 },
                    });
                }
                check => {
                    sleep(Duration::from_secs(2)).await;
                    if check_success(self, check).await {
                        return Ok(SliderResult {
                            passed: true,
                            attempts,
                            align_error: if best_err.is_finite() { best_err } else { -1.0 },
                        });
                    }
                    nudge_refresh(self, cfg).await;
                }
            }
        }
        Ok(SliderResult {
            passed: false,
            attempts,
            align_error: if best_err.is_finite() { best_err } else { -1.0 },
        })
    }

    /// 极验 v4 滑块缺口(预设 [`SliderConfig::geetest_v4`] 的 [`slider_gap`](Self::slider_gap))。
    pub async fn geetest_slide_gap(&self) -> Result<SliderGap> {
        self.slider_gap(&SliderConfig::geetest_v4()).await
    }

    /// 一把梭求解极验 v4 滑块(预设 [`SliderConfig::geetest_v4`] 的 [`solve_slider`](Self::solve_slider))。
    /// 要调尝试次数等用 `solve_slider(&SliderConfig::geetest_v4().max_attempts(8))`。
    pub async fn solve_geetest_slide(&self) -> Result<SliderResult> {
        self.solve_slider(&SliderConfig::geetest_v4()).await
    }

    /// 顶象滑块缺口(预设 [`SliderConfig::dingxiang`] 的 [`slider_gap`](Self::slider_gap),`index`=实例后缀)。
    pub async fn dingxiang_slide_gap(&self, index: u32) -> Result<SliderGap> {
        self.slider_gap(&SliderConfig::dingxiang(index)).await
    }

    /// 一把梭求解顶象滑块(预设 [`SliderConfig::dingxiang`])。弹出式传 `open` 触发按钮(如 `#btn-popup`)。
    /// 注:顶象 demo 对自动化拖动有轨迹/IP 行为风控会弹回(与缺口算法无关);本方法只保证**缺口找得准 +
    /// 拖到位**。要自定义尝试次数等改用 `solve_slider(&SliderConfig::dingxiang(i)...)`。
    pub async fn solve_dingxiang_slide(&self, index: u32, open: Option<&str>) -> Result<SliderResult> {
        let mut cfg = SliderConfig::dingxiang(index);
        if let Some(o) = open {
            cfg = cfg.open(o);
        }
        self.solve_slider(&cfg).await
    }
}

/// 在形状法 `da` 与颜色法 `db`(画布像素)间选位移:相差 ≤6 源像素取颜色法(最贴"放回原位"),
/// 否则取形状法(不受拼图描边/辉光干扰)。
fn choose_displacement(da: f64, db: f64) -> f64 {
    if (da - db).abs() > 6.0 { da } else { db }
}

/// 内容相关法 JS:全程在底图 canvas backing 像素空间。读 `bg` 可读 canvas + 注入的截图拼图
/// `#__drission_shot`(像素)+ 原拼图元素 `piece`(几何 home 位置)。四步:①绿环掩膜(检测绿色发光
/// 描边 → BFS 填洞 → 内部纹理核;绿环不足回退差分掩膜)②彩色 3 通道内容 NCC(对压暗的线性亮度免疫)
/// ③暗度门控(缺口不比拼图亮)④NCC 峰≥0.45 描边局部对齐微调 / 否则暗度+描边兜底。返回 CSS 位移 + 置信。
fn content_ncc_js(bg_sel: &str, piece_sel: &str) -> String {
    format!(
        r#"(function(){{
  var A=document.getElementById('__drission_shot');
  var bg=document.querySelector({bg});
  var pe=document.querySelector({piece});
  if(!A||!bg||!pe||!A.naturalWidth) return JSON.stringify({{ok:false,reason:'missing'}});
  var br=bg.getBoundingClientRect(), pr=pe.getBoundingClientRect();
  var cw=bg.width, ch=bg.height;                 // 底图 backing 分辨率(权威)
  if(!cw||!ch||!br.width||!br.height) return JSON.stringify({{ok:false,reason:'bad canvas'}});
  var sx=cw/br.width, sy=ch/br.height;
  var pxc=Math.round((pr.left-br.left)*sx), pyc=Math.round((pr.top-br.top)*sy);
  var pwc=Math.round(pr.width*sx), phc=Math.round(pr.height*sy);
  if(pwc<6||phc<6||!isFinite(pwc)||!isFinite(phc)) return JSON.stringify({{ok:false,reason:'bad geom'}});
  var N=pwc*phc;
  var bc=document.createElement('canvas'); bc.width=cw;bc.height=ch; var bx=bc.getContext('2d'); bx.drawImage(bg,0,0,cw,ch); var BG=bx.getImageData(0,0,cw,ch).data;
  var pcv=document.createElement('canvas'); pcv.width=pwc; pcv.height=phc; var px=pcv.getContext('2d'); px.drawImage(A,0,0,pwc,phc); var AD=px.getImageData(0,0,pwc,phc).data;
  function erode(src,it){{var cur=src; for(var t=0;t<it;t++){{var nx=new Uint8Array(N); for(var y=1;y<phc-1;y++)for(var x=1;x<pwc-1;x++){{var idx=y*pwc+x; if(cur[idx]&&cur[idx-1]&&cur[idx+1]&&cur[idx-pwc]&&cur[idx+pwc])nx[idx]=1;}} cur=nx;}} return cur;}}
  // (1) 绿环掩膜:检测绿色发光描边 → BFS 从四边填洞 → 内部 = 非绿且被绿环包住。
  var green=new Uint8Array(N), gcount=0;
  for(var k=0;k<N;k++){{ var o=k*4,R=AD[o],G=AD[o+1],B=AD[o+2]; if(G>R+15&&G>B+15&&G>80){{green[k]=1;gcount++;}} }}
  var reach=new Uint8Array(N), stk=[];
  function pushIf(x,y){{ if(x<0||y<0||x>=pwc||y>=phc)return; var k=y*pwc+x; if(!green[k]&&!reach[k]){{reach[k]=1;stk.push(k);}} }}
  for(var x=0;x<pwc;x++){{pushIf(x,0);pushIf(x,phc-1);}}
  for(var y=0;y<phc;y++){{pushIf(0,y);pushIf(pwc-1,y);}}
  while(stk.length){{ var k=stk.pop(),xx=k%pwc,yy=(k-(k%pwc))/pwc; pushIf(xx-1,yy);pushIf(xx+1,yy);pushIf(xx,yy-1);pushIf(xx,yy+1); }}
  var interior=new Uint8Array(N), icount=0;
  for(var k=0;k<N;k++){{ if(!green[k]&&!reach[k]){{interior[k]=1;icount++;}} }}
  if(gcount<40||icount<40){{   // 绿环不足 → 回退差分掩膜(拼图截图 vs 底图 home 裁剪)。
    interior=new Uint8Array(N); icount=0;
    for(var y=0;y<phc;y++)for(var x=0;x<pwc;x++){{ var sX=Math.min(cw-1,pxc+x),sY=Math.min(ch-1,pyc+y); var so=(sY*cw+sX)*4,k=y*pwc+x,ao=k*4; var d=Math.abs(AD[ao]-BG[so])+Math.abs(AD[ao+1]-BG[so+1])+Math.abs(AD[ao+2]-BG[so+2]); if(d>40){{interior[k]=1;icount++;}} }}
  }}
  if(icount<30) return JSON.stringify({{ok:false,reason:'thin mask i='+icount}});
  var core=erode(interior,2), ccount=0; for(var k=0;k<N;k++) ccount+=core[k];
  if(ccount<40){{ core=interior; ccount=icount; }}
  var full=new Uint8Array(N); for(var k=0;k<N;k++) full[k]=(green[k]||interior[k])?1:0;
  var fe=erode(full,1); var bnd=[]; for(var k=0;k<N;k++) if(full[k]&&!fe[k]) bnd.push(k);
  var cX=[],cY=[],pR=[],pG=[],pB=[],sumP=0;
  for(var y=0;y<phc;y++)for(var x=0;x<pwc;x++){{ var k=y*pwc+x; if(core[k]){{ var o=k*4; cX.push(x);cY.push(y); pR.push(AD[o]);pG.push(AD[o+1]);pB.push(AD[o+2]); sumP+=(AD[o]+AD[o+1]+AD[o+2])/3; }} }}
  var nc=cX.length, pl=sumP/nc;
  var mPR=0,mPG=0,mPB=0; for(var j=0;j<nc;j++){{mPR+=pR[j];mPG+=pG[j];mPB+=pB[j];}} mPR/=nc;mPG/=nc;mPB/=nc;
  var vPR=0,vPG=0,vPB=0; for(var j=0;j<nc;j++){{var a=pR[j]-mPR;vPR+=a*a;var b=pG[j]-mPG;vPG+=b*b;var c=pB[j]-mPB;vPB+=c*c;}}
  function lum(o){{return (BG[o]+BG[o+1]+BG[o+2])/3;}}
  var E=new Float32Array(cw*ch);
  for(var y=1;y<ch-1;y++)for(var x=1;x<cw-1;x++){{ var p=y*cw+x; E[p]=Math.abs(lum((p+1)*4)-lum((p-1)*4))+Math.abs(lum((p+cw)*4)-lum((p-cw)*4)); }}
  var skip=Math.floor(pwc/2), maxD=cw-pxc-pwc;
  if(maxD<=skip) return JSON.stringify({{ok:false,reason:'no room'}});
  var Dn=maxD-skip+1, nccA=new Float64Array(Dn), clA=new Float64Array(Dn), rimA=new Float64Array(Dn);
  // (2) 逐 D 扫:彩色内容 NCC + 核暗度 + 描边强度。
  for(var D=skip;D<=maxD;D++){{
    var mR=0,mG=0,mB=0,sumL=0;
    for(var j=0;j<nc;j++){{ var o=((pyc+cY[j])*cw+(pxc+D+cX[j]))*4; mR+=BG[o];mG+=BG[o+1];mB+=BG[o+2]; sumL+=(BG[o]+BG[o+1]+BG[o+2])/3; }}
    mR/=nc;mG/=nc;mB/=nc;
    var nR=0,dR=0,nG=0,dG=0,nB=0,dB=0;
    for(var j=0;j<nc;j++){{ var o=((pyc+cY[j])*cw+(pxc+D+cX[j]))*4; var er=BG[o]-mR,eg=BG[o+1]-mG,eb=BG[o+2]-mB;
      nR+=(pR[j]-mPR)*er; dR+=er*er; nG+=(pG[j]-mPG)*eg; dG+=eg*eg; nB+=(pB[j]-mPB)*eb; dB+=eb*eb; }}
    var cR=(vPR>1e-6&&dR>1e-6)?nR/Math.sqrt(vPR*dR):0, cG=(vPG>1e-6&&dG>1e-6)?nG/Math.sqrt(vPG*dG):0, cB=(vPB>1e-6&&dB>1e-6)?nB/Math.sqrt(vPB*dB):0;
    var idx=D-skip; nccA[idx]=(cR+cG+cB)/3; clA[idx]=sumL/nc;
    var rs=0; for(var b=0;b<bnd.length;b++){{ var kk=bnd[b],bxp=kk%pwc,byp=(kk-(kk%pwc))/pwc; rs+=E[(pyc+byp)*cw+(pxc+D+bxp)]; }}
    rimA[idx]=rs/bnd.length;
  }}
  // (3) 暗度门控:缺口不比拼图亮太多。
  var gate=new Uint8Array(Dn), gcnt=0; for(var idx=0;idx<Dn;idx++){{ if(clA[idx]<pl*1.08){{gate[idx]=1;gcnt++;}} }}
  if(gcnt<5) for(var idx=0;idx<Dn;idx++) gate[idx]=1;
  var nmax=-2,ip=0; for(var idx=0;idx<Dn;idx++){{ if(gate[idx]&&nccA[idx]>nmax){{nmax=nccA[idx];ip=idx;}} }}
  // (4) NCC 高置信 → 描边局部对齐微调;否则暗度+描边兜底。
  var bestIdx=ip, method, conf;
  if(nmax>=0.45){{
    var best=-1; for(var idx=0;idx<Dn;idx++){{ if(gate[idx]&&nccA[idx]>=0.8*nmax&&Math.abs(idx-ip)<=pwc*0.7&&rimA[idx]>best){{best=rimA[idx];bestIdx=idx;}} }}
    method='content_ncc'; conf=Math.max(0,Math.min(1,nmax));
  }} else {{
    var dmin=1e9,dmax=-1e9,rmin=1e9,rmax=-1e9;
    for(var idx=0;idx<Dn;idx++){{ if(gate[idx]){{ var dk=255-clA[idx]; if(dk<dmin)dmin=dk; if(dk>dmax)dmax=dk; if(rimA[idx]<rmin)rmin=rimA[idx]; if(rimA[idx]>rmax)rmax=rimA[idx]; }} }}
    var best=-1; for(var idx=0;idx<Dn;idx++){{ if(gate[idx]){{ var dn=(255-clA[idx]-dmin)/(dmax-dmin+1e-9), rn=(rimA[idx]-rmin)/(rmax-rmin+1e-9), s=dn*0.6+rn*0.4; if(s>best){{best=s;bestIdx=idx;}} }} }}
    method='content_ncc'; conf=Math.max(0,nmax);
  }}
  var bestD=skip+bestIdx, displace=bestD*br.width/cw;   // canvas px → CSS px
  return JSON.stringify({{ok:true, displace:displace, conf:conf, method:method}});
}})()"#,
        bg = json!(bg_sel),
        piece = json!(piece_sel)
    )
}

/// 确保验证图可见:把手不可见且配了 `open` 就点它。
async fn ensure_visible(tab: &Tab, cfg: &SliderConfig) -> Result<()> {
    let js = format!(
        "(function(){{var e=document.querySelector({sel}); if(!e)return false; var r=e.getBoundingClientRect(); return r.width>0&&r.height>0;}})()",
        sel = json!(cfg.handle)
    );
    if tab.run_js(&js).await?.as_bool() == Some(true) {
        sleep(Duration::from_millis(300)).await;
        return Ok(());
    }
    if let Some(open) = &cfg.open {
        if let Ok(btn) = tab.ele(&format!("css:{open}")).await {
            let _ = btn.click().await;
        }
        let _ = tab.ele(&format!("css:{}", cfg.handle)).await;
        sleep(Duration::from_millis(900)).await;
    }
    Ok(())
}

/// 换一张验证图:点 `refresh`(可逗号多选);没配则点 `open` 重开;都没有就静默。
async fn nudge_refresh(tab: &Tab, cfg: &SliderConfig) {
    if let Some(refresh) = &cfg.refresh {
        let js = format!(
            "(function(){{var ss={sels}.split(','); for(var i=0;i<ss.length;i++){{var e=document.querySelector(ss[i].trim()); if(e&&e.getBoundingClientRect().width>0){{e.click(); return true;}}}} return false;}})()",
            sels = json!(refresh)
        );
        let _ = tab.run_js(&js).await;
    } else if let Some(open) = &cfg.open {
        let _ = tab.run_js(&format!(
            "(function(){{var e=document.querySelector({sel}); if(e)e.click();}})()",
            sel = json!(open)
        )).await;
    }
    sleep(Duration::from_millis(1300)).await;
}

/// 判定是否通过。
async fn check_success(tab: &Tab, check: &SuccessCheck) -> bool {
    let js = match check {
        SuccessCheck::Visible(sel) => format!(
            "(function(){{var e=document.querySelector({sel}); if(!e)return false; var r=e.getBoundingClientRect(); return r.width>0&&r.height>0;}})()",
            sel = json!(sel)
        ),
        SuccessCheck::Js(expr) => format!("(function(){{try{{return !!({expr});}}catch(e){{return false;}}}})()"),
        SuccessCheck::None => return true,
    };
    tab.run_js(&js).await.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 读元素中心视口坐标 `[x,y]`。
async fn center(tab: &Tab, sel: &str) -> Result<(f64, f64)> {
    let js = format!(
        "(function(){{var e=document.querySelector({sel}); if(!e)return [0,0]; var r=e.getBoundingClientRect(); return [r.left+r.width/2, r.top+r.height/2];}})()",
        sel = json!(sel)
    );
    let v = tab.run_js(&js).await?;
    Ok((
        v.get(0).and_then(Value::as_f64).unwrap_or(0.0),
        v.get(1).and_then(Value::as_f64).unwrap_or(0.0),
    ))
}

/// 读拼图当前左缘视口 x:canvas 用"rect.left + 最左非透明列"(兼容 transform 与重绘);img 用 rect.left。
async fn piece_left(tab: &Tab, src: &ImageSource) -> f64 {
    let js = match src {
        ImageSource::Canvas(sel) => format!(
            r#"(function(){{var c=document.querySelector({sel}); if(!c)return -1; var r=c.getBoundingClientRect(),W=c.width;
  try{{var s=c.getContext('2d').getImageData(0,0,W,c.height).data,lx=-1;
    for(var x=0;x<W&&lx<0;x++){{for(var y=0;y<c.height;y++){{if(s[(y*W+x)*4+3]>0){{lx=x;break;}}}}}}
    if(lx<0)lx=0; return r.left+lx*(r.width/W);}}catch(e){{return r.left;}}}})()"#,
            sel = json!(sel)
        ),
        ImageSource::Img(sel) | ImageSource::Shot(sel) => format!(
            "(function(){{var c=document.querySelector({sel}); if(!c)return -1; return c.getBoundingClientRect().left;}})()",
            sel = json!(sel)
        ),
    };
    tab.run_js(&js).await.ok().and_then(|v| v.as_f64()).unwrap_or(-1.0)
}

/// 闭环/开环拖动把手,使拼图水平移动 `displace`(CSS px)。返回对齐误差(无 `piece` 则 `NaN`)。
async fn slide_drag(tab: &Tab, cfg: &SliderConfig, displace: f64) -> Result<f64> {
    let (mut hx, hy0) = center(tab, &cfg.handle).await?;
    let start_hx = hx;

    // 移动到把手(非瞬移)+ 略偏中心按下,更像人手。
    let press_x = hx + 3.0;
    let press_y = hy0 - 2.0;
    let (ax, ay) = (press_x - 46.0, press_y - 28.0);
    for i in 1..=6 {
        let t = i as f64 / 6.0;
        tab.mouse_move(ax + (press_x - ax) * t, ay + (press_y - ay) * t).await?;
        sleep(Duration::from_millis(25 + i * 6)).await;
    }
    hx = press_x;
    let hy = press_y;
    sleep(Duration::from_millis(110)).await;
    tab.mouse_down(hx, hy).await?;
    sleep(Duration::from_millis(150)).await;

    // 标定比例(有 piece 闭环;否则用 track_ratio/默认 1.0)。
    let (ratio, gap_screen) = if let Some(piece) = &cfg.piece {
        let piece0 = piece_left(tab, piece).await;
        for d in [1.5_f64, 2.5, 4.0, 5.5, 7.0] {
            hx += d;
            tab.mouse_drag(hx, hy + (d * 0.2)).await?;
            sleep(Duration::from_millis(45)).await;
        }
        let piece1 = piece_left(tab, piece).await;
        let ratio = ((piece1 - piece0) / (hx - start_hx)).clamp(0.2, 5.0);
        (ratio, Some(piece0 + displace))
    } else {
        (cfg.track_ratio.unwrap_or(1.0).clamp(0.05, 20.0), None)
    };
    let target_hx = start_hx + displace / ratio;

    // 主滑行:minimum-jerk 钟形速度 + 密集 fire + 手抖。
    let from = hx;
    let glide_dist = (target_hx - from).abs();
    let steps = ((glide_dist / 4.0).round() as i64).clamp(40, 90);
    let mut st = 0x2545_F491_4F6C_DD1Du64;
    let mut rnd = || {
        st = st.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = st;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let mj = 10.0 * t.powi(3) - 15.0 * t.powi(4) + 6.0 * t.powi(5);
        let nx = from + (target_hx - from) * mj;
        let jy = (rnd() - 0.5) * 2.0 + (i as f64 * 0.6).sin() * 0.8;
        tab.mouse_drag_fast(nx, hy + jy)?;
        let d = 9.0 + rnd() * 7.0;
        sleep(Duration::from_millis(d as u64)).await;
    }
    hx = target_hx;

    // fire 不等往返:一次会等待的 drag 作"屏障"。
    tab.mouse_drag(hx, hy).await?;
    sleep(Duration::from_millis(90)).await;

    // 闭环纠偏(仅有 piece 时):最多 6 次、步长 ±8、读前留沉降,确保大位移也能收敛到 ≤1px。
    let mut align_err = f64::NAN;
    if let (Some(piece), Some(target)) = (&cfg.piece, gap_screen) {
        for _ in 0..6 {
            sleep(Duration::from_millis(55)).await; // 等上一次拖动沉降再读,避免读到中途位置
            let err = target - piece_left(tab, piece).await;
            if err.abs() <= 1.0 {
                break;
            }
            hx += (err / ratio).clamp(-8.0, 8.0);
            tab.mouse_drag(hx, hy).await?;
        }
        sleep(Duration::from_millis(220)).await; // 到位后停顿再松手(更像人)
        align_err = target - piece_left(tab, piece).await;
    } else {
        sleep(Duration::from_millis(200)).await;
    }
    tab.mouse_up(hx, hy).await?;
    Ok(align_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_prefers_color_when_close() {
        assert_eq!(choose_displacement(73.0, 73.0), 73.0);
        assert_eq!(choose_displacement(78.0, 73.0), 73.0); // 差 5 ≤6 → 颜色法
        assert_eq!(choose_displacement(67.0, 73.0), 73.0);
    }

    #[test]
    fn choose_falls_back_to_shape_when_far() {
        assert_eq!(choose_displacement(90.0, 73.0), 90.0); // 差 17 >6 → 形状法
        assert_eq!(choose_displacement(40.0, 73.0), 40.0);
    }

    #[test]
    fn template_has_exactly_one_placeholder() {
        assert_eq!(MATCH_TEMPLATE.matches("__CFG__").count(), 1);
    }

    #[test]
    fn geetest_preset_is_two_image() {
        let c = SliderConfig::geetest_v4();
        assert!(c.full_bg.is_some() && c.piece.is_some());
        assert_eq!(c.bg, ImageSource::canvas(".geetest_canvas_bg"));
        assert_eq!(c.handle, ".geetest_slider_button");
    }

    #[test]
    fn dingxiang_preset_uses_shot_and_content_ncc() {
        let c = SliderConfig::dingxiang(4);
        assert_eq!(c.bg, ImageSource::canvas("#dx_captcha_basic_bg_4 canvas"));
        assert_eq!(c.piece, Some(ImageSource::shot("#dx_captcha_basic_sub-slider_4 img")));
        assert_eq!(c.handle, "#dx_captcha_basic_slider_4");
        assert_eq!(c.refresh.as_deref(), Some("#dx_captcha_basic_btn-refresh_4"));
        assert!(matches!(c.success, SuccessCheck::Js(_)));
    }

    #[test]
    fn gapmethod_parses_content_ncc() {
        assert_eq!(GapMethod::parse("content_ncc"), GapMethod::ContentNcc);
        assert_eq!(GapMethod::parse("two_image"), GapMethod::TwoImage);
    }

    #[test]
    fn content_ncc_js_interpolates_selectors_and_shot_id() {
        let js = content_ncc_js("#bg canvas", "#piece img");
        assert!(js.contains("\"#bg canvas\""));
        assert!(js.contains("\"#piece img\""));
        // 注入的截图 img id 固定且唯一引用一次(getElementById)。
        assert_eq!(js.matches("getElementById('__drission_shot')").count(), 1);
    }

    #[test]
    fn builder_sets_fields() {
        let c = SliderConfig::new(ImageSource::img("#bg"), "#h")
            .piece(ImageSource::img("#p"))
            .open("#o")
            .track_ratio(0.9)
            .max_attempts(3);
        assert_eq!(c.bg, ImageSource::img("#bg"));
        assert_eq!(c.piece, Some(ImageSource::img("#p")));
        assert_eq!(c.open.as_deref(), Some("#o"));
        assert_eq!(c.track_ratio, Some(0.9));
        assert_eq!(c.max_attempts, 3);
    }
}
