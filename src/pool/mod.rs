//! 高并发规模化采集:浏览器 worker 池 + 任务编排。
//!
//! [`BrowserPool`] 管理一组浏览器进程(worker),提供:
//! - **并发上限**:总并发槽 = `size * tabs_per_worker`,用信号量限流。
//! - **每任务轮换代理/指纹**:从 [`ProxyPool`] / [`FingerprintPool`] 取,拼成 per-context 覆盖。
//! - **失败重试**:按 [`RetryPolicy`] 重试(可换 worker)。
//! - **健康自愈**:worker 连接断/进程退时惰性重建。
//! - **断点续抓**:配合 [`Checkpoint`] 的 [`map_resumable`](BrowserPool::map_resumable)。
//!
//! 不破坏单浏览器 API:池是 [`Browser`]/[`Tab`] 之上的编排层。设计见 `docs/并发池.md`。

pub mod checkpoint;
pub mod fingerprint;
pub mod proxy_pool;
pub mod rotate;

pub use checkpoint::Checkpoint;
pub use fingerprint::{FingerprintPool, FingerprintProfile};
pub use proxy_pool::ProxyPool;
pub use rotate::RotateStrategy;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream;
use tokio::sync::{Mutex, Semaphore};

use crate::browser::{Browser, ContextOverride, Tab};
use crate::launcher::BrowserOptions;
use crate::{Error, Result};

use rotate::hash_key;

/// 失败重试策略(指数退避)。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 最大重试次数(总尝试 = `max_retries + 1`)。
    pub max_retries: u32,
    /// 首次重试前的等待。
    pub backoff: Duration,
    /// 每次重试后退避时长乘以该系数。
    pub backoff_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            backoff: Duration::from_millis(500),
            backoff_factor: 2.0,
        }
    }
}

impl RetryPolicy {
    /// 指定最大重试次数,其余取默认退避。
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }
    /// 不重试。
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }
    /// 设置首次退避时长。
    pub fn backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }
    /// 设置退避系数。
    pub fn backoff_factor(mut self, f: f64) -> Self {
        self.backoff_factor = f;
        self
    }
}

/// 并发池启动选项。
pub struct PoolOptions {
    /// worker(浏览器进程)数量。默认 4。
    pub size: usize,
    /// 每个 worker 内并发标签(context)数。默认 1(一个任务独占一个上下文,最强隔离)。
    pub tabs_per_worker: usize,
    /// 各 worker 的启动基线(可被 `worker_options` 逐个覆盖)。
    pub base_options: BrowserOptions,
    /// 可选:逐 worker 不同的启动选项(用于**深指纹**差异,如不同 `camou_config`)。
    /// 第 i 个 worker 取 `worker_options[i]`,越界则用 `base_options`。
    pub worker_options: Vec<BrowserOptions>,
    /// 可选:代理池(每任务轮换出口)。
    pub proxies: Option<ProxyPool>,
    /// 可选:轻量指纹池(每任务轮换 locale/时区/视口等)。
    pub fingerprints: Option<FingerprintPool>,
    /// 失败重试策略。
    pub retry: RetryPolicy,
    /// 任务结束后是否关闭该标签(默认 `true`,防泄漏 + 强隔离)。
    pub close_tab_after_task: bool,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            size: 4,
            tabs_per_worker: 1,
            base_options: BrowserOptions::default(),
            worker_options: Vec::new(),
            proxies: None,
            fingerprints: None,
            retry: RetryPolicy::default(),
            close_tab_after_task: true,
        }
    }
}

impl PoolOptions {
    pub fn new() -> Self {
        Self::default()
    }
    /// 设置 worker 数量。
    pub fn size(mut self, n: usize) -> Self {
        self.size = n;
        self
    }
    /// 设置每 worker 并发标签数。
    pub fn tabs_per_worker(mut self, n: usize) -> Self {
        self.tabs_per_worker = n;
        self
    }
    /// 设置各 worker 的启动基线。
    pub fn base_options(mut self, opts: BrowserOptions) -> Self {
        self.base_options = opts;
        self
    }
    /// 设置逐 worker 启动选项(深指纹差异)。
    pub fn worker_options(mut self, opts: Vec<BrowserOptions>) -> Self {
        self.worker_options = opts;
        self
    }
    /// 设置代理池。
    pub fn proxies(mut self, pool: ProxyPool) -> Self {
        self.proxies = Some(pool);
        self
    }
    /// 设置指纹池。
    pub fn fingerprints(mut self, pool: FingerprintPool) -> Self {
        self.fingerprints = Some(pool);
        self
    }
    /// 设置重试策略。
    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }
    /// 设置任务后是否关标签。
    pub fn close_tab_after_task(mut self, yes: bool) -> Self {
        self.close_tab_after_task = yes;
        self
    }
}

