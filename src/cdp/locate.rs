//! 定位本机 **Chromium 系浏览器**(Google Chrome / Edge / Brave / Chromium)可执行文件。
//!
//! 探测优先级**对标 DrissionPage 的 `get_chrome_path`**,并补齐其在 Windows 上的薄弱点:
//!
//! 1. 环境变量 `CHROME_BIN` / `DRISSION_CHROME`(显式指定,最高优先)。
//! 2. 各平台**常见安装路径**——Windows 同时覆盖**用户级** `%LOCALAPPDATA%`(免管理员安装,
//!    极常见)与**系统级** `%PROGRAMFILES%` / `%PROGRAMFILES(X86)%` / `%PROGRAMW6432%`。
//! 3. **Windows 注册表** `App Paths\chrome.exe`(`HKEY_CURRENT_USER` 优先,再 `HKEY_LOCAL_MACHINE`)
//!    ——这是 Windows 应用注册自身可执行路径的规范位置,也是 DrissionPage 的核心做法。
//! 4. 系统 `PATH` 环境变量里的 chrome / chromium / brave / edge 可执行文件。
//!
//! 全程**优先 Google Chrome**(满足“默认支持谷歌浏览器”),其次 Chromium / Brave / Edge。

use std::path::PathBuf;

use crate::{Error, Result};

/// 定位 Chrome/Edge/Brave/Chromium 可执行文件。探测优先级见[模块文档](self)。
///
/// 找不到时返回错误,提示安装 Google Chrome 或设置 `CHROME_BIN`。
pub fn chrome_path() -> Result<PathBuf> {
    // 1. 显式环境变量(库专用 `DRISSION_CHROME` 与通用 `CHROME_BIN` 都认)。
    for var in ["CHROME_BIN", "DRISSION_CHROME"] {
        if let Some(v) = std::env::var_os(var) {
            let pb = PathBuf::from(v);
            if pb.is_file() {
                return Ok(pb);
            }
        }
    }

    // 2. 各平台常见安装路径(Chrome 优先)。
    if let Some(p) = first_existing(install_candidates()) {
        return Ok(p);
    }

    // 3. Windows 注册表 App Paths(对标 DrissionPage)。
    #[cfg(windows)]
    if let Some(s) = from_registry() {
        let pb = PathBuf::from(&s);
        if pb.is_file() {
            return Ok(pb);
        }
    }

    // 4. 系统 PATH 扫描。
    if let Some(p) = from_path_env() {
        return Ok(p);
    }

    Err(Error::msg(
        "CDP: 未找到 Chrome/Edge/Brave/Chromium。请安装 Google Chrome,\
         或用环境变量 CHROME_BIN / DRISSION_CHROME 指定浏览器可执行文件路径。",
    ))
}

/// 各平台常见安装路径(Chrome 优先,然后 Chromium / Brave / Edge)。
fn install_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();

    #[cfg(target_os = "macos")]
    {
        for p in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ] {
            v.push(PathBuf::from(p));
        }
    }

    #[cfg(target_os = "windows")]
    {
        // 安装路径 = {基目录} + {相对子路径};基目录覆盖用户级与系统级。
        let suffixes = [
            r"Google\Chrome\Application\chrome.exe",
            r"Google\Chrome Beta\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
            r"Microsoft\Edge\Application\msedge.exe",
        ];
        // 用户级安装在 %LOCALAPPDATA%(免管理员,极常见);系统级在 Program Files 系列。
        for base_var in [
            "LOCALAPPDATA",
            "PROGRAMFILES",
            "ProgramFiles(x86)",
            "PROGRAMW6432",
        ] {
            if let Some(base) = std::env::var_os(base_var) {
                let base = PathBuf::from(base);
                for s in suffixes {
                    v.push(base.join(s));
                }
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for p in [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/opt/google/chrome/google-chrome",
            "/opt/google/chrome/chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/usr/bin/brave-browser",
            "/usr/bin/microsoft-edge",
        ] {
            v.push(PathBuf::from(p));
        }
    }

    v
}

/// 返回第一个存在的可执行文件。
fn first_existing(paths: Vec<PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|p| p.is_file())
}

