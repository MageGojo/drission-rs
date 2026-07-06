//! 容器 / 无 GPU Linux 环境探测 + 「**补环境**」——让无头 / 有头-via-Xvfb 的浏览器在 **Docker /
//! 云服务器**里也能被强风控(如易盾点选)当成真实桌面,从而**下发并通过挑战**。
//!
//! ## 为什么需要(里程碑 73 在 Docker 里挑战图始终不弹的根因)
//! Docker / 云主机 / `Xvfb`「假有头」里 Chrome **没有真实 GPU**,WebGL 只能走软件渲染
//! (SwiftShader / llvmpipe / Mesa softpipe)。这带来两个**强机器人信号**:
//! 1. **近期 Chrome(≥121)默认禁用 SwiftShader 的 WebGL**,不加 `--enable-unsafe-swiftshader`
//!    时 `canvas.getContext('webgl')` 直接返回 `null` —— "**完全没有 WebGL**" 比软渲染更可疑;
//! 2. 即便可用,`UNMASKED_RENDERER_WEBGL` 报 `Google SwiftShader` / `llvmpipe` / `SwiftShader Device`,
//!    行为风控据此判定为机器人,**干脆不下发点选图**。
//!
//! ## 「补环境」两手(自动,识别到容器/无 GPU Linux 即生效)
//! - **启动参数**(见 [`browser`](crate::cdp::browser)):自动补 `--enable-unsafe-swiftshader`,
//!   保证软件 WebGL **可用**(否则下面的字符串改写无的放矢——上下文都建不出来);
//! - **导航前注入**([`spoof_js`]):hook `getParameter`,把 `UNMASKED_VENDOR/RENDERER_WEBGL`
//!   改写成与 **Linux UA 自洽**的常见桌面 GPU(默认 Intel Mesa),抹掉 SwiftShader/llvmpipe 破绽;
//!   用 `Proxy` 包裹原生函数 → `getParameter.toString()` 仍是 `[native code]`(不留替换痕迹)。
//!
//! ## 触发条件([`should_spoof`])
//! **Linux + stealth + (检测到容器 或 无 DRM 渲染节点 `/dev/dri/renderD*`)**。
//! 后者能顺带覆盖"裸机无 GPU 的 Linux 服务器 + Xvfb"(非容器但同样软渲染)。
//! - `ChromiumOptions::spoof_container(bool)` 显式开/关(优先级最高);
//! - 环境变量 `DRISSION_CONTAINER_SPOOF=1/0` 同义;
//! - `DRISSION_WEBGL_VENDOR` / `DRISSION_WEBGL_RENDERER` 自定义改写的 GPU 字符串。
//!
//! 非 Linux 一律不触发(改写的是 Linux GPU 字符串,在 mac/Win 上反而自相矛盾)。

/// 解析"开/关"型环境变量:`1/on/true/yes` → `Some(true)`,`0/off/false/no` → `Some(false)`,
/// 其余(未设/无法识别)→ `None`。
fn env_flag(key: &str) -> Option<bool> {
    match std::env::var(key).ok().as_deref().map(str::trim) {
        Some("1") | Some("on") | Some("true") | Some("yes") => Some(true),
        Some("0") | Some("off") | Some("false") | Some("no") => Some(false),
        _ => None,
    }
}

/// 是否运行在容器里(Docker / Podman / Kubernetes / LXC)。仅 Linux 有意义。
///
/// 逐项探测(任一命中即为真):`/.dockerenv`(Docker)、`/run/.containerenv`(Podman)、
/// `/proc/1/cgroup` 或 `/proc/self/cgroup` 含 `docker`/`kubepods`/`containerd`/`lxc`/`podman`。
pub(crate) fn detect_container() -> bool {
    if std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
    {
        return true;
    }
    let hit = |p: &str| {
        std::fs::read_to_string(p)
            .map(|s| {
                let s = s.to_ascii_lowercase();
                s.contains("docker")
                    || s.contains("kubepods")
                    || s.contains("containerd")
                    || s.contains("/lxc/")
                    || s.contains("libpod")
                    || s.contains("podman")
            })
            .unwrap_or(false)
    };
    hit("/proc/1/cgroup") || hit("/proc/self/cgroup")
}