/// 池内一个浏览器 worker:持浏览器进程 + 健康标志,支持惰性重建(自愈)。
struct Worker {
    browser: Mutex<Arc<Browser>>,
    /// 该 worker 的启动选项(重建时复用)。
    options: BrowserOptions,
    /// 是否健康;连接断/进程退后置 false,下次取用时重建。
    healthy: AtomicBool,
}

impl Worker {
    /// 取一个可用的浏览器句柄;若已标记不健康则先重建(自愈)。
    async fn handle(&self) -> Result<Arc<Browser>> {
        if self.healthy.load(Ordering::Acquire) {
            return Ok(self.browser.lock().await.clone());
        }
        // 重建:加锁后二次确认(避免并发重复重建)。
        let mut guard = self.browser.lock().await;
        if !self.healthy.load(Ordering::Acquire) {
            tracing::warn!("worker 不健康,重建浏览器进程");
            let fresh = Browser::launch(self.options.clone()).await?;
            *guard = Arc::new(fresh);
            self.healthy.store(true, Ordering::Release);
        }
        Ok(guard.clone())
    }

    fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::Release);
    }
}

/// 浏览器 worker 池。`launch` 创建,`run`/`map`/`map_resumable` 派发任务,`shutdown` 关闭。
pub struct BrowserPool {
    workers: Vec<Arc<Worker>>,
    sem: Arc<Semaphore>,
    worker_cursor: AtomicU64,
    concurrency: usize,
    proxies: Option<ProxyPool>,
    fingerprints: Option<FingerprintPool>,
    retry: RetryPolicy,
    close_tab_after_task: bool,
}

impl BrowserPool {
    /// 启动一个并发池:并行拉起 `size` 个浏览器 worker。
    pub async fn launch(opts: PoolOptions) -> Result<Self> {
        let PoolOptions {
            size,
            tabs_per_worker,
            base_options,
            worker_options,
            proxies,
            fingerprints,
            retry,
            close_tab_after_task,
        } = opts;

        let size = size.max(1);
        let tabs_per_worker = tabs_per_worker.max(1);

        // 逐 worker 选定启动选项。
        let worker_opts: Vec<BrowserOptions> = (0..size)
            .map(|i| worker_options.get(i).cloned().unwrap_or_else(|| base_options.clone()))
            .collect();

        // 并行启动;任一失败则整体失败(已起的浏览器经 Drop 兜底清理)。
        let workers = futures_util::future::try_join_all(worker_opts.into_iter().map(|o| async move {
            let browser = Browser::launch(o.clone()).await?;
            Ok::<Arc<Worker>, Error>(Arc::new(Worker {
                browser: Mutex::new(Arc::new(browser)),
                options: o,
                healthy: AtomicBool::new(true),
            }))
        }))
        .await?;

        let concurrency = size * tabs_per_worker;
        Ok(Self {
            workers,
            sem: Arc::new(Semaphore::new(concurrency)),
            worker_cursor: AtomicU64::new(0),
            concurrency,
            proxies,
            fingerprints,
            retry,
            close_tab_after_task,
        })
    }

    /// 总并发槽数(= worker 数 × 每 worker 标签数)。
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// worker(浏览器进程)数量。
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// 取下一个 worker(轮询;带 key 时按 key 粘性)。
    fn pick_worker(&self, key: Option<&str>) -> Arc<Worker> {
        let n = self.workers.len() as u64;
        let idx = match key {
            Some(k) => hash_key(k) % n,
            None => self.worker_cursor.fetch_add(1, Ordering::Relaxed) % n,
        };
        self.workers[idx as usize].clone()
    }

    /// 组装一次任务用的 per-context 覆盖(代理 + 指纹)。
    fn build_override(&self, key: Option<&str>) -> ContextOverride {
        let mut ov = ContextOverride::new();
        if let Some(pp) = &self.proxies {
            let p = match key {
                Some(k) => pp.for_key(k),
                None => pp.next(),
            };
            if let Some(p) = p {
                ov = ov.proxy(p);
            }
        }
        if let Some(fp) = &self.fingerprints {
            let prof = match key {
                Some(k) => fp.for_key(k),
                None => fp.next(),
            };
            if let Some(prof) = prof {
                ov = prof.apply_to(ov);
            }
        }
        ov
    }

