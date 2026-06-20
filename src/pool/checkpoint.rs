//! 断点续抓:用 JSONL 文件持久化"已完成的 key",中断后重跑只补未完成的。
//!
//! 设计取最简可靠:每完成一个 key 就**追加**一行 `{"key": "...", "value": ...}`(进程崩溃也不丢
//! 已落盘部分);加载时读回所有 key 建集合。与 [`BrowserPool::map_resumable`](super::BrowserPool::map_resumable)
//! 配合即可"中断续作"。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::Result;

/// 一个文件支持的断点记录。克隆代价低(内部 `Arc`),可在多任务并发标记完成。
#[derive(Clone)]
pub struct Checkpoint {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    /// 已完成 key 集合 + 写锁(同一把锁串行化"判重 + 追加",保证落盘原子有序)。
    done: Mutex<HashSet<String>>,
}

impl Checkpoint {
    /// 从文件加载已完成的 key(文件不存在视为空)。后续 [`mark_done`](Self::mark_done) 追加写同一文件。
    pub async fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut done = HashSet::new();
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(line)
                    && let Some(k) = v.get("key").and_then(|x| x.as_str())
                {
                    done.insert(k.to_string());
                }
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                path,
                done: Mutex::new(done),
            }),
        })
    }

    /// 该 key 是否已完成。
    pub async fn is_done(&self, key: &str) -> bool {
        self.inner.done.lock().await.contains(key)
    }

    /// 已完成的 key 数量。
    pub async fn done_count(&self) -> usize {
        self.inner.done.lock().await.len()
    }

    /// 标记一个 key 已完成并落盘(幂等:重复标记同一 key 不重复写)。`value` 可选,随行持久化。
    ///
    /// 先写文件再入集合:若写盘失败,key 不算完成(下次会重试)。
    pub async fn mark_done(&self, key: &str, value: Option<Value>) -> Result<()> {
        let mut set = self.inner.done.lock().await;
        if set.contains(key) {
            return Ok(());
        }
        let mut line = json!({ "key": key });
        if let Some(v) = value {
            line["value"] = v;
        }
        let mut text = line.to_string();
        text.push('\n');

        if let Some(parent) = self.inner.path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.inner.path)
            .await?;
        f.write_all(text.as_bytes()).await?;
        f.flush().await?;

        set.insert(key.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        // 写到项目 target 目录下(在 home 下、已 gitignore),避免 /var/folders 沙箱问题。
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("target");
        p.push("test-tmp");
        p.push(format!("ckpt-{}-{}.jsonl", name, std::process::id()));
        p
    }

    #[tokio::test]
    async fn mark_and_reload_roundtrip() {
        let path = tmp_path("roundtrip");
        let _ = tokio::fs::remove_file(&path).await;

        let ck = Checkpoint::load(&path).await.unwrap();
        assert_eq!(ck.done_count().await, 0);
        ck.mark_done("a", None).await.unwrap();
        ck.mark_done("b", Some(json!({"n": 1}))).await.unwrap();
        ck.mark_done("a", None).await.unwrap(); // 幂等
        assert_eq!(ck.done_count().await, 2);
        assert!(ck.is_done("a").await);
        assert!(!ck.is_done("zzz").await);

        // 重新加载:已完成集合应恢复。
        let ck2 = Checkpoint::load(&path).await.unwrap();
        assert_eq!(ck2.done_count().await, 2);
        assert!(ck2.is_done("a").await && ck2.is_done("b").await);

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn missing_file_is_empty() {
        let path = tmp_path("missing");
        let _ = tokio::fs::remove_file(&path).await;
        let ck = Checkpoint::load(&path).await.unwrap();
        assert_eq!(ck.done_count().await, 0);
    }
}
