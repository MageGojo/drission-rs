//! 磁盘 Profile 管理:每个账号一份目录(Chrome user-data-dir + 清单 + 文件锁)。
//!
//! ```ignore
//! let profiles = ProfileManager::open("profiles")?;
//! let lease = profiles.acquire("user_001").await?;
//! let browser = lease.launch().await?;
//! ```
//!
//! Chrome 目录里自带 Cookie / LocalStorage / IndexedDB / Cache。清单只描述
//! **启动时**要套上的 Proxy / UA / locale / timezone / screen / args。
//!
//! 仅用于你拥有或已获明确授权的系统。不要用本模块批量伪造身份去绕过站点控制。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cdp::browser::ChromiumBrowser;
use crate::cdp::context::ContextSpec;
use crate::cdp::options::ChromiumOptions;
use crate::{Error, Result};

/// 一个磁盘 profile 的启动清单(与 Chrome 数据目录并列)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileManifest {
    pub id: String,
    pub proxy: Option<String>,
    pub user_agent: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub screen: Option<(u32, u32)>,
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
    #[serde(default)]
    pub args: Vec<String>,
    pub notes: Option<String>,
}

impl ProfileManifest {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Default::default()
        }
    }

    pub fn to_options(&self, chrome_dir: &Path) -> ChromiumOptions {
        let mut o = ChromiumOptions::new().user_data_dir(chrome_dir);
        if let Some(ua) = &self.user_agent {
            o = o.user_agent(ua);
        }
        if let Some(l) = &self.locale {
            o = o.locale(l);
        }
        if let Some(tz) = &self.timezone {
            o = o.timezone(tz);
        }
        if let Some(p) = &self.proxy {
            o = o.proxy(p);
        }
        if let Some((w, h)) = self.screen {
            o = o.window_size(w, h);
        }
        for a in &self.args {
            o = o.add_arg(a);
        }
        o
    }

    pub fn to_context_spec(&self) -> ContextSpec {
        let mut s = ContextSpec::new().name(&self.id);
        if let Some(p) = &self.proxy {
            s = s.proxy(p);
        }
        if let Some(ua) = &self.user_agent {
            s = s.user_agent(ua);
        }
        if let Some(l) = &self.locale {
            s = s.locale(l);
        }
        if let Some(tz) = &self.timezone {
            s = s.timezone(tz);
        }
        if !self.extra_headers.is_empty() {
            s = s.extra_headers(self.extra_headers.clone());
        }
        if let Some((w, h)) = self.screen {
            s = s.viewport(w, h);
        }
        s
    }
}

/// `profiles/` 根目录上的管理器。
#[derive(Debug, Clone)]
pub struct ProfileManager {
    root: PathBuf,
}