    /// 在池中跑一个任务:取并发许可 → 选 worker → 套代理/指纹开标签 → 跑 `task` → (默认)关标签。
    /// 失败按 [`RetryPolicy`] 重试。任务闭包须为 `Fn`(可能被重试多次调用)。
    pub async fn run<F, Fut, T>(&self, task: F) -> Result<T>
    where
        F: Fn(Tab) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.run_keyed(None, task).await
    }

    /// 同 [`run`](Self::run),但用 `key` 做 worker / 代理 / 指纹的粘性定位(策略为 Sticky 时同 key 同出口)。
    pub async fn run_keyed<F, Fut, T>(&self, key: Option<&str>, task: F) -> Result<T>
    where
        F: Fn(Tab) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let _permit = self
            .sem
            .acquire()
            .await
            .map_err(|_| Error::Other("并发池信号量已关闭".into()))?;

        let mut attempt = 0u32;
        let mut backoff = self.retry.backoff;
        loop {
            let worker = self.pick_worker(key);
            match self.try_once(&worker, key, &task).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if is_worker_dead(&e) {
                        worker.mark_unhealthy();
                    }
                    if attempt >= self.retry.max_retries {
                        return Err(e);
                    }
                    attempt += 1;
                    tracing::debug!(attempt, error = %e, "任务失败,重试");
                    if !backoff.is_zero() {
                        tokio::time::sleep(backoff).await;
                        backoff = Duration::from_secs_f64(
                            backoff.as_secs_f64() * self.retry.backoff_factor,
                        );
                    }
                }
            }
        }
    }

    /// 单次尝试:开标签 → 跑任务 → 关标签。
    async fn try_once<F, Fut, T>(
        &self,
        worker: &Arc<Worker>,
        key: Option<&str>,
        task: &F,
    ) -> Result<T>
    where
        F: Fn(Tab) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let browser = worker.handle().await?;
        let ov = self.build_override(key);
        let tab = browser.new_tab_with(&ov).await.inspect_err(|e| {
            if is_worker_dead(e) {
                worker.mark_unhealthy();
            }
        })?;
        let out = task(tab.clone()).await;
        if self.close_tab_after_task {
            let _ = tab.close().await;
        }
        out
    }

    /// 并发对 `items` 逐项跑任务,返回与输入**顺序一致**的 `(item, 结果)`。并发受 [`concurrency`](Self::concurrency) 限制。
    /// 任务闭包签名 `Fn(item, Tab) -> Future<Result<T>>`。
    pub async fn map<I, F, Fut, T>(&self, items: Vec<I>, task: F) -> Vec<(I, Result<T>)>
    where
        I: Clone,
        F: Fn(I, Tab) -> Fut + Clone,
        Fut: Future<Output = Result<T>>,
    {
        let n = items.len();
        let concurrency = self.concurrency.max(1);
        let mut collected: Vec<(usize, I, Result<T>)> = stream::iter(items.into_iter().enumerate())
            .map(|(i, item)| {
                let task = task.clone();
                async move {
                    let item2 = item.clone();
                    let r = self.run(move |tab| task(item2.clone(), tab)).await;
                    (i, item, r)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        // 按输入顺序还原。
        collected.sort_by_key(|(i, _, _)| *i);
        let mut out: Vec<(I, Result<T>)> = Vec::with_capacity(n);
        for (_, item, r) in collected {
            out.push((item, r));
        }
        out
    }

    /// 断点续抓:跳过 `ckpt` 中已完成的 key,只跑未完成项;每项成功即落盘标记完成。
    /// 返回**本次实际尝试**的项的结果(已完成的被跳过、不在结果里)。`key_of` 从 item 取唯一 key。
    pub async fn map_resumable<I, K, F, Fut, T>(
        &self,
        items: Vec<I>,
        key_of: K,
        ckpt: &Checkpoint,
        task: F,
    ) -> Vec<(I, Result<T>)>
    where
        I: Clone,
        K: Fn(&I) -> String,
        F: Fn(I, Tab) -> Fut + Clone,
        Fut: Future<Output = Result<T>>,
    {
        // 过滤掉已完成的。
        let mut pending: Vec<(String, I)> = Vec::new();
        for it in items {
            let k = key_of(&it);
            if !ckpt.is_done(&k).await {
                pending.push((k, it));
            }
        }
        let n = pending.len();
        let concurrency = self.concurrency.max(1);

        let mut collected: Vec<(usize, I, Result<T>)> =
            stream::iter(pending.into_iter().enumerate())
                .map(|(i, (k, item))| {
                    let task = task.clone();
                    async move {
                        let item2 = item.clone();
                        let r = self.run(move |tab| task(item2.clone(), tab)).await;
                        if r.is_ok() {
                            let _ = ckpt.mark_done(&k, None).await;
                        }
                        (i, item, r)
                    }
                })
                .buffer_unordered(concurrency)
                .collect()
                .await;

        collected.sort_by_key(|(i, _, _)| *i);
        let mut out: Vec<(I, Result<T>)> = Vec::with_capacity(n);
        for (_, item, r) in collected {
            out.push((item, r));
        }
        out
    }

    /// 关闭池:并行优雅退出所有 worker(`Browser::quit`),`Drop` 仍兜底。
    pub async fn shutdown(self) -> Result<()> {
        let mut futs = Vec::new();
        for w in &self.workers {
            let b = w.browser.lock().await.clone();
            futs.push(async move {
                let _ = b.quit().await;
            });
        }
        futures_util::future::join_all(futs).await;
        Ok(())
    }
}

/// 判断错误是否意味着"worker 进程/连接已死"(据此标记 worker 不健康并触发自愈)。
fn is_worker_dead(e: &Error) -> bool {
    matches!(e, Error::Transport(_))
}
