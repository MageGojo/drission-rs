//! CDP 后端的**高并发浏览器池** [`ChromiumPool`](对齐 camoufox [`BrowserPool`](crate::pool::BrowserPool))。
//!
//! 管理一组 Chrome worker(进程),提供:
//! - **并发上限**:总并发槽 = `size * tabs_per_worker`,信号量限流。
//! - **每任务轮换出口/指纹**:从 `proxies` / `user_agents` 轮换,经 **CDP 原生 per-context 代理**
//!   ([`Target.createBrowserContext`])拼成 [`ChromiumContextOverride`] —— 每任务一个独立上下文,
//!   出口/Cookie 互不串台。
//! - **失败重试**([`RetryPolicy`](crate::pool::RetryPolicy),指数退避,可换 worker)。
//! - **健康自愈**:worker 连接断/进程退时惰性重建。
//! - **断点续抓**:配合 [`Checkpoint`](crate::pool::Checkpoint) 的 [`map_resumable`](ChromiumPool::map_resumable)。
//!
//! 后端无关的 `RetryPolicy`/`RotateStrategy`/`Checkpoint` 与 camoufox 共用(见 [`crate::pool`])。

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::cdp::{
    ChromiumBrowser, ChromiumBrowserContext, ChromiumContextOverride, ChromiumOptions, ChromiumTab,
    ContextSpec,
};
use crate::pool::ProxyPool;
use crate::pool::rotate::{RotateStrategy, Rotator, hash_key};
use crate::pool::{Checkpoint, RetryPolicy, is_worker_dead};
use crate::{Error, Result};

/// [`ChromiumPool`] 启动选项。
pub struct ChromiumPoolOptions {
    /// worker(Chrome 进程)数量。默认 4。
    pub size: usize,
    /// 每个 worker 内并发标签(context)数。默认 1(一个任务独占一个上下文,最强隔离)。
    pub tabs_per_worker: usize,
    /// 各 worker 的启动基线(可被 `worker_options` 逐个覆盖)。
    pub base_options: ChromiumOptions,
    /// 可选:逐 worker 不同的启动选项;第 i 个 worker 取 `worker_options[i]`,越界用 `base_options`。
    pub worker_options: Vec<ChromiumOptions>,
    /// 出口代理池(每任务轮换;`http://host:port` / `socks5://host:1080`)。空 = 不换代理。
    /// 简单场景用这个;要**健康剔除 + 出口地理自洽**(时区/语言随 IP)用 [`proxy_pool`](Self::proxy_pool)。
    pub proxies: Vec<String>,
    /// 可选:带**健康检查 + 出口地理探测**的代理池([`ProxyPool`])。设置后每任务经
    /// [`next_coherent_cdp`](ProxyPool::next_coherent_cdp) 取一个**健康**代理并附带**与其出口地理自洽**的
    /// 时区 / 语言覆盖(坏代理自动跳过),优先级高于 [`proxies`](Self::proxies)。用前建议先 `check_health().await`。
    pub proxy_pool: Option<ProxyPool>,
    /// UA 池(每任务轮换;经会话级 `Emulation` 覆盖)。空 = 不换 UA。
    pub user_agents: Vec<String>,
    /// 代理 / UA / worker 的轮换策略(RoundRobin / Random / Sticky)。
    pub rotate: RotateStrategy,
    /// 失败重试策略。
    pub retry: RetryPolicy,
    /// 任务结束后是否关闭该标签(默认 `true`,防泄漏 + 强隔离)。
    pub close_tab_after_task: bool,
    /// Chrome 进程 RSS 上限(MB)。超过则标 worker 不健康,下次重建。`None` = 不限。
    pub memory_limit_mb: Option<u64>,
    /// Chrome 进程 CPU 上限(百分比,`ps` 的 `%cpu`)。超过同样标不健康。`None` = 不限。
    pub cpu_limit_pct: Option<f32>,
    /// 空闲 Tab 回收间隔。`ZERO` = 不按时间回收。
    pub idle_recycle: Duration,
    /// 每个 Browser 上命名 Context 建议上限(回收时作线索,同名仍复用)。
    pub max_contexts_per_browser: usize,
    /// Tab 池空闲条数上限。
    pub max_tabs_per_context: usize,
}