impl ProfileManager {
    /// 打开(或创建)根目录。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 列出已有 profile id(子目录名)。
    pub fn list(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for ent in fs::read_dir(&self.root)? {
            let ent = ent?;
            if ent.file_type()?.is_dir() {
                if let Some(n) = ent.file_name().to_str() {
                    if !n.starts_with('.') {
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// 确保 `id` 目录与默认清单存在。
    pub fn ensure(&self, id: &str) -> Result<ManagedProfile> {
        validate_id(id)?;
        let dir = self.root.join(id);
        let chrome = dir.join("chrome");
        fs::create_dir_all(&chrome)?;
        let manifest_path = dir.join("manifest.json");
        let manifest = if manifest_path.exists() {
            let raw = fs::read_to_string(&manifest_path)?;
            let mut m: ProfileManifest = serde_json::from_str(&raw)?;
            if m.id.is_empty() {
                m.id = id.to_string();
            }
            m
        } else {
            let m = ProfileManifest::new(id);
            let mut f = File::create(&manifest_path)?;
            f.write_all(serde_json::to_string_pretty(&m)?.as_bytes())?;
            m
        };
        Ok(ManagedProfile {
            id: id.to_string(),
            dir,
            chrome_dir: chrome,
            manifest,
        })
    }

    pub fn get(&self, id: &str) -> Result<ManagedProfile> {
        validate_id(id)?;
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(Error::Other(format!("profile `{id}` 不存在: {}", dir.display())));
        }
        self.ensure(id)
    }

    /// 创建或取出 `id`,并加上排他文件锁。同一 profile 同时只应有一个浏览器。
    pub async fn acquire(&self, id: &str) -> Result<ProfileLease> {
        let profile = self.ensure(id)?;
        let lock = lock_profile(&profile.dir)?;
        Ok(ProfileLease { profile, _lock: lock })
    }
}

/// 磁盘上的一份 profile(未持锁)。
#[derive(Debug, Clone)]
pub struct ManagedProfile {
    pub id: String,
    pub dir: PathBuf,
    pub chrome_dir: PathBuf,
    pub manifest: ProfileManifest,
}

impl ManagedProfile {
    pub fn options(&self) -> ChromiumOptions {
        self.manifest.to_options(&self.chrome_dir)
    }

    pub fn context_spec(&self) -> ContextSpec {
        self.manifest.to_context_spec()
    }

    /// 写回清单(改 proxy / UA 等之后)。
    pub fn save_manifest(&self) -> Result<()> {
        let path = self.dir.join("manifest.json");
        fs::write(path, serde_json::to_string_pretty(&self.manifest)?)?;
        Ok(())
    }
}

/// 持有排他锁的 profile 租约。Drop 即释放。
pub struct ProfileLease {
    profile: ManagedProfile,
    _lock: ProfileLock,
}

impl ProfileLease {
    pub fn id(&self) -> &str {
        &self.profile.id
    }
    pub fn profile(&self) -> &ManagedProfile {
        &self.profile
    }
    pub fn options(&self) -> ChromiumOptions {
        self.profile.options()
    }
    pub fn context_spec(&self) -> ContextSpec {
        self.profile.context_spec()
    }

    /// 用本 profile 的 user-data-dir 启动浏览器(登录态跨进程保留)。
    pub async fn launch(&self) -> Result<ChromiumBrowser> {
        ChromiumBrowser::launch(self.options()).await
    }
}

struct ProfileLock {
    file: File,
    path: PathBuf,
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.starts_with('.')
    {
        return Err(Error::Other(format!("非法 profile id: {id}")));
    }
    Ok(())
}

fn lock_profile(dir: &Path) -> Result<ProfileLock> {
    let path = dir.join(".lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(Error::Other(format!(
                "profile 已被占用: {}",
                dir.display()
            )));
        }
    }
    #[cfg(windows)]
    {
        // 已用 create+truncate 占位;同机并发再靠 identity-job 调度。严格锁后续可接 LockFileEx。
        let _ = &file;
    }
    let _ = writeln!(&file, "{}", std::process::id());
    Ok(ProfileLock { file, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_and_list() {
        let root = std::env::temp_dir().join(format!("drs-profile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mgr = ProfileManager::open(&root).unwrap();
        let p = mgr.ensure("user_001").unwrap();
        assert!(p.chrome_dir.is_dir());
        assert!(p.dir.join("manifest.json").is_file());
        assert_eq!(mgr.list().unwrap(), vec!["user_001".to_string()]);
        assert!(validate_id("../x").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn acquire_is_exclusive() {
        let root = std::env::temp_dir().join(format!("drs-profile-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mgr = ProfileManager::open(&root).unwrap();
        let lease = mgr.acquire("user_002").await.unwrap();
        #[cfg(unix)]
        {
            let again = mgr.acquire("user_002").await;
            assert!(again.is_err(), "第二把锁应失败");
        }
        drop(lease);
        let _ = mgr.acquire("user_002").await.unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn manifest_to_options() {
        let m = ProfileManifest {
            id: "u".into(),
            proxy: Some("http://127.0.0.1:9".into()),
            user_agent: Some("UA".into()),
            locale: Some("en-US".into()),
            timezone: Some("UTC".into()),
            screen: Some((800, 600)),
            extra_headers: vec![("X".into(), "1".into())],
            args: vec!["--mute-audio".into()],
            notes: None,
        };
        let o = m.to_options(Path::new("/tmp/chrome"));
        assert_eq!(o.proxy.as_deref(), Some("http://127.0.0.1:9"));
        assert_eq!(o.user_agent.as_deref(), Some("UA"));
        assert!(o.args.iter().any(|a| a == "--mute-audio"));
        let s = m.to_context_spec();
        assert_eq!(s.name.as_deref(), Some("u"));
        assert_eq!(s.extra_headers.len(), 1);
    }
}
