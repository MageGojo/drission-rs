# Agent 运行时

LLM 不直接写 `querySelector`。中间隔一层语义动作，再按固定顺序落到页面：

```
LLM
 → Semantic Action（「点登录」「等 Dashboard」）
 → Locator Resolver
 → Accessibility → DOM / 文本 →（可选）截图/OCR
 → CDP
```

`observe()` 只给角色 + 可见名 + ref，不喂整页 HTML。

现有 `tab.ele` / `tab.wait()` / `tab.screenshot()` 都不动。新入口：

```rust
let page = tab.agent();
page.observe().await?;
page.find("登录按钮").click().await?;
page.type_text("用户名", "xxx").await?;
page.wait_for("Dashboard").await?;
```

定位统一走 `Locator`：`role` / `name` / `text` / `css` / `xpath` / `id` / `label` / `placeholder`。

Resolver 顺序：

```
Accessibility / role+name
  → DOM / 最内层文本
  → 截图空间（可见字旁边的可点区域）
  → OCR（`--features ocr`，无字按钮/图）
  → CDP 点击
```

等待：

```rust
tab.wait().element("#x").visible().timeout(Duration::from_secs(10)).await?;
tab.wait().network().idle().timeout(Duration::from_secs(10)).await?;
tab.wait().popup().timeout(Duration::from_secs(5)).await?; // Option<Tab>
tab.wait().download().timeout(Duration::from_secs(15)).await?;
```

不要 `sleep`。

媒体走 `tab.capture()`。

池：

```
Task → Scheduler → ProxyPool → Profile → BrowserPool → ContextPool → TabPool
```

`ChromiumPool::acquire_context` / `acquire_tab`；超内存/CPU 标 worker 不健康并重建；空闲 Tab 超时回收。