impl Default for ChromiumPoolOptions {
    fn default() -> Self {
        Self {
            size: 4,
            tabs_per_worker: 1,
            base_options: ChromiumOptions::default(),
            worker_options: Vec::new(),
            proxies: Vec::new(),
            proxy_pool: None,
            user_agents: Vec::new(),
            rotate: RotateStrategy::default(),
            retry: RetryPolicy::default(),
            close_tab_after_task: true,
            memory_limit_mb: None,
            cpu_limit_pct: None,
            idle_recycle: Duration::from_secs(300),
            max_contexts_per_browser: 8,
            max_tabs_per_context: 4,
        }
    }
}

impl ChromiumPoolOptions {
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
    pub fn base_options(mut self, opts: ChromiumOptions) -> Self {
        self.base_options = opts;
        self
    }
    /// 设置逐 worker 启动选项。
    pub fn worker_options(mut self, opts: Vec<ChromiumOptions>) -> Self {
        self.worker_options = opts;
        self
    }
    /// 设置出口代理池(每任务轮换)。
    pub fn proxies(mut self, proxies: Vec<String>) -> Self {
        self.proxies = proxies;
        self
    }
    /// 设置带健康检查 + 出口地理自洽的 [`ProxyPool`](优先级高于 [`proxies`](Self::proxies))。
    pub fn proxy_pool(mut self, pool: ProxyPool) -> Self {
        self.proxy_pool = Some(pool);
        self
    }
    /// 设置 UA 池(每任务轮换)。
    pub fn user_agents(mut self, uas: Vec<String>) -> Self {
        self.user_agents = uas;
        self
    }
    /// 设置轮换策略。
    pub fn rotate(mut self, s: RotateStrategy) -> Self {
        self.rotate = s;
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
    pub fn memory_limit_mb(mut self, mb: u64) -> Self {
        self.memory_limit_mb = Some(mb);
        self
    }
    pub fn cpu_limit_pct(mut self, pct: f32) -> Self {
        self.cpu_limit_pct = Some(pct);
        self
    }
    pub fn idle_recycle(mut self, d: Duration) -> Self {
        self.idle_recycle = d;
        self
    }
    pub fn max_contexts_per_browser(mut self, n: usize) -> Self {
        self.max_contexts_per_browser = n.max(1);
        self
    }
    pub fn max_tabs_per_context(mut self, n: usize) -> Self {
        self.max_tabs_per_context = n.max(1);
        self
    }
}

/// 池内一个 Chrome worker:持浏览器进程 + 健康标志,支持惰性重建(自愈)。
struct CdpWorker {
    browser: Mutex<Arc<ChromiumBrowser>>,
    options: ChromiumOptions,
    healthy: AtomicBool,
}

impl CdpWorker {
    async fn handle(&self) -> Result<Arc<ChromiumBrowser>> {
        if self.healthy.load(Ordering::Acquire) {
            return Ok(self.browser.lock().await.clone());
        }
        let mut guard = self.browser.lock().await;
        if !self.healthy.load(Ordering::Acquire) {
            tracing::warn!("CDP worker 不健康,重建 Chrome 进程");
            let fresh = ChromiumBrowser::launch(self.options.clone()).await?;
            *guard = Arc::new(fresh);
            self.healthy.store(true, Ordering::Release);
        }
        Ok(guard.clone())
    }

    fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::Release);
    }
}

/// CDP 浏览器 worker 池。`launch` 创建,`run`/`map`/`map_resumable` 派发任务,`shutdown` 关闭。
pub struct ChromiumPool {
    workers: Vec<Arc<CdpWorker>>,
    sem: Arc<Semaphore>,
    worker_cursor: AtomicU64,
    concurrency: usize,
    proxies: Vec<String>,
    proxy_pool: Option<ProxyPool>,
    user_agents: Vec<String>,
    proxy_rotator: Rotator,
    ua_rotator: Rotator,
    retry: RetryPolicy,
    close_tab_after_task: bool,
    memory_limit_mb: Option<u64>,
    cpu_limit_pct: Option<f32>,
    idle_recycle: Duration,
    max_contexts_per_browser: usize,
    max_tabs_per_context: usize,
    idle_tabs: Arc<Mutex<Vec<(std::time::Instant, ChromiumTab)>>>,
}

impl ChromiumPool {
    /// 启动一个并发池:并行拉起 `size` 个 Chrome worker。
    pub async fn launch(opts: ChromiumPoolOptions) -> Result<Self> {
        let ChromiumPoolOptions {
            size,
            tabs_per_worker,
            base_options,
            worker_options,
            proxies,
            proxy_pool,
            user_agents,
            rotate,
            retry,
            close_tab_after_task,
            memory_limit_mb,
            cpu_limit_pct,
            idle_recycle,
            max_contexts_per_browser,
            max_tabs_per_context,
        } = opts;

        let size = size.max(1);
        let tabs_per_worker = tabs_per_worker.max(1);

        let worker_opts: Vec<ChromiumOptions> = (0..size)
            .map(|i| {
                worker_options
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| base_options.clone())
            })
            .collect();