/// 是否存在 **DRM 渲染节点**(`/dev/dri/renderD*`)—— 真实 GPU 在 Linux 上的可靠标志。
///
/// 容器(未做 GPU 直通)/ 多数云主机 / Xvfb 下 `/dev/dri` 不存在或无 `renderD*` → 返回 `false`
/// ⇒ 几乎必然软件渲染,应补环境。真实桌面/工作站 GPU 则有 `renderD128` 等 → 返回 `true`,不补
/// (其真实 renderer 本就可信)。非 Linux 恒 `true`(不触发补环境)。
pub(crate) fn has_dri_render_node() -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    match std::fs::read_dir("/dev/dri") {
        Ok(entries) => entries.flatten().any(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("renderD"))
                .unwrap_or(false)
        }),
        Err(_) => false,
    }
}

/// 是否应对本次启动做 GPU 补环境。`forced` 来自 [`ChromiumOptions::spoof_container`](crate::cdp::ChromiumOptions::spoof_container)。
///
/// 优先级:**显式选项 > 环境变量 `DRISSION_CONTAINER_SPOOF` > 自动判定**。非 Linux 恒 `false`。
/// 自动判定 = `stealth && (容器 || 无 DRM 渲染节点)`。
pub(crate) fn should_spoof(stealth: bool, forced: Option<bool>) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    if let Some(f) = forced {
        return f;
    }
    if let Some(f) = env_flag("DRISSION_CONTAINER_SPOOF") {
        return f;
    }
    stealth && (detect_container() || !has_dri_render_node())
}

/// 改写用的 WebGL vendor(`UNMASKED_VENDOR_WEBGL`)。优先级:显式参数 > `DRISSION_WEBGL_VENDOR` >
/// 默认 `Google Inc. (Intel)`(与 Linux 上经 ANGLE 的 Chrome 一致)。
pub(crate) fn spoof_vendor(explicit: Option<&str>) -> String {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("DRISSION_WEBGL_VENDOR").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Google Inc. (Intel)".to_string())
}

/// 改写用的 WebGL renderer(`UNMASKED_RENDERER_WEBGL`)。优先级:显式参数 > `DRISSION_WEBGL_RENDERER`
/// > 默认一台常见 Linux Intel 集显(经 ANGLE/Mesa,真实桌面 Chrome 的典型取值)。
pub(crate) fn spoof_renderer(explicit: Option<&str>) -> String {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("DRISSION_WEBGL_RENDERER").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics 620 (KBL GT2), OpenGL 4.6 (Core Profile) Mesa 22.3.6)"
                .to_string()
        })
}

/// 生成 WebGL renderer 补环境**导航前注入脚本**:hook `WebGL{,2}RenderingContext.prototype.getParameter`,
/// 对 `UNMASKED_VENDOR_WEBGL(37445)` / `UNMASKED_RENDERER_WEBGL(37446)` 返回伪装值,其余原样转发。
///
/// 用 `Proxy` 包裹**原生** `getParameter`:`apply` 拦截改写、`get` 拦截让 `toString()` 等仍落到原生
/// 函数(`getParameter.toString()` ⇒ `function getParameter() { [native code] }`,不暴露被替换);
/// `__dfc` 自标记避免重复包裹(再注入/与其它指纹脚本叠加时幂等)。
pub(crate) fn spoof_js(vendor: &str, renderer: &str) -> String {
    SPOOF_JS_TEMPLATE
        .replace("__VENDOR__", &json_str(vendor))
        .replace("__RENDERER__", &json_str(renderer))
}

/// 把 Rust 字符串编码为 JS 字面量(JSON 双引号字符串),安全嵌进模板。
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

