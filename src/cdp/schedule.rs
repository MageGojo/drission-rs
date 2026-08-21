//! 任务调度:在现有 [`ChromiumPool`] 外包一层超时。
//!
//! 并发、代理轮换、重试、空闲回收仍在池里。这里只表达
//! `Task → Scheduler → Pool → Browser → Tab`。

use std::future::Future;
use std::time::Duration;

use super::pool::{ChromiumPool, PoolLease};
use crate::{Error, Result};

/// 池上的任务调度。
pub struct TaskScheduler {
    pool: ChromiumPool,
    task_timeout: Duration,
}

impl TaskScheduler {
    pub fn new(pool: ChromiumPool) -> Self {
        Self {
            pool,
            task_timeout: Duration::from_secs(120),
        }
    }

    pub fn task_timeout(mut self, d: Duration) -> Self {
        self.task_timeout = d;
        self
    }

    pub fn pool(&self) -> &ChromiumPool {
        &self.pool
    }

    /// 取一个租约跑任务;超时当失败。
    pub async fn run<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(PoolLease) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let lease = self.pool.acquire().await?;
        match tokio::time::timeout(self.task_timeout, f(lease)).await {
            Ok(r) => r,
            Err(_) => Err(Error::Timeout(self.task_timeout)),
        }
    }
}

impl ChromiumPool {
    pub fn scheduler(self) -> TaskScheduler {
        TaskScheduler::new(self)
    }
}
