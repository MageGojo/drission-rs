//! 元素 [`Element`]:对应 DrissionPage 的元素对象。
//!
//! 元素由其在某个执行上下文里的 `objectId` 标识。读写文本/属性走 `Runtime.callFunction`;
//! 点击走 `Page.getContentQuads` 取中心点 + `Page.dispatchMouseEvent`;输入走 `Page.insertText`。
//! 注意:导航会使旧 `objectId` 失效(此时操作会报协议错误,需要重新 `ele` 查找)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::{Instant, sleep};

use crate::browser::frame::Frame;
use crate::browser::keys::KeyInput;
use crate::browser::shadow::ShadowRoot;
use crate::browser::static_element::StaticElement;
use crate::browser::tab::{ImageFormat, TabCore, write_file};
use crate::locator::{self, Query};
use crate::{Error, Result};

/// 一个页面元素句柄。
#[derive(Clone)]
pub struct Element {
    core: Arc<TabCore>,
    object_id: String,
    /// 元素所属 frame;`None` 表示主帧(用主帧的执行上下文)。iframe 内元素带子帧 id。
    frame_id: Option<String>,
}

impl Element {
    pub(crate) fn new(core: Arc<TabCore>, object_id: String) -> Self {
        Self {
            core,
            object_id,
            frame_id: None,
        }
    }

    /// 构造一个**归属于某个 frame**(如 iframe)的元素。
    pub(crate) fn new_in_frame(core: Arc<TabCore>, object_id: String, frame_id: String) -> Self {
        Self {
            core,
            object_id,
            frame_id: Some(frame_id),
        }
    }

    /// 底层 objectId(调试用)。
    pub fn object_id(&self) -> &str {
        &self.object_id
    }

    /// 该元素所属 frameId(主帧或其所在 iframe)。
    pub(crate) fn frame_id_ref(&self) -> &str {
        self.frame_id.as_deref().unwrap_or(&self.core.main_frame_id)
    }

    fn self_arg(&self) -> Value {
        json!({ "objectId": self.object_id })
    }

    /// 在该元素上调用一个函数声明(第一个参数为本元素节点)。在元素所属 frame 的上下文里执行。
    async fn call(&self, declaration: &str, extra: Vec<Value>, by_value: bool) -> Result<Value> {
        let mut args = vec![self.self_arg()];
        args.extend(extra);
        match &self.frame_id {
            Some(fid) => {
                self.core
                    .call_function_in(fid, declaration, args, by_value)
                    .await
            }
            None => self.core.call_function(declaration, args, by_value).await,
        }
    }

    /// 在本元素上调用返回**单个节点**的函数,构造归属同一 frame 的 [`Element`];
    /// 函数返回 `null`(无此节点)时返回 [`Error::ElementNotFound`]`(what)`。
    async fn call_to_element(
        &self,
        declaration: &str,
        extra: Vec<Value>,
        what: &str,
    ) -> Result<Element> {
        let result = self.call(declaration, extra, false).await?;
        match result.get("objectId").and_then(|v| v.as_str()) {
            Some(oid) => Ok(Element {
                core: self.core.clone(),
                object_id: oid.to_string(),
                frame_id: self.frame_id.clone(),
            }),
            None => Err(Error::ElementNotFound(what.to_string())),
        }
    }

    /// 在本元素上调用返回**节点数组**的函数,展开为归属同一 frame 的 [`Element`] 列表。
    async fn call_to_elements(&self, declaration: &str, extra: Vec<Value>) -> Result<Vec<Element>> {
        let result = self.call(declaration, extra, false).await?;
        let Some(array_object_id) = result.get("objectId").and_then(|v| v.as_str()) else {
            return Ok(Vec::new());
        };
        let oids = self
            .core
            .node_array_object_ids(self.frame_id_ref(), array_object_id)
            .await?;
        Ok(oids
            .into_iter()
            .map(|oid| Element {
                core: self.core.clone(),
                object_id: oid,
                frame_id: self.frame_id.clone(),
            })
            .collect())
    }