        // 并行启动;任一失败则整体失败(已起的浏览器经 Drop 兜底清理)。
        let workers =
            futures_util::future::try_join_all(worker_opts.into_iter().map(|o| async move {
                let browser = ChromiumBrowser::launch(o.clone()).await?;
                Ok::<Arc<CdpWorker>, Error>(Arc::new(CdpWorker {
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
            proxy_pool,
            user_agents,
            proxy_rotator: Rotator::new(rotate),
            ua_rotator: Rotator::new(rotate),
            retry,
            close_tab_after_task,
            memory_limit_mb,
            cpu_limit_pct,
            idle_recycle,
            max_contexts_per_browser,
            max_tabs_per_context,
            idle_tabs: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// 总并发槽数(= worker 数 × 每 worker 标签数)。
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// worker(Chrome 进程)数量。
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// 取下一个 worker(轮询;带 key 时按 key 粘性)。
    fn pick_worker(&self, key: Option<&str>) -> Arc<CdpWorker> {
        let n = self.workers.len() as u64;
        let idx = match key {
            Some(k) => hash_key(k) % n,
            None => self.worker_cursor.fetch_add(1, Ordering::Relaxed) % n,
        };
        self.workers[idx as usize].clone()
    }

    /// 组装一次任务用的上下文覆盖(代理 + UA 轮换)。有 [`ProxyPool`] 时优先用它:取**健康**代理 +
    /// **与出口地理自洽**的时区/语言;否则退回裸 `proxies` 轮换。UA 轮换叠加在最上。
    fn build_override(&self, key: Option<&str>) -> ChromiumContextOverride {
        let mut ov = if let Some(pp) = &self.proxy_pool {
            pp.next_coherent_cdp()
                .unwrap_or_else(ChromiumContextOverride::new)
        } else {
            let mut o = ChromiumContextOverride::new();
            if let Some(i) = self.proxy_rotator.pick(self.proxies.len(), key) {
                o = o.proxy(self.proxies[i].clone());
            }
            o
        };
        if let Some(i) = self.ua_rotator.pick(self.user_agents.len(), key) {
            ov = ov.user_agent(self.user_agents[i].clone());
        }
        ov
    }

    /// 在池中跑一个任务:取并发许可 → 选 worker → 套代理/UA 开标签 → 跑 `task` → (默认)关标签。
    /// 失败按 [`RetryPolicy`](crate::pool::RetryPolicy) 重试。任务闭包须为 `Fn`(可能被重试多次)。
    pub async fn run<F, Fut, T>(&self, task: F) -> Result<T>
    where
        F: Fn(ChromiumTab) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.run_keyed(None, task).await
    }

    /// 同 [`run`](Self::run),但用 `key` 做 worker / 代理 / UA 的粘性定位(策略为 Sticky 时同 key 同出口)。
    pub async fn run_keyed<F, Fut, T>(&self, key: Option<&str>, task: F) -> Result<T>
    where
        F: Fn(ChromiumTab) -> Fut,
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
                    tracing::debug!(attempt, error = %e, "CDP 任务失败,重试");
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
        worker: &Arc<CdpWorker>,
        key: Option<&str>,
        task: &F,
    ) -> Result<T>
    where
        F: Fn(ChromiumTab) -> Fut,
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
    pub async fn map<I, F, Fut, T>(&self, items: Vec<I>, task: F) -> Vec<(I, Result<T>)>
    where
        I: Clone,
        F: Fn(I, ChromiumTab) -> Fut + Clone,
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

        collected.sort_by_key(|(i, _, _)| *i);
        let mut out: Vec<(I, Result<T>)> = Vec::with_capacity(n);
        for (_, item, r) in collected {
            out.push((item, r));
        }
        out
    }

    /// 断点续抓:跳过 `ckpt` 中已完成的 key,只跑未完成项;每项成功即落盘标记完成。
    /// 返回**本次实际尝试**项的结果(已完成的被跳过)。`key_of` 从 item 取唯一 key。
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
        F: Fn(I, ChromiumTab) -> Fut + Clone,
        Fut: Future<Output = Result<T>>,
    {
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

    /// 代理池(若启动时配了 [`ProxyPool`])。
    pub fn proxies(&self) -> Option<&ProxyPool> {
        self.proxy_pool.as_ref()
    }

    /// 取/建命名 Context(Browser → Context)。
    pub async fn acquire_context(&self, name: &str) -> Result<ContextLease> {
        self.recycle().await?;
        let lease = self.acquire().await?;
        let ctx = lease.context(name).await?;
        let _ = self.max_contexts_per_browser;
        Ok(ContextLease { lease, ctx })
    }

    /// 从 Tab 池取一个标签;没有空闲的就新开。Drop 时回到空闲池(满则关掉)。
    pub async fn acquire_tab(&self) -> Result<TabLease> {
        self.recycle().await?;
        if let Some((_, tab)) = self.idle_tabs.lock().await.pop() {
            return Ok(TabLease::new(
                tab,
                None,
                self.idle_tabs.clone(),
                self.max_tabs_per_context,
            ));
        }
        let lease = self.acquire().await?;
        let tab = lease.new_tab().await?;
        Ok(TabLease::new(
            tab,
            Some(lease),
            self.idle_tabs.clone(),
            self.max_tabs_per_context,
        ))
    }

    /// 空闲 Tab 回收 + 超内存/CPU 的 worker 标不健康。
    pub async fn recycle(&self) -> Result<RecycleReport> {
        let mut report = RecycleReport::default();
        let now = std::time::Instant::now();
        let mut keep = Vec::new();
        for (t, tab) in self.idle_tabs.lock().await.drain(..) {
            let stale = !self.idle_recycle.is_zero() && now.duration_since(t) >= self.idle_recycle;
            if stale {
                let _ = tab.close().await;
                report.tabs_closed += 1;
            } else {
                keep.push((t, tab));
            }
        }
        *self.idle_tabs.lock().await = keep;

        for w in &self.workers {
            let browser = w.browser.lock().await.clone();
            let Some(pid) = browser.pid() else {
                continue;
            };
            let Some(st) = proc_stat(pid) else {
                continue;
            };
            let over_mem = self
                .memory_limit_mb
                .map(|m| st.rss_bytes > m.saturating_mul(1024 * 1024))
                .unwrap_or(false);
            let over_cpu = self.cpu_limit_pct.map(|c| st.cpu_pct > c).unwrap_or(false);
            if over_mem || over_cpu {
                w.mark_unhealthy();
                report.workers_flagged += 1;
                if Arc::strong_count(&browser) == 1 {
                    let _ = browser.quit().await;
                    report.workers_restarted += 1;
                }
            }
        }
        Ok(report)
    }

    /// 各 worker 当前 RSS / CPU(读不到的跳过)。
    pub async fn resource_stats(&self) -> Vec<WorkerResource> {
        let mut out = Vec::new();
        for (i, w) in self.workers.iter().enumerate() {
            let browser = w.browser.lock().await.clone();
            let Some(pid) = browser.pid() else {
                continue;
            };
            if let Some(st) = proc_stat(pid) {
                out.push(WorkerResource {
                    worker: i,
                    pid,
                    rss_bytes: st.rss_bytes,
                    cpu_pct: st.cpu_pct,
                    healthy: w.healthy.load(Ordering::Acquire),
                });
            }
        }
        out
    }

    /// 领取一个并发槽 + 一个 worker 浏览器。Drop 释放槽。
    ///
    /// ```ignore
    /// let browser = pool.acquire().await?;
    /// let context = browser.context("account_001").await?;
    /// let tab = context.new_tab(None).await?;
    /// ```
    pub async fn acquire(&self) -> Result<PoolLease> {
        self.acquire_keyed(None).await
    }

    /// 同 [`acquire`](Self::acquire),`key` 做 worker / 代理 / UA 粘性定位。
    pub async fn acquire_keyed(&self, key: Option<&str>) -> Result<PoolLease> {
        let permit = self
            .sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Other("并发池信号量已关闭".into()))?;
        let worker = self.pick_worker(key);
        let browser = worker.handle().await?;
        Ok(PoolLease {
            browser,
            _permit: permit,
            override_: self.build_override(key),
        })
    }

    /// 关闭池:并行优雅退出所有 worker(`ChromiumBrowser::quit`),`Drop` 仍兜底。
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

/// [`ChromiumPool::acquire`] 拿到的租约:持并发许可,Deref 到 [`ChromiumBrowser`]。
pub struct PoolLease {
    browser: Arc<ChromiumBrowser>,
    _permit: OwnedSemaphorePermit,
    override_: ChromiumContextOverride,
}

impl PoolLease {
    pub fn browser(&self) -> &ChromiumBrowser {
        &self.browser
    }

    /// 在本 worker 上取/建命名 Context。
    pub async fn context(&self, name: &str) -> Result<ChromiumBrowserContext> {
        self.browser.context(name).await
    }

    pub async fn context_with(&self, spec: ContextSpec) -> Result<ChromiumBrowserContext> {
        self.browser.context_with(spec).await
    }

    /// 按池轮换覆盖开一个隔离标签(对齐 `run` 的 per-task context)。
    pub async fn new_tab(&self) -> Result<ChromiumTab> {
        self.browser.new_tab_with(&self.override_).await
    }
}

impl std::ops::Deref for PoolLease {
    type Target = ChromiumBrowser;
    fn deref(&self) -> &ChromiumBrowser {
        &self.browser
    }
}

/// `acquire_context` 租约:持并发槽 + 命名 Context。
pub struct ContextLease {
    lease: PoolLease,
    ctx: ChromiumBrowserContext,
}

impl ContextLease {
    pub fn browser(&self) -> &ChromiumBrowser {
        self.lease.browser()
    }
    pub fn context(&self) -> &ChromiumBrowserContext {
        &self.ctx
    }
    pub async fn new_tab(&self, url: Option<&str>) -> Result<ChromiumTab> {
        self.ctx.new_tab(url).await
    }
}

impl std::ops::Deref for ContextLease {
    type Target = ChromiumBrowserContext;
    fn deref(&self) -> &ChromiumBrowserContext {
        &self.ctx
    }
}

/// `acquire_tab` 租约:Drop 时把 Tab 放回空闲池。
pub struct TabLease {
    tab: ChromiumTab,
    _lease: Option<PoolLease>,
    idle: Arc<Mutex<Vec<(std::time::Instant, ChromiumTab)>>>,
    max_idle: usize,
    return_idle: bool,
}

impl TabLease {
    fn new(
        tab: ChromiumTab,
        lease: Option<PoolLease>,
        idle: Arc<Mutex<Vec<(std::time::Instant, ChromiumTab)>>>,
        max_idle: usize,
    ) -> Self {
        Self {
            tab,
            _lease: lease,
            idle,
            max_idle,
            return_idle: true,
        }
    }

    pub fn tab(&self) -> &ChromiumTab {
        &self.tab
    }

    /// 取出 Tab,不再回池。
    pub fn into_inner(mut self) -> ChromiumTab {
        self.return_idle = false;
        self.tab.clone()
    }
}

impl std::ops::Deref for TabLease {
    type Target = ChromiumTab;
    fn deref(&self) -> &ChromiumTab {
        &self.tab
    }
}

impl Drop for TabLease {
    fn drop(&mut self) {
        if !self.return_idle {
            return;
        }
        let tab = self.tab.clone();
        let idle = self.idle.clone();
        let max = self.max_idle;
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let mut g = idle.lock().await;
                if g.len() < max {
                    g.push((std::time::Instant::now(), tab));
                } else {
                    drop(g);
                    let _ = tab.close().await;
                }
            });
        }
    }
}

/// [`ChromiumPool::recycle`] 结果。
#[derive(Debug, Clone, Default)]
pub struct RecycleReport {
    pub tabs_closed: usize,
    pub workers_flagged: usize,
    pub workers_restarted: usize,
}

/// 一个 worker 的进程资源。
#[derive(Debug, Clone)]
pub struct WorkerResource {
    pub worker: usize,
    pub pid: u32,
    pub rss_bytes: u64,
    pub cpu_pct: f32,
    pub healthy: bool,
}

/// 读 Chrome 进程 RSS / CPU。Unix 走 `ps`;读不到返回 `None`。
pub fn proc_stat(pid: u32) -> Option<ProcStat> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=,pcpu=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        let mut it = s.split_whitespace();
        let rss_kb: u64 = it.next()?.parse().ok()?;
        let cpu_pct: f32 = it.next()?.parse().ok()?;
        Some(ProcStat {
            rss_bytes: rss_kb.saturating_mul(1024),
            cpu_pct,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcStat {
    pub rss_bytes: u64,
    pub cpu_pct: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_self() {
        let st = proc_stat(std::process::id());
        #[cfg(unix)]
        {
            let st = st.expect("ps 应能读本进程");
            assert!(st.rss_bytes > 0);
        }
        #[cfg(not(unix))]
        {
            let _ = st;
        }
    }
}