/// PATH 扫描用的可执行文件名(Chrome 优先)。
fn browser_exe_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["chrome.exe", "chromium.exe", "brave.exe", "msedge.exe"]
    }
    #[cfg(not(windows))]
    {
        &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "brave-browser",
            "microsoft-edge",
        ]
    }
}

/// 在给定目录集合里按名字找第一个存在的可执行文件(纯函数,便于单测)。
fn scan_dirs(dirs: impl IntoIterator<Item = PathBuf>, names: &[&str]) -> Option<PathBuf> {
    for dir in dirs {
        for name in names {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// 从系统 `PATH` 环境变量里找 chrome / chromium / brave / edge。
/// 用 [`std::env::split_paths`] 正确处理 Windows(`;`)与 unix(`:`)的分隔符。
fn from_path_env() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    scan_dirs(std::env::split_paths(&path), browser_exe_names())
}

/// 从 Windows 注册表 `App Paths\chrome.exe` 的**默认值**读取 chrome.exe 完整路径。
/// 对标 DrissionPage:先查 `HKEY_CURRENT_USER`,失败再查 `HKEY_LOCAL_MACHINE`。
#[cfg(windows)]
fn from_registry() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
        RegQueryValueExW,
    };

    // 以 NUL 结尾的宽字符串子键。
    let subkey: Vec<u16> = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            // 打开失败(键不存在)→ 试下一个根。
            if RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
                continue;
            }
            // 查询默认值(lpValueName = NULL):App Paths 的默认值即 exe 完整路径。
            let mut buf = [0u16; 1024];
            let mut len: u32 = (buf.len() * 2) as u32; // 入参为缓冲字节数
            let st = RegQueryValueExW(
                hkey,
                std::ptr::null(),     // lpValueName = NULL → 默认值
                std::ptr::null_mut(), // lpReserved
                std::ptr::null_mut(), // lpType
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            );
            RegCloseKey(hkey);
            if st == 0 && len >= 2 {
                let count = (len as usize / 2).min(buf.len());
                let s = String::from_utf16_lossy(&buf[..count]);
                let s = s.trim_end_matches('\0').trim().trim_matches('"').trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_candidates_nonempty_and_chromium_family() {
        let c = install_candidates();
        assert!(!c.is_empty(), "每个支持平台都应有候选安装路径");
        assert!(
            c.iter().any(|p| {
                let s = p.to_string_lossy().to_lowercase();
                s.contains("chrome")
                    || s.contains("chromium")
                    || s.contains("brave")
                    || s.contains("edge")
            }),
            "候选路径应含 chromium 家族浏览器: {c:?}"
        );
    }

    #[test]
    fn browser_exe_names_prefers_chrome() {
        let names = browser_exe_names();
        assert!(!names.is_empty());
        assert!(
            names[0].contains("chrome"),
            "PATH 扫描应优先 Google Chrome: {names:?}"
        );
    }

    #[test]
    fn scan_dirs_finds_created_file_and_skips_missing() {
        let dir = std::env::temp_dir().join(format!(
            "drission_locate_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let exe = dir.join("mybrowser-bin");
        std::fs::write(&exe, b"x").expect("写临时可执行文件");

        // 第二个名字命中(验证按名顺序查找)。
        let found = scan_dirs(vec![dir.clone()], &["nope", "mybrowser-bin"]);
        assert_eq!(found.as_deref(), Some(exe.as_path()));

        // 全不存在返回 None。
        assert!(scan_dirs(vec![dir.clone()], &["does-not-exist"]).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_existing_returns_none_for_all_missing() {
        let missing = vec![
            PathBuf::from("/no/such/chrome/binary/xyz"),
            PathBuf::from("/another/missing/edge"),
        ];
        assert!(first_existing(missing).is_none());
    }
}