const SPOOF_JS_TEMPLATE: &str = r#"(function(){
  try{
    var V=__VENDOR__, R=__RENDERER__;
    var wrap=function(proto){
      if(!proto) return;
      var gp=proto.getParameter;
      if(!gp || gp.__dfc) return;
      var np=new Proxy(gp,{
        apply:function(t,self,args){
          var p=args[0];
          if(p===37445) return V;   // UNMASKED_VENDOR_WEBGL
          if(p===37446) return R;   // UNMASKED_RENDERER_WEBGL
          return Reflect.apply(t,self,args);
        },
        get:function(t,k,r){ if(k==='__dfc') return true; return Reflect.get(t,k,r); }
      });
      try{ proto.getParameter=np; }catch(e){}
    };
    if(window.WebGLRenderingContext) wrap(WebGLRenderingContext.prototype);
    if(window.WebGL2RenderingContext) wrap(WebGL2RenderingContext.prototype);
  }catch(e){}
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_parses_common_truthy_falsy() {
        // 用真实环境变量名(进程内独占设置,不与其它测试并发同名变量)。
        unsafe { std::env::set_var("DRISSION_TEST_FLAG_X", "1") };
        assert_eq!(env_flag("DRISSION_TEST_FLAG_X"), Some(true));
        unsafe { std::env::set_var("DRISSION_TEST_FLAG_X", "off") };
        assert_eq!(env_flag("DRISSION_TEST_FLAG_X"), Some(false));
        unsafe { std::env::set_var("DRISSION_TEST_FLAG_X", "maybe") };
        assert_eq!(env_flag("DRISSION_TEST_FLAG_X"), None);
        unsafe { std::env::remove_var("DRISSION_TEST_FLAG_X") };
        assert_eq!(env_flag("DRISSION_TEST_FLAG_X"), None);
    }

    #[test]
    fn non_linux_never_spoofs() {
        // 在非 Linux(如开发用的 mac)上,即便强制 forced=Some(true) 也不触发(改写的是 Linux GPU 串)。
        if !cfg!(target_os = "linux") {
            assert!(!should_spoof(true, Some(true)));
            assert!(!should_spoof(true, None));
        }
    }

    #[test]
    fn linux_forced_option_wins() {
        if cfg!(target_os = "linux") {
            assert!(
                should_spoof(false, Some(true)),
                "显式 true 强制开,忽略 stealth"
            );
            assert!(!should_spoof(true, Some(false)), "显式 false 强制关");
        }
    }

    #[test]
    fn vendor_renderer_defaults_and_explicit() {
        // 默认是 Linux Intel/Mesa 取值。
        assert!(spoof_vendor(None).contains("Intel"));
        let r = spoof_renderer(None);
        assert!(r.contains("ANGLE") && r.contains("Mesa") && r.contains("Intel"));
        assert!(!r.contains("SwiftShader") && !r.contains("llvmpipe"));
        // 显式参数优先。
        assert_eq!(
            spoof_vendor(Some("Google Inc. (NVIDIA)")),
            "Google Inc. (NVIDIA)"
        );
        assert_eq!(
            spoof_renderer(Some("ANGLE (NVIDIA, ...)")),
            "ANGLE (NVIDIA, ...)"
        );
    }

    #[test]
    fn spoof_js_hooks_unmasked_params_with_proxy() {
        let js = spoof_js(
            "Google Inc. (Intel)",
            "ANGLE (Intel, Mesa Intel(R) UHD Graphics 620 (KBL GT2), OpenGL 4.6)",
        );
        // 改写两个 UNMASKED 常量。
        assert!(js.contains("37445") && js.contains("37446"));
        // 注入的字符串经 JSON 编码(含括号/逗号也安全)。
        assert!(js.contains("Mesa Intel(R) UHD Graphics 620"));
        // 用 Proxy 包裹原生函数(toString 仍 native)、带幂等标记。
        assert!(js.contains("new Proxy") && js.contains("Reflect.apply"));
        assert!(js.contains("__dfc"));
        // 单一 IIFE。
        assert!(js.starts_with("(function()") && js.trim_end().ends_with("})();"));
        // 绝不能把软渲染串写进改写值(那等于没改)。
        assert!(!js.contains("SwiftShader") && !js.contains("llvmpipe"));
    }
}
