//! 标签级网络门面 [`NetworkHandle`]:`filter → method → listen | block | mock | rewrite | record`。
//!
//! 旧的 `tab.listen()` / `tab.intercept()` / `har_record` / `route_from_har` **全部保留**。
//! 本模块只是把过滤和拦截动作收成一条链,统一落到 [`NetRequest`] / [`NetResponse`]。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::Result;
use crate::cdp::core::CdpCore;
use crate::cdp::extras::{HarPlayer, HarRecorder, HarReplayOptions};
use crate::cdp::interceptor::CdpIntercept;
use crate::cdp::listener::CdpListen;
use crate::cdp::tab::ChromiumTab;
use crate::net::{ListenFilter, ResumeOptions};

/// `tab.network()` 返回的链式句柄。
#[derive(Clone)]
pub struct NetworkHandle {
    core: Arc<CdpCore>,
    filter: ListenFilter,
}

impl NetworkHandle {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self {
            core,
            filter: ListenFilter::default(),
        }
    }

    /// URL 子串过滤(可多次调用,OR)。
    pub fn filter(mut self, keyword: impl Into<String>) -> Self {
        self.filter.url_keywords.push(keyword.into());
        self
    }

    /// 仅匹配该 HTTP 方法(可多次,OR;`POST` / `GET` …,大小写不敏感)。
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.filter.methods.push(method.into().to_ascii_uppercase());
        self
    }

    /// 仅 XHR / fetch。
    pub fn xhr_only(self) -> Self {
        let mut s = self;
        s.filter.xhr_only = true;
        s
    }

    /// 开始监听,返回既有 [`CdpListen`](可 `wait` / `wait_count` / `stop`)。
    pub async fn listen(self) -> Result<CdpListen> {
        let h = CdpListen::new(self.core);
        h.start_filter(self.filter).await?;
        Ok(h)
    }

    /// 开始拦截,匹配项交调用方 `next()` 决策。
    pub async fn intercept(self) -> Result<CdpIntercept> {
        let h = CdpIntercept::new(self.core);
        h.start_filter(self.filter).await?;
        Ok(h)
    }

    /// 匹配的请求一律中止(`blockedbyclient`);不匹配自动放行。
    pub async fn block(self) -> Result<NetworkRoute> {
        self.route(RouteRule::Block).await
    }

    /// 匹配的请求用伪造响应满足(不打到真实服务器)。
    pub async fn mock(
        self,
        status: u16,
        headers: Vec<(String, String)>,
        body: impl Into<String>,
    ) -> Result<NetworkRoute> {
        self.route(RouteRule::Mock {
            status,
            headers,
            body: body.into(),
        })
        .await
    }

    /// 匹配的请求改写后放行。
    pub async fn rewrite(self, opts: ResumeOptions) -> Result<NetworkRoute> {
        self.route(RouteRule::Modify(opts)).await
    }

    /// HAR 录制(委托既有 `har_record`)。
    pub async fn record(self) -> Result<HarRecorder> {
        ChromiumTab::new(self.core).har_record().await
    }

    /// HAR 回放(委托既有 `route_from_har`)。**导航前调用**。
    pub async fn replay(
        self,
        path: impl AsRef<Path>,
        opts: &HarReplayOptions,
    ) -> Result<HarPlayer> {
        ChromiumTab::new(self.core).route_from_har(path, opts).await
    }

    async fn route(self, rule: RouteRule) -> Result<NetworkRoute> {
        let intercept = CdpIntercept::new(self.core);
        intercept.start_filter(self.filter).await?;
        let worker = intercept.clone();
        let task = tokio::spawn(async move {
            loop {
                match worker.next(Some(Duration::from_secs(30))).await {
                    Ok(Some(req)) => match &rule {
                        RouteRule::Allow => {
                            let _ = req.resume().await;
                        }
                        RouteRule::Block => {
                            let _ = req.abort("blockedbyclient").await;
                        }
                        RouteRule::Modify(opts) => {
                            let _ = req.resume_with(opts.clone()).await;
                        }
                        RouteRule::Mock {
                            status,
                            headers,
                            body,
                        } => {
                            let _ = req.fulfill(*status, headers.clone(), body).await;
                        }
                    },
                    Ok(None) => {
                        if !worker.is_intercepting().await {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(NetworkRoute {
            intercept,
            abort: task.abort_handle(),
        })
    }
}

#[derive(Clone)]
enum RouteRule {
    #[allow(dead_code)]
    Allow,
    Block,
    Modify(ResumeOptions),
    Mock {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
}

/// 自动拦截守卫:后台按规则处理匹配请求;`stop()` 撤销;`Drop` 中止后台任务。
pub struct NetworkRoute {
    intercept: CdpIntercept,
    abort: tokio::task::AbortHandle,
}

impl NetworkRoute {
    /// 停止自动拦截并关闭 Fetch 域。
    pub async fn stop(self) -> Result<()> {
        self.abort.abort();
        self.intercept.stop().await
    }

    pub async fn is_active(&self) -> bool {
        self.intercept.is_intercepting().await
    }
}

impl Drop for NetworkRoute {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[cfg(test)]
mod tests {
    use crate::net::ListenFilter;

    #[test]
    fn filter_builder_collects() {
        let mut h_filter = ListenFilter {
            url_keywords: vec!["/api/".into()],
            xhr_only: true,
            methods: vec!["POST".into()],
        };
        assert!(h_filter.matches_request("https://x.com/api/v", "xhr", "POST"));
        assert!(!h_filter.matches_request("https://x.com/api/v", "xhr", "GET"));
        h_filter.methods.clear();
        assert!(h_filter.matches_request("https://x.com/api/v", "fetch", "GET"));
    }
}