    /// 元素可见文本(`innerText`)。
    pub async fn text(&self) -> Result<String> {
        let v = self
            .call(
                "node => node.innerText ?? node.textContent ?? ''",
                vec![],
                true,
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 读取属性;不存在返回 `None`。
    pub async fn attr(&self, name: &str) -> Result<Option<String>> {
        let v = self
            .call(
                "(node, name) => node.getAttribute(name)",
                vec![json!({ "value": name })],
                true,
            )
            .await?;
        Ok(v.as_str().map(str::to_string))
    }

    /// 表单元素的 `value`。
    pub async fn value(&self) -> Result<String> {
        let v = self.call("node => node.value ?? ''", vec![], true).await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 标签名(小写)。
    pub async fn tag(&self) -> Result<String> {
        let v = self
            .call(
                "node => node.tagName ? node.tagName.toLowerCase() : ''",
                vec![],
                true,
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 是否可见。
    pub async fn is_displayed(&self) -> Result<bool> {
        let v = self
            .call(
                "node => { const r = node.getClientRects(); const s = getComputedStyle(node); \
                 return r.length > 0 && s.visibility !== 'hidden' && s.display !== 'none'; }",
                vec![],
                true,
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 是否可用(表单元素未 `disabled`;非表单元素恒 `true`)。
    pub async fn is_enabled(&self) -> Result<bool> {
        let v = self.call("node => !node.disabled", vec![], true).await?;
        Ok(v.as_bool().unwrap_or(true))
    }

    /// 是否(部分)在视口内(`getBoundingClientRect` 与视口相交)。
    pub async fn is_in_viewport(&self) -> Result<bool> {
        let v = self
            .call(
                "node => { const r = node.getBoundingClientRect(); \
                 const w = innerWidth || document.documentElement.clientWidth; \
                 const h = innerHeight || document.documentElement.clientHeight; \
                 return r.bottom > 0 && r.right > 0 && r.top < h && r.left < w; }",
                vec![],
                true,
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 是否被其它元素遮挡(中心点的 `elementFromPoint` 既不是自己也不是自己的后代)。
    pub async fn is_covered(&self) -> Result<bool> {
        let v = self
            .call(
                "node => { const r = node.getBoundingClientRect(); \
                 if (r.width<=0||r.height<=0) return false; \
                 const x = r.left + r.width/2, y = r.top + r.height/2; \
                 const top = document.elementFromPoint(x, y); \
                 if (!top) return false; \
                 return !(top === node || node.contains(top) || top.contains(node)); }",
                vec![],
                true,
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 是否可点击(可见 + 可用 + 在视口 + 中心点未被其它元素遮挡)。
    pub async fn is_clickable(&self) -> Result<bool> {
        let v = self
            .call(
                "node => { const s = getComputedStyle(node); \
                 if (s.visibility==='hidden'||s.display==='none'||node.disabled) return false; \
                 const r = node.getBoundingClientRect(); \
                 if (r.width<=0||r.height<=0) return false; \
                 const w = innerWidth, h = innerHeight; \
                 if (r.bottom<=0||r.right<=0||r.top>=h||r.left>=w) return false; \
                 const x = Math.min(Math.max(r.left+r.width/2,0),w-1); \
                 const y = Math.min(Math.max(r.top+r.height/2,0),h-1); \
                 const top = document.elementFromPoint(x,y); \
                 return !!top && (top===node || node.contains(top)); }",
                vec![],
                true,
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 元素几何信息(页面坐标 `x/y` + 视口坐标 `viewport_x/y` + 尺寸)。
    pub async fn rect(&self) -> Result<ElementRect> {
        let v = self
            .call(
                "node => { const r = node.getBoundingClientRect(); return { \
                 x: r.left + scrollX, y: r.top + scrollY, vx: r.left, vy: r.top, \
                 w: r.width, h: r.height }; }",
                vec![],
                true,
            )
            .await?;
        let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        Ok(ElementRect {
            x: f("x"),
            y: f("y"),
            viewport_x: f("vx"),
            viewport_y: f("vy"),
            width: f("w"),
            height: f("h"),
        })
    }

    /// 元素左上角的**页面坐标** `(x, y)`(含滚动偏移)。
    pub async fn location(&self) -> Result<(f64, f64)> {
        let r = self.rect().await?;
        Ok((r.x, r.y))
    }

    /// 元素尺寸 `(width, height)`。
    pub async fn size(&self) -> Result<(f64, f64)> {
        let r = self.rect().await?;
        Ok((r.width, r.height))
    }

    /// 全部属性(`name → value`)。
    pub async fn attrs(&self) -> Result<HashMap<String, String>> {
        let v = self
            .call(
                "node => { const o = {}; for (const a of (node.attributes||[])) o[a.name] = a.value; return o; }",
                vec![],
                true,
            )
            .await?;
        let mut map = HashMap::new();
        if let Some(o) = v.as_object() {
            for (k, val) in o {
                if let Some(s) = val.as_str() {
                    map.insert(k.clone(), s.to_string());
                }
            }
        }
        Ok(map)
    }

    /// 计算样式某属性值(`getComputedStyle().getPropertyValue(name)`)。
    pub async fn style(&self, name: &str) -> Result<String> {
        let v = self
            .call(
                "(node, n) => getComputedStyle(node).getPropertyValue(n)",
                vec![json!({ "value": name })],
                true,
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 读取 JS 属性(如 `checked`/`value`/`href`;返回原始 JSON 值)。与 [`attr`](Self::attr)(HTML 特性)不同。
    pub async fn property(&self, name: &str) -> Result<Value> {
        self.call("(node, n) => node[n]", vec![json!({ "value": name })], true)
            .await
    }

    /// 从 DOM 移除本元素(对应 DP `ele.remove()`)。
    pub async fn remove(&self) -> Result<()> {
        self.call("node => node.remove()", vec![], false).await?;
        Ok(())
    }

    /// 元素级等待句柄(对应 DP `ele.wait`):等本元素 `displayed`/`hidden`/`deleted`/`clickable`。
    pub fn wait(&self) -> ElementWait {
        ElementWait { ele: self.clone() }
    }

    /// 在元素上执行 JS(函数体内 `node` 即本元素)。返回其值。
    ///
    /// 例:`el.run_js("return node.dataset.id;")`。元素以**参数** `node` 传入——Juggler 的
    /// `Runtime.callFunction` 不会把对象绑成 `this`,故请用 `node` 而非 `this`。
    pub async fn run_js(&self, body: &str) -> Result<Value> {
        let decl = format!("(node) => {{ {body} }}");
        self.call(&decl, vec![], true).await
    }

    /// 该元素的 outer HTML。
    pub async fn html(&self) -> Result<String> {
        let v = self
            .call("node => node.outerHTML ?? ''", vec![], true)
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 该元素的 inner HTML。
    pub async fn inner_html(&self) -> Result<String> {
        let v = self
            .call("node => node.innerHTML ?? ''", vec![], true)
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 元素截图,返回图片字节(默认 PNG)。截图前先把元素**滚到视口中央**(DP 默认 `scroll_to_center=true`)。
    ///
    /// 注:面向主帧元素;iframe 内元素的裁剪区不叠加 iframe 偏移(如需可对该 iframe 整体截图)。
    pub async fn screenshot_bytes(&self) -> Result<Vec<u8>> {
        let clip = self.shot_clip(true).await?;
        self.core.capture(clip, ImageFormat::Png, None).await
    }

    /// 元素截图并保存到 `path`(格式按后缀:`.jpg`/`.jpeg`→JPEG,其余 PNG)。返回写入路径。
    pub async fn get_screenshot(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let path = path.as_ref().to_path_buf();
        let format = ImageFormat::from_path(&path);
        let clip = self.shot_clip(true).await?;
        let bytes = self.core.capture(clip, format, None).await?;
        write_file(&path, &bytes).await?;
        Ok(path)
    }

    /// 计算元素截图裁剪区(页面坐标);`scroll_to_center` 为 `true` 时先滚到视口中央。
    async fn shot_clip(&self, scroll_to_center: bool) -> Result<Value> {
        if scroll_to_center {
            self.call(
                "node => { node.scrollIntoView({ block: 'center', inline: 'center' }); }",
                vec![],
                true,
            )
            .await?;
        } else {
            self.scroll_into_view().await?;
        }
        let r = self
            .call(
                "node => { const b = node.getBoundingClientRect(); \
                 return [b.left + window.scrollX, b.top + window.scrollY, b.width, b.height]; }",
                vec![],
                true,
            )
            .await?;
        let x = r.get(0).and_then(Value::as_f64).unwrap_or(0.0);
        let y = r.get(1).and_then(Value::as_f64).unwrap_or(0.0);
        let w = r.get(2).and_then(Value::as_f64).unwrap_or(0.0).max(1.0);
        let h = r.get(3).and_then(Value::as_f64).unwrap_or(0.0).max(1.0);
        Ok(json!({ "x": x, "y": y, "width": w, "height": h }))
    }

    /// 把本元素(应是 `<table>` 或含表格)抽成二维表(行 × 单元格文本)。离线解析其 outer HTML。
    pub async fn table(&self) -> Result<Vec<Vec<String>>> {
        StaticElement::parse(&self.html().await?)?.table()
    }

    /// 把本元素的表格抽成**记录列表**(首行作表头)。离线解析其 outer HTML。
    pub async fn table_records(&self) -> Result<Vec<HashMap<String, String>>> {
        StaticElement::parse(&self.html().await?)?.table_records()
    }

    /// 解析本元素 outer HTML,取第一个匹配的**静态元素**(DP `ele.s_ele`)。
    pub async fn s_ele(&self, selector: &str) -> Result<StaticElement> {
        let html = self.html().await?;
        StaticElement::parse(&html)?.ele(selector)
    }

    /// 解析本元素 outer HTML,取全部匹配的**静态元素**(DP `ele.s_eles`)。
    pub async fn s_eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        let html = self.html().await?;
        StaticElement::parse(&html)?.eles(selector)
    }

    /// 在本元素范围内查找子元素(DP 定位语法)。
    pub async fn ele(&self, selector: &str) -> Result<Element> {
        let query = locator::parse(selector);
        let (decl, arg) = match &query {
            Query::Css(sel) => (
                "(node, sel) => node.querySelector(sel)".to_string(),
                json!({ "value": sel }),
            ),
            Query::Xpath(xp) => (
                "(node, xp) => document.evaluate(xp, node, null, 9, null).singleNodeValue"
                    .to_string(),
                json!({ "value": xp }),
            ),
        };
        let result = self.call(&decl, vec![arg], false).await?;
        match result.get("objectId").and_then(|v| v.as_str()) {
            // 子元素与本元素同属一个 frame。
            Some(oid) => Ok(Element {
                core: self.core.clone(),
                object_id: oid.to_string(),
                frame_id: self.frame_id.clone(),
            }),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    // ---- 相对定位(对标 DrissionPage 的相对元素查找;均归属同一 frame)----

    /// 直接父元素(DP `ele.parent()`)。无父元素则 [`Error::ElementNotFound`]。
    pub async fn parent(&self) -> Result<Element> {
        self.call_to_element("node => node.parentElement", vec![], "parent")
            .await
    }

    /// 向上第 `level` 级祖先(`level=1` 即直接父元素;DP `ele.parent(level)`)。
    pub async fn parent_n(&self, level: usize) -> Result<Element> {
        self.call_to_element(
            "(node, n) => { let e = node; for (let i = 0; i < n && e; i++) e = e.parentElement; return e; }",
            vec![json!({ "value": level })],
            "parent_n",
        )
        .await
    }

    /// 最近的**匹配定位**的祖先(DP `ele.parent('tag:div')`)。
    ///
    /// 仅支持 CSS 系定位(`#`/`.`/`tag:`/`@attr`/`css:`);`xpath:` 祖先请改用实时
    /// [`Tab::ele`](crate::browser::Tab::ele)`("xpath:...")`(走浏览器原生 `document.evaluate`)。
    pub async fn parent_until(&self, selector: &str) -> Result<Element> {
        let css = match locator::parse(selector) {
            Query::Css(sel) => sel,
            Query::Xpath(_) => {
                return Err(Error::Other(
                    "parent_until 仅支持 CSS 系定位;xpath 祖先请用 tab.ele(\"xpath:...\")".into(),
                ));
            }
        };
        self.call_to_element(
            "(node, sel) => node.parentElement ? node.parentElement.closest(sel) : null",
            vec![json!({ "value": css })],
            selector,
        )
        .await
    }

    /// 所有直接子元素(DP `ele.children()`)。
    pub async fn children(&self) -> Result<Vec<Element>> {
        self.call_to_elements("node => Array.from(node.children)", vec![])
            .await
    }

    /// 第 `index` 个直接子元素(**0 基**,与本库其它索引一致,注意 DP 为 1 基)。越界则 [`Error::ElementNotFound`]。
    pub async fn child(&self, index: usize) -> Result<Element> {
        self.call_to_element(
            "(node, i) => node.children[i] || null",
            vec![json!({ "value": index })],
            "child",
        )
        .await
    }

    /// 下一个同级元素(DP `ele.next()`)。无则 [`Error::ElementNotFound`]。
    pub async fn next(&self) -> Result<Element> {
        self.call_to_element("node => node.nextElementSibling", vec![], "next")
            .await
    }

    /// 上一个同级元素(DP `ele.prev()`)。无则 [`Error::ElementNotFound`]。
    pub async fn prev(&self) -> Result<Element> {
        self.call_to_element("node => node.previousElementSibling", vec![], "prev")
            .await
    }

    /// 后面所有同级元素(按文档顺序;DP `ele.nexts()`)。
    pub async fn nexts(&self) -> Result<Vec<Element>> {
        self.call_to_elements(
            "node => { const a = []; let e = node.nextElementSibling; \
             while (e) { a.push(e); e = e.nextElementSibling; } return a; }",
            vec![],
        )
        .await
    }

    /// 前面所有同级元素(按文档顺序,从最靠前到本元素的前一个;DP `ele.prevs()`)。
    pub async fn prevs(&self) -> Result<Vec<Element>> {
        self.call_to_elements(
            "node => { const a = []; let e = node.previousElementSibling; \
             while (e) { a.unshift(e); e = e.previousElementSibling; } return a; }",
            vec![],
        )
        .await
    }

    /// 所有同级元素(不含自身,按文档顺序)。
    pub async fn siblings(&self) -> Result<Vec<Element>> {
        self.call_to_elements(
            "node => node.parentElement \
             ? Array.from(node.parentElement.children).filter(c => c !== node) : []",
            vec![],
        )
        .await
    }

    /// 若本元素挂着 **open** shadow root,返回其 [`ShadowRoot`](用于查 shadow 内的元素;DP `ele.shadow_root`)。
    ///
    /// 仅 `mode:'open'` 的 shadow 可被脚本访问(`closed` 的 `shadowRoot` 为 `null`,此时返回错误)。
    pub async fn shadow_root(&self) -> Result<ShadowRoot> {
        let result = self.call("node => node.shadowRoot", vec![], false).await?;
        match result.get("objectId").and_then(|v| v.as_str()) {
            Some(oid) => Ok(ShadowRoot::new(
                self.core.clone(),
                oid.to_string(),
                self.frame_id.clone(),
            )),
            None => Err(Error::Other("该元素没有 open shadow root".into())),
        }
    }

    /// 滚动到可见。
    pub async fn scroll_into_view(&self) -> Result<()> {
        self.core
            .send_page(
                "Page.scrollIntoViewIfNeeded",
                json!({ "frameId": self.frame_id_ref(), "objectId": self.object_id }),
            )
            .await?;
        Ok(())
    }

    /// 点击元素(取内容区中心点,派发 move/down/up 鼠标事件)。
    pub async fn click(&self) -> Result<()> {
        self.scroll_into_view().await?;
        let (x, y) = self.center_point().await?;
        let base = |ty: &str, buttons: i64, click_count: i64| {
            json!({
                "type": ty,
                "button": 0,
                "buttons": buttons,
                "x": x,
                "y": y,
                "modifiers": 0,
                "clickCount": click_count,
            })
        };
        self.core
            .send_page("Page.dispatchMouseEvent", base("mousemove", 0, 0))
            .await?;
        self.core
            .send_page("Page.dispatchMouseEvent", base("mousedown", 1, 1))
            .await?;
        self.core
            .send_page("Page.dispatchMouseEvent", base("mouseup", 1, 1))
            .await?;
        Ok(())
    }

    /// 把鼠标移到元素中心(hover,不点击)。
    pub async fn hover(&self) -> Result<()> {
        self.scroll_into_view().await?;
        let (x, y) = self.center_point().await?;
        self.core.dispatch_mouse("mousemove", x, y, 0).await
    }

    /// 人手轨迹拖拽:按住元素中心,沿 `(dx, dy)` 像素方向移动后释放(对应 DP `ele.drag`)。
    ///
    /// 轨迹"由快到慢 + 轻微过冲回拉 + 上下抖动",并在每步之间随机停顿——用于滑块验证码等
    /// 需要拟人轨迹的拖动。`duration` 为大致总时长(秒);`<=0` 则用默认较快节奏。
    pub async fn drag(&self, dx: f64, dy: f64, duration: f64) -> Result<()> {
        self.scroll_into_view().await?;
        let (x0, y0) = self.center_point().await?;
        self.drag_from(x0, y0, dx, dy, duration).await
    }

    /// 人手轨迹拖拽到视口绝对坐标 `(x, y)`(对应 DP `ele.drag_to`)。
    pub async fn drag_to(&self, x: f64, y: f64, duration: f64) -> Result<()> {
        self.scroll_into_view().await?;
        let (x0, y0) = self.center_point().await?;
        self.drag_from(x0, y0, x - x0, y - y0, duration).await
    }

    /// 从 `(x0,y0)` 起按住并按拟人轨迹移动 `(dx,dy)` 后释放。
    ///
    /// 中间的密集 `mousemove` 走**不等待往返**的 [`dispatch_mouse_fire`](crate::browser::tab::TabCore::dispatch_mouse_fire),
    /// 由 `sleep` 精确控制 ~10ms 级采样(真人 60~120Hz);仅 down/up 走会等待的路径保证边界正确。
    async fn drag_from(&self, x0: f64, y0: f64, dx: f64, dy: f64, duration: f64) -> Result<()> {
        let mut rng = Xorshift::new(seed_from_clock());
        let steps = human_drag_track(dx, dy, duration, rng.next_u64());

        // 起步:先把指针落到起点(未按),再按下——按下前后都有人手迟滞。
        self.core.dispatch_mouse("mousemove", x0, y0, 0).await?;
        sleep(Duration::from_millis(rng.range_ms(20, 60))).await;
        self.core.dispatch_mouse("mousedown", x0, y0, 1).await?;
        sleep(Duration::from_millis(rng.range_ms(70, 140))).await; // 起步前迟滞

        for (ox, oy, delay) in steps {
            // 密集移动不等返回值,把节奏交给 sleep(贴近真人采样率)。
            self.core
                .dispatch_mouse_fire("mousemove", x0 + ox, y0 + oy, 1)?;
            if delay > 0 {
                sleep(Duration::from_millis(delay)).await;
            }
        }
        sleep(Duration::from_millis(rng.range_ms(60, 130))).await; // 到位后停顿再松手
        self.core
            .dispatch_mouse("mouseup", x0 + dx, y0 + dy, 0)
            .await?;
        Ok(())
    }

    /// 聚焦元素。
    pub async fn focus(&self) -> Result<()> {
        self.call("node => node.focus()", vec![], false).await?;
        Ok(())
    }

    /// 在元素中输入文本(先聚焦,再一次性插入)。节奏不敏感的场景用它最快。
    pub async fn input(&self, text: &str) -> Result<()> {
        self.focus().await?;
        self.core
            .send_page("Page.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    /// **逐字符拟人输入**:聚焦后一个字符一个字符地敲(`keydown`+`insertText`+`keyup`),
    /// 字符间 30~140ms 随机停顿(真人打字节奏)——比 [`input`](Self::input) 一次性插入更像人,
    /// 适合对输入节奏敏感 / 监听 `keydown`/`keyup`(如自动补全、表单校验)的站点。
    ///
    /// `keydown`/`keyup` 的 `key` 取字符本身(对触发站点 JS 有用),即便个别字符(如 CJK)
    /// 的按键事件不被接受也无妨——真正的插入由 `Page.insertText` 完成,稳定可靠。
    pub async fn input_human(&self, text: &str) -> Result<()> {
        self.focus().await?;
        let mut rng = Xorshift::new(seed_from_clock());
        for ch in text.chars() {
            let s = ch.to_string();
            // keydown / keyup 仅用于触发站点监听;失败忽略(插入不依赖它)。
            let _ = self
                .core
                .send_page(
                    "Page.dispatchKeyEvent",
                    json!({ "type": "keydown", "key": s }),
                )
                .await;
            self.core
                .send_page("Page.insertText", json!({ "text": s }))
                .await?;
            let _ = self
                .core
                .send_page(
                    "Page.dispatchKeyEvent",
                    json!({ "type": "keyup", "key": s }),
                )
                .await;
            let delay = 30 + (rng.next_u64() % 110);
            sleep(Duration::from_millis(delay)).await;
        }
        Ok(())
    }

    /// 按**序列**输入(对应 DP `ele.input(['abc', Keys.ENTER])`):文本片段直接插入、特殊键派发按键。
    /// 先聚焦本元素。
    ///
    /// ```ignore
    /// ele.input_keys(&[KeyInput::text("hello"), KeyInput::key(Keys::ENTER)]).await?;
    /// ```
    pub async fn input_keys(&self, parts: &[KeyInput]) -> Result<()> {
        self.focus().await?;
        for p in parts {
            match p {
                KeyInput::Text(t) => {
                    self.core
                        .send_page("Page.insertText", json!({ "text": t }))
                        .await?;
                }
                KeyInput::Key(k) => {
                    self.core.press_key(k).await?;
                }
            }
        }
        Ok(())
    }

    /// 为 `<select>` 选择给定 `value` 的选项,并派发 input/change 事件。
    pub async fn select_value(&self, value: &str) -> Result<()> {
        self.call(
            "(node, val) => { node.value = val; \
             node.dispatchEvent(new Event('input', { bubbles: true })); \
             node.dispatchEvent(new Event('change', { bubbles: true })); }",
            vec![json!({ "value": value })],
            true,
        )
        .await?;
        Ok(())
    }

    /// 为 `<select>` 按可见文本选择选项(找到第一个 `option.text` 包含给定文本者)。
    pub async fn select_text(&self, text: &str) -> Result<()> {
        self.call(
            "(node, t) => { for (const o of node.options) { \
             if ((o.textContent || '').includes(t)) { node.value = o.value; \
             node.dispatchEvent(new Event('input', { bubbles: true })); \
             node.dispatchEvent(new Event('change', { bubbles: true })); return true; } } return false; }",
            vec![json!({ "value": text })],
            true,
        )
        .await?;
        Ok(())
    }

    /// 勾选 / 取消勾选 checkbox 或 radio;状态变化时派发 input/change。
    pub async fn set_checked(&self, checked: bool) -> Result<()> {
        self.call(
            "(node, c) => { if (node.checked !== c) { node.checked = c; \
             node.dispatchEvent(new Event('input', { bubbles: true })); \
             node.dispatchEvent(new Event('change', { bubbles: true })); } }",
            vec![json!({ "value": checked })],
            true,
        )
        .await?;
        Ok(())
    }

    /// 是否处于选中态(checkbox/radio)。
    pub async fn is_checked(&self) -> Result<bool> {
        let v = self.call("node => !!node.checked", vec![], true).await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 清空输入框(focus 后置空并派发 input/change 事件)。
    pub async fn clear(&self) -> Result<()> {
        self.call(
            "node => { node.focus(); if ('value' in node) { node.value = ''; \
             node.dispatchEvent(new Event('input', { bubbles: true })); \
             node.dispatchEvent(new Event('change', { bubbles: true })); } \
             else if (node.isContentEditable) { node.textContent = ''; } }",
            vec![],
            false,
        )
        .await?;
        Ok(())
    }

    /// 给 `<input type=file>` 设置要上传的文件(对应 DP 对文件输入框的 `input`)。
    ///
    /// `paths` 为本地文件**绝对路径**;多文件需该 input 带 `multiple`。
    pub async fn set_files(&self, paths: &[&str]) -> Result<()> {
        let files: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.core
            .send_page(
                "Page.setFileInputFiles",
                json!({
                    "frameId": self.frame_id_ref(),
                    "objectId": self.object_id,
                    "files": files,
                }),
            )
            .await?;
        Ok(())
    }

    /// 上传单个文件到 `<input type=file>`(便捷封装)。
    pub async fn upload(&self, path: &str) -> Result<()> {
        self.set_files(&[path]).await
    }

    /// 点击本元素并完成"自然上传"(对应 DP `ele.click.to_upload`)。
    ///
    /// 一步到位:武装文件选择器拦截 → 点击本元素(应是会弹出系统文件框的按钮)→ 等待文件被填入。
    /// 适用于**没有可直接定位的 `<input type=file>`**(或其隐藏、由 JS `input.click()` 唤起)的场景;
    /// 若你已经能拿到那个 `<input>`,直接用 [`set_files`](Self::set_files) 更简单。
    ///
    /// `paths` 为本地文件**绝对路径**;`timeout=None` 用默认超时。返回是否在超时内完成填入。
    pub async fn click_to_upload(&self, paths: &[&str], timeout: Option<Duration>) -> Result<bool> {
        if paths.is_empty() {
            return Err(Error::Other("click_to_upload: 文件列表为空".into()));
        }
        let files = paths.iter().map(|p| p.to_string()).collect();
        self.core.arm_upload(files).await?;
        self.click().await?;
        let d = timeout.unwrap_or_else(|| self.core.timeout());
        self.core.wait_upload(d).await
    }

    /// 若本元素是 `<iframe>`/`<frame>`,返回其内容 [`Frame`](用于查 iframe 内的元素)。
    pub async fn content_frame(&self) -> Result<Frame> {
        let r = self
            .core
            .send_page(
                "Page.describeNode",
                json!({ "frameId": self.frame_id_ref(), "objectId": self.object_id }),
            )
            .await?;
        let cfid = r["contentFrameId"]
            .as_str()
            .ok_or_else(|| Error::Other("该元素不是 iframe,或其内容帧尚不可用".into()))?;
        Ok(Frame::new(self.core.clone(), cfid.to_string()))
    }

    /// 取内容区第一个四边形的中心点(视口坐标)。供 [`Actions`](crate::browser::Actions) 等复用。
    pub(crate) async fn center_point(&self) -> Result<(f64, f64)> {
        let r = self
            .core
            .send_page(
                "Page.getContentQuads",
                json!({ "frameId": self.frame_id_ref(), "objectId": self.object_id }),
            )
            .await?;
        let quad = r["quads"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| Error::Other("元素无可见内容区(可能不可见)".into()))?;
        let mut sx = 0.0;
        let mut sy = 0.0;
        for p in ["p1", "p2", "p3", "p4"] {
            sx += quad[p]["x"].as_f64().unwrap_or(0.0);
            sy += quad[p]["y"].as_f64().unwrap_or(0.0);
        }
        Ok((sx / 4.0, sy / 4.0))
    }
}

/// 元素几何信息(由 [`Element::rect`] 返回)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementRect {
    /// 左上角页面坐标 X(含滚动偏移)。
    pub x: f64,
    /// 左上角页面坐标 Y(含滚动偏移)。
    pub y: f64,
    /// 左上角视口坐标 X(`getBoundingClientRect().left`)。
    pub viewport_x: f64,
    /// 左上角视口坐标 Y(`getBoundingClientRect().top`)。
    pub viewport_y: f64,
    /// 宽。
    pub width: f64,
    /// 高。
    pub height: f64,
}

/// `ele.wait()` 返回的元素级等待句柄(对应 DP `ele.wait`)。
///
/// 都基于本元素自身的 `objectId` 轮询;超时返回 `Ok(false)`(不报错)。`None` 超时用所属标签的默认超时。
pub struct ElementWait {
    ele: Element,
}

impl ElementWait {
    fn timeout_or_default(&self, t: Option<Duration>) -> Duration {
        t.unwrap_or_else(|| self.ele.core.timeout())
    }

    /// 等本元素变为**可见**。
    pub async fn displayed(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, || self.ele.is_displayed()).await
    }

    /// 等本元素变为**不可见**(隐藏)。
    pub async fn hidden(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, || async {
            Ok(!self.ele.is_displayed().await.unwrap_or(false))
        })
        .await
    }

    /// 等本元素从 DOM **移除**(`node.isConnected === false`)。
    pub async fn deleted(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, || async {
            // 节点被移除后 isConnected=false;objectId 失效(异常)也视为已删除。
            match self
                .ele
                .call("node => node.isConnected", vec![], true)
                .await
            {
                Ok(v) => Ok(!v.as_bool().unwrap_or(false)),
                Err(_) => Ok(true),
            }
        })
        .await
    }

    /// 等本元素变为**可点击**(可见 + 可用 + 在视口 + 未被遮挡)。
    pub async fn clickable(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, || self.ele.is_clickable()).await
    }

    /// 通用轮询:`check` 返回 `Ok(true)` 即成功;超时返回 `Ok(false)`。
    async fn poll<F, Fut>(&self, timeout: Option<Duration>, check: F) -> Result<bool>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<bool>>,
    {
        let deadline = Instant::now() + self.timeout_or_default(timeout);
        loop {
            if check().await.unwrap_or(false) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

/// 极简 xorshift64 伪随机(std-only,不引第三方),用于拖拽轨迹的抖动/停顿扰动。
struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Xorshift(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// `[0, 1)` 随机数。
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// `[lo, hi)` 区间随机整数毫秒。
    fn range_ms(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            lo
        } else {
            lo + self.next_u64() % (hi - lo)
        }
    }
}

/// 用系统时钟纳秒做种子。
fn seed_from_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

/// 生成一段拟人拖拽轨迹:返回 `(相对起点的 x 位移, y 位移, 该步后停顿毫秒)` 序列。
///
/// 相比线性/ease-out,更贴近真人手臂运动:
/// - **最小冲量(minimum-jerk)速度曲线** `10t³-15t⁴+6t⁵`(神经运动学里手够取目标的标准模型,
///   钟形速度:慢起→中段最快→慢收),而非数学上"太完美"的 ease 曲线;
/// - **时间驱动的密集采样**:点数按总时长 / ~13ms 计(真人 60~120Hz),节奏带 ±抖动(非等间隔);
/// - **末段过冲再回拉**到精确目标;
/// - 全程**手抖**(x/y 高频小噪声)+ 纵向缓慢漂移;
/// - 偶发**迟疑**(个别点延时变长,模拟人手中途的微停顿)。
///
/// `duration_secs<=0` 时按距离自估一个较快但拟人的总时长。最后一点精确落在 `(dx, dy)`。
fn human_drag_track(dx: f64, dy: f64, duration_secs: f64, seed: u64) -> Vec<(f64, f64, u64)> {
    let mut rng = Xorshift::new(seed);
    let dist = (dx * dx + dy * dy).sqrt();
    // 总时长:给定则用,否则按距离自估(慢起步 + 与距离正相关),夹在 0.35~1.6s。
    let dur_ms = if duration_secs > 0.0 {
        (duration_secs * 1000.0).round()
    } else {
        (dist * 3.5 + 320.0).clamp(350.0, 1600.0)
    };
    // 采样点数 = 时长 / ~13ms(真人采样率),夹在 24..=160。
    let n = ((dur_ms / 13.0).round() as usize).clamp(24, 160);
    let base = (dur_ms / n as f64).max(4.0); // 每点基础间隔
    // 过冲幅度:2%~6%(短距离不过冲)。
    let overshoot = if dist > 40.0 {
        1.0 + 0.02 + rng.unit() * 0.04
    } else {
        1.0
    };
    let fwd = ((n as f64) * 0.82) as usize;
    let back = n.saturating_sub(fwd).max(3);
    // 纵向漂移幅度(整段缓慢偏移几像素)与手抖幅度。
    let drift_y = (rng.unit() - 0.5) * 6.0;

    // minimum-jerk 归一化位移 s(t)。
    let mj = |t: f64| 10.0 * t.powi(3) - 15.0 * t.powi(4) + 6.0 * t.powi(5);
    // 每点延时:基础 ± 40% 抖动。
    let delay = |rng: &mut Xorshift| -> u64 {
        let jit = (rng.unit() - 0.5) * 0.8; // ±40%
        ((base * (1.0 + jit)).round() as u64).max(3)
    };

    let mut out = Vec::with_capacity(n + 2);
    // 前段:minimum-jerk 抵达过冲峰值。
    for i in 1..=fwd {
        let t = i as f64 / fwd as f64;
        let frac = overshoot * mj(t);
        let tremor_x = (rng.unit() - 0.5) * 1.0;
        let tremor_y = (rng.unit() - 0.5) * 1.6;
        out.push((
            dx * frac + tremor_x,
            dy * frac + drift_y * mj(t) + tremor_y,
            delay(&mut rng),
        ));
    }
    // 后段:从过冲峰值 minimum-jerk 回拉到精确目标(收得更慢)。
    for i in 1..=back {
        let t = i as f64 / back as f64;
        let frac = overshoot - (overshoot - 1.0) * mj(t);
        let tremor_x = (rng.unit() - 0.5) * 0.8;
        let tremor_y = (rng.unit() - 0.5) * 1.2;
        out.push((
            dx * frac + tremor_x,
            dy * frac + drift_y + tremor_y,
            delay(&mut rng) + 2,
        ));
    }
    // 收尾:精确落在 (dx, dy)。
    out.push((dx, dy, base.round() as u64));

    // 偶发迟疑:给前段 1~2 个点加一段额外停顿(模拟人手中途微停)。
    let pauses = 1 + (rng.unit() * 2.0) as usize;
    for _ in 0..pauses {
        if fwd > 4 {
            let idx = 2 + (rng.unit() * (fwd as f64 - 4.0)) as usize;
            if let Some(p) = out.get_mut(idx) {
                p.2 += rng.range_ms(25, 75);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::human_drag_track;

    #[test]
    fn drag_track_lands_exactly_on_target() {
        let track = human_drag_track(200.0, 0.0, 0.8, 12345);
        let last = *track.last().unwrap();
        assert_eq!((last.0, last.1), (200.0, 0.0));
        assert!(track.len() >= 24);
    }

    #[test]
    fn drag_track_overshoots_then_returns() {
        let track = human_drag_track(200.0, 0.0, 0.8, 999);
        let peak = track.iter().map(|p| p.0).fold(0.0_f64, f64::max);
        // 末段应回拉:峰值大于目标(含手抖容差),最终精确等于目标。
        assert!(peak > 200.0, "应有过冲,peak={peak}");
        assert_eq!(track.last().unwrap().0, 200.0);
    }

    #[test]
    fn drag_track_monotonic_delays_positive() {
        let track = human_drag_track(120.0, 5.0, 0.0, 7);
        assert!(track.iter().all(|p| p.2 > 0));
    }

    #[test]
    fn drag_track_dense_sampling_matches_duration() {
        // 0.8s / ~13ms ≈ 60 个点(密集采样,远多于旧版 ~30)。
        let track = human_drag_track(180.0, 0.0, 0.8, 42);
        assert!(track.len() >= 45, "采样应密集,len={}", track.len());
        // 各点延时之和应大致等于设定时长(±迟疑额外量)。
        let total: u64 = track.iter().map(|p| p.2).sum();
        assert!((700..=1100).contains(&total), "总时长≈0.8s,实得 {total}ms");
        // 中段速度应快于两端(minimum-jerk 钟形):比较相邻位移增量。
        let xs: Vec<f64> = track.iter().map(|p| p.0).collect();
        let mid = xs.len() / 2;
        let v_mid = xs[mid] - xs[mid - 1];
        let v_start = xs[1] - xs[0];
        assert!(v_mid > v_start, "中段应比起步快(钟形速度)");
    }
}
