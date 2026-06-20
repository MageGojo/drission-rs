//! [`ChromiumElement`]:CDP 后端的元素句柄(由 `objectId` 标识),对标 Camoufox 后端的 `Element`。
//!
//! - 读写文本/属性走 `Runtime.callFunctionOn`(函数里 `this` 即本元素)。
//! - **可信点击**走 `Input.dispatchMouseEvent`(`isTrusted=true`,优于 JS `.click()`):
//!   取元素中心点 → `mouseMoved`→`mousePressed`→`mouseReleased`。
//! - **拟人输入** `input_human`:逐字符 `keyDown(text)`+`keyUp` + 随机停顿(完整按键事件序列)。
//! - 支持链式 `ele.ele(..)` / 相对定位 / `eles` / 元素截图。
//!
//! 注意:导航会使旧 `objectId` 失效(此时操作报协议错误,需重新 `tab.ele` 查找)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::sleep;

use crate::cdp::core::{CdpCore, Xorshift, human_drag_track, seed_from_clock};
use crate::cdp::shadow::ChromiumShadowRoot;
use crate::keys::KeyInput;
use crate::locator::{self, Query};
use crate::{Error, Result};

/// 一个页面元素句柄(CDP 后端)。
#[derive(Clone)]
pub struct ChromiumElement {
    core: Arc<CdpCore>,
    object_id: String,
}

impl ChromiumElement {
    pub(crate) fn new(core: Arc<CdpCore>, object_id: String) -> Self {
        Self { core, object_id }
    }

    /// 底层 objectId(调试用)。
    pub fn object_id(&self) -> &str {
        &self.object_id
    }

    /// 在本元素上调用函数(`this` 指代本元素)取值。
    async fn call_value(&self, declaration: &str, args: Vec<Value>) -> Result<Value> {
        self.core
            .call_value(&self.object_id, declaration, args)
            .await
    }

    /// 在本元素上调用返回**单个节点**的函数 → 新元素;`null` 则 [`Error::ElementNotFound`]。
    async fn call_to_element(
        &self,
        declaration: &str,
        args: Vec<Value>,
        what: &str,
    ) -> Result<ChromiumElement> {
        match self
            .core
            .call_handle(&self.object_id, declaration, args)
            .await?
        {
            Some(oid) => Ok(ChromiumElement::new(self.core.clone(), oid)),
            None => Err(Error::ElementNotFound(what.to_string())),
        }
    }

    /// 在本元素上调用返回**节点数组**的函数 → 元素列表。
    async fn call_to_elements(
        &self,
        declaration: &str,
        args: Vec<Value>,
    ) -> Result<Vec<ChromiumElement>> {
        let Some(arr) = self
            .core
            .call_handle(&self.object_id, declaration, args)
            .await?
        else {
            return Ok(Vec::new());
        };
        let oids = self.core.array_object_ids(&arr).await?;
        Ok(oids
            .into_iter()
            .map(|oid| ChromiumElement::new(self.core.clone(), oid))
            .collect())
    }

    // ── 读取 ──────────────────────────────────────────────────────────────

    /// 元素可见文本(`innerText`,回退 `textContent`)。
    pub async fn text(&self) -> Result<String> {
        let v = self
            .call_value(
                "function(){ return this.innerText ?? this.textContent ?? ''; }",
                vec![],
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 读取 HTML 特性;不存在返回 `None`。
    pub async fn attr(&self, name: &str) -> Result<Option<String>> {
        let v = self
            .call_value(
                "function(name){ return this.getAttribute(name); }",
                vec![json!({ "value": name })],
            )
            .await?;
        Ok(v.as_str().map(str::to_string))
    }

    /// 全部属性(`name → value`)。
    pub async fn attrs(&self) -> Result<HashMap<String, String>> {
        let v = self
            .call_value(
                "function(){ const o={}; for (const a of (this.attributes||[])) o[a.name]=a.value; return o; }",
                vec![],
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

    /// 读取 JS 属性(如 `checked`/`value`/`href`,返回原始 JSON 值)。区别于 [`attr`](Self::attr)。
    pub async fn property(&self, name: &str) -> Result<Value> {
        self.call_value(
            "function(n){ return this[n]; }",
            vec![json!({ "value": name })],
        )
        .await
    }

    /// 表单元素的 `value`。
    pub async fn value(&self) -> Result<String> {
        let v = self
            .call_value("function(){ return this.value ?? ''; }", vec![])
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 标签名(小写)。
    pub async fn tag(&self) -> Result<String> {
        let v = self
            .call_value(
                "function(){ return this.tagName ? this.tagName.toLowerCase() : ''; }",
                vec![],
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// outer HTML。
    pub async fn html(&self) -> Result<String> {
        let v = self
            .call_value("function(){ return this.outerHTML ?? ''; }", vec![])
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// inner HTML。
    pub async fn inner_html(&self) -> Result<String> {
        let v = self
            .call_value("function(){ return this.innerHTML ?? ''; }", vec![])
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 在本元素 HTML 快照内静态查找第一个匹配元素(对齐 camoufox `Element::s_ele`)。
    pub async fn s_ele(&self, selector: &str) -> Result<crate::static_element::StaticElement> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.ele(selector)
    }

    /// 在本元素 HTML 快照内静态查找所有匹配元素(对齐 camoufox `Element::s_eles`)。
    pub async fn s_eles(
        &self,
        selector: &str,
    ) -> Result<Vec<crate::static_element::StaticElement>> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.eles(selector)
    }

    /// 把本元素(或其内首个 `<table>`)解析为二维文本表格。对齐 camoufox `Element::table`。
    pub async fn table(&self) -> Result<Vec<Vec<String>>> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.table()
    }

    /// 表格转记录(首行作表头)。对齐 camoufox `Element::table_records`。
    pub async fn table_records(&self) -> Result<Vec<std::collections::HashMap<String, String>>> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.table_records()
    }

    /// 是否可见(有客户端矩形 + 非 `hidden`/`none`)。
    pub async fn is_displayed(&self) -> Result<bool> {
        let v = self
            .call_value(
                "function(){ const r=this.getClientRects(); const s=getComputedStyle(this); \
                 return r.length>0 && s.visibility!=='hidden' && s.display!=='none'; }",
                vec![],
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 是否可用(未 `disabled`)。
    pub async fn is_enabled(&self) -> Result<bool> {
        let v = self
            .call_value("function(){ return !this.disabled; }", vec![])
            .await?;
        Ok(v.as_bool().unwrap_or(true))
    }

    /// 是否处于选中态(checkbox/radio)。
    pub async fn is_checked(&self) -> Result<bool> {
        let v = self
            .call_value("function(){ return !!this.checked; }", vec![])
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 是否在视口内(与视口有交叠)。对齐 camoufox `is_in_viewport`。
    pub async fn is_in_viewport(&self) -> Result<bool> {
        let v = self
            .call_value(
                "function(){ const r=this.getBoundingClientRect(); \
                 return r.bottom>0 && r.right>0 && r.top<innerHeight && r.left<innerWidth; }",
                vec![],
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 中心点是否被其它元素遮挡(`elementFromPoint` 命中的不是自身/血缘)。对齐 camoufox `is_covered`。
    pub async fn is_covered(&self) -> Result<bool> {
        let v = self
            .call_value(
                "function(){ const r=this.getBoundingClientRect(); \
                 const x=r.left+r.width/2, y=r.top+r.height/2; const el=document.elementFromPoint(x,y); \
                 if(!el) return false; return !(el===this || this.contains(el) || el.contains(this)); }",
                vec![],
            )
            .await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// 是否可点击(可见 + 可用 + 在视口 + 未被遮挡)。对齐 camoufox `is_clickable`。
    pub async fn is_clickable(&self) -> Result<bool> {
        Ok(self.is_displayed().await?
            && self.is_enabled().await?
            && self.is_in_viewport().await?
            && !self.is_covered().await?)
    }

    /// 元素左上角的**页面坐标** `(x, y)`(含滚动)。对齐 camoufox `location`。
    pub async fn location(&self) -> Result<(f64, f64)> {
        let r = self.rect().await?;
        Ok((r.x, r.y))
    }

    /// 计算样式某属性值(`getComputedStyle`,如 `style("background-color")`)。对齐 camoufox `style`。
    pub async fn style(&self, name: &str) -> Result<String> {
        let v = self
            .call_value(
                "function(n){ return getComputedStyle(this).getPropertyValue(n); }",
                vec![json!({ "value": name })],
            )
            .await?;
        Ok(v.as_str().unwrap_or("").trim().to_string())
    }

    /// 从 DOM 移除本元素(对齐 camoufox `remove`)。
    pub async fn remove(&self) -> Result<()> {
        self.call_value("function(){ this.remove(); }", vec![])
            .await?;
        Ok(())
    }

    /// 元素级等待句柄:`ele.wait().displayed(..)/hidden/deleted/clickable`(对齐 camoufox `ele.wait()`)。
    pub fn wait(&self) -> ChromiumElementWait {
        ChromiumElementWait { ele: self.clone() }
    }

    /// 元素几何信息(页面坐标 + 视口坐标 + 尺寸)。
    pub async fn rect(&self) -> Result<ElementRect> {
        let v = self
            .call_value(
                "function(){ const r=this.getBoundingClientRect(); return { \
                 x:r.left+scrollX, y:r.top+scrollY, vx:r.left, vy:r.top, w:r.width, h:r.height }; }",
                vec![],
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

    /// 元素尺寸 `(width, height)`。
    pub async fn size(&self) -> Result<(f64, f64)> {
        let r = self.rect().await?;
        Ok((r.width, r.height))
    }

    /// 在本元素上执行 JS(函数体内 `this` 即本元素),返回其值。
    ///
    /// 例:`el.run_js("return this.dataset.id;")`。
    pub async fn run_js(&self, body: &str) -> Result<Value> {
        let decl = format!("function(){{ {body} }}");
        self.call_value(&decl, vec![]).await
    }

    // ── 查找(链式 / 相对定位)─────────────────────────────────────────────

    /// 在本元素范围内查找子元素(DP 定位语法)。
    pub async fn ele(&self, selector: &str) -> Result<ChromiumElement> {
        let (decl, arg) = query_decl(selector, true);
        self.call_to_element(&decl, vec![arg], selector).await
    }

    /// 在本元素范围内查找所有匹配子元素。
    pub async fn eles(&self, selector: &str) -> Result<Vec<ChromiumElement>> {
        let (decl, arg) = query_decl(selector, false);
        self.call_to_elements(&decl, vec![arg]).await
    }

    /// 直接父元素。无父元素则 [`Error::ElementNotFound`]。
    pub async fn parent(&self) -> Result<ChromiumElement> {
        self.call_to_element("function(){ return this.parentElement; }", vec![], "parent")
            .await
    }

    /// 向上第 `level` 级祖先(`level=1` 即父元素)。
    pub async fn parent_n(&self, level: usize) -> Result<ChromiumElement> {
        self.call_to_element(
            "function(n){ let e=this; for (let i=0;i<n&&e;i++) e=e.parentElement; return e; }",
            vec![json!({ "value": level })],
            "parent_n",
        )
        .await
    }

    /// 所有直接子元素。
    pub async fn children(&self) -> Result<Vec<ChromiumElement>> {
        self.call_to_elements("function(){ return Array.from(this.children); }", vec![])
            .await
    }

    /// 第 `index` 个直接子元素(**0 基**)。越界则 [`Error::ElementNotFound`]。
    pub async fn child(&self, index: usize) -> Result<ChromiumElement> {
        self.call_to_element(
            "function(i){ return this.children[i] || null; }",
            vec![json!({ "value": index })],
            "child",
        )
        .await
    }

    /// 下一个同级元素。
    pub async fn next(&self) -> Result<ChromiumElement> {
        self.call_to_element(
            "function(){ return this.nextElementSibling; }",
            vec![],
            "next",
        )
        .await
    }

    /// 上一个同级元素。
    pub async fn prev(&self) -> Result<ChromiumElement> {
        self.call_to_element(
            "function(){ return this.previousElementSibling; }",
            vec![],
            "prev",
        )
        .await
    }

    /// 所有同级元素(不含自身,按文档顺序)。
    pub async fn siblings(&self) -> Result<Vec<ChromiumElement>> {
        self.call_to_elements(
            "function(){ return this.parentElement ? \
             Array.from(this.parentElement.children).filter(c=>c!==this) : []; }",
            vec![],
        )
        .await
    }

    /// 向上找到第一个匹配 `selector`(**仅 CSS**,`closest`)的祖先。对齐 camoufox `parent_until`。
    pub async fn parent_until(&self, selector: &str) -> Result<ChromiumElement> {
        let css = match locator::parse(selector) {
            Query::Css(s) => s,
            Query::Xpath(_) => {
                return Err(Error::msg("parent_until 仅支持 CSS 选择器(xpath 改用 ele)"));
            }
        };
        self.call_to_element(
            "function(sel){ return this.parentElement ? this.parentElement.closest(sel) : null; }",
            vec![json!({ "value": css })],
            "parent_until",
        )
        .await
    }

    /// 之后的所有同级元素(文档顺序)。对齐 camoufox `nexts`。
    pub async fn nexts(&self) -> Result<Vec<ChromiumElement>> {
        self.call_to_elements(
            "function(){ const a=[]; let e=this.nextElementSibling; while(e){a.push(e); e=e.nextElementSibling;} return a; }",
            vec![],
        )
        .await
    }

    /// 之前的所有同级元素(文档顺序)。对齐 camoufox `prevs`。
    pub async fn prevs(&self) -> Result<Vec<ChromiumElement>> {
        self.call_to_elements(
            "function(){ const a=[]; let e=this.previousElementSibling; while(e){a.unshift(e); e=e.previousElementSibling;} return a; }",
            vec![],
        )
        .await
    }

    /// 取本元素的 **open** shadow root(`this.shadowRoot`)用于查找;无则报错。对齐 camoufox `shadow_root`。
    pub async fn shadow_root(&self) -> Result<ChromiumShadowRoot> {
        match self
            .core
            .call_handle(
                &self.object_id,
                "function(){ return this.shadowRoot; }",
                vec![],
            )
            .await?
        {
            Some(oid) => Ok(ChromiumShadowRoot::new(self.core.clone(), oid)),
            None => Err(Error::msg("元素没有 open shadowRoot")),
        }
    }

    /// 取本 `<iframe>` 的内容帧用于在帧内查找(对齐 camoufox `content_frame`)。
    pub async fn content_frame(&self) -> Result<crate::cdp::frame::ChromiumFrame> {
        crate::cdp::frame::ChromiumFrame::from_iframe(self.core.clone(), &self.object_id).await
    }

    // ── 动作 ──────────────────────────────────────────────────────────────

    /// 滚动到可见(居中)。
    pub async fn scroll_into_view(&self) -> Result<()> {
        self.call_value(
            "function(){ this.scrollIntoView({ block:'center', inline:'center' }); }",
            vec![],
        )
        .await?;
        Ok(())
    }

    /// 取元素中心点(视口坐标);先滚到可见。
    pub(crate) async fn center_point(&self) -> Result<(f64, f64)> {
        self.scroll_into_view().await?;
        let v = self
            .call_value(
                "function(){ const r=this.getBoundingClientRect(); \
                 return [r.left + r.width/2, r.top + r.height/2]; }",
                vec![],
            )
            .await?;
        let x = v.get(0).and_then(Value::as_f64).unwrap_or(0.0);
        let y = v.get(1).and_then(Value::as_f64).unwrap_or(0.0);
        if x <= 0.0 && y <= 0.0 {
            return Err(Error::Other("元素无可见内容区(可能不可见)".into()));
        }
        Ok((x, y))
    }

    /// **原生可信点击**(取中心点派发 `mouseMoved`→`mousePressed`→`mouseReleased`,`isTrusted=true`)。
    pub async fn click(&self) -> Result<()> {
        let (x, y) = self.center_point().await?;
        self.core
            .dispatch_mouse("mouseMoved", x, y, "none", 0, 0)
            .await?;
        self.core
            .dispatch_mouse("mousePressed", x, y, "left", 1, 1)
            .await?;
        self.core
            .dispatch_mouse("mouseReleased", x, y, "left", 0, 1)
            .await?;
        Ok(())
    }

    /// 把鼠标移到元素中心(hover,不点击)。
    pub async fn hover(&self) -> Result<()> {
        let (x, y) = self.center_point().await?;
        self.core
            .dispatch_mouse("mouseMoved", x, y, "none", 0, 0)
            .await
    }

    /// 人手轨迹拖拽:按住元素中心,沿 `(dx, dy)` 像素移动后释放(滑块验证码等)。
    ///
    /// 中间密集 `mouseMoved` 走**不等往返**的 fire 路径(由 `sleep` 控制 ~13ms 采样),
    /// 轨迹为 minimum-jerk 钟形速度 + 过冲回拉;`duration<=0` 按距离自估较快节奏。
    pub async fn drag(&self, dx: f64, dy: f64, duration: f64) -> Result<()> {
        let (x0, y0) = self.center_point().await?;
        self.drag_from(x0, y0, dx, dy, duration).await
    }

    /// 人手轨迹拖拽到视口绝对坐标 `(x, y)`。
    pub async fn drag_to(&self, x: f64, y: f64, duration: f64) -> Result<()> {
        let (x0, y0) = self.center_point().await?;
        self.drag_from(x0, y0, x - x0, y - y0, duration).await
    }

    async fn drag_from(&self, x0: f64, y0: f64, dx: f64, dy: f64, duration: f64) -> Result<()> {
        let mut rng = Xorshift::new(seed_from_clock());
        let steps = human_drag_track(dx, dy, duration, rng.next_u64());
        self.core
            .dispatch_mouse("mouseMoved", x0, y0, "none", 0, 0)
            .await?;
        sleep(Duration::from_millis(rng.range_ms(20, 60))).await;
        self.core
            .dispatch_mouse("mousePressed", x0, y0, "left", 1, 1)
            .await?;
        sleep(Duration::from_millis(rng.range_ms(70, 140))).await;
        for (ox, oy, delay) in steps {
            self.core
                .dispatch_mouse_fire("mouseMoved", x0 + ox, y0 + oy, "none", 1, 0)?;
            if delay > 0 {
                sleep(Duration::from_millis(delay)).await;
            }
        }
        sleep(Duration::from_millis(rng.range_ms(60, 130))).await;
        self.core
            .dispatch_mouse("mouseReleased", x0 + dx, y0 + dy, "left", 0, 1)
            .await?;
        Ok(())
    }

    /// 聚焦元素。
    pub async fn focus(&self) -> Result<()> {
        self.call_value("function(){ this.focus(); }", vec![])
            .await?;
        Ok(())
    }

    /// 在元素中输入文本(先聚焦,再一次性插入)。节奏不敏感时最快。
    pub async fn input(&self, text: &str) -> Result<()> {
        self.focus().await?;
        self.core.insert_text(text).await
    }

    /// **逐字符拟人输入**:聚焦后一个字符一个字符地敲(`keyDown`(带 `text`)+`keyUp`),
    /// 字符间 30~140ms 随机停顿——产生完整 keydown/keypress/input/keyup 事件序列,比
    /// [`input`](Self::input) 更像人,适合监听 `keydown`/`keyup`(自动补全、表单校验)的站点。
    pub async fn input_human(&self, text: &str) -> Result<()> {
        self.focus().await?;
        let mut rng = Xorshift::new(seed_from_clock());
        for ch in text.chars() {
            let s = ch.to_string();
            self.core.press_key(&s).await?;
            let delay = 30 + (rng.next_u64() % 110);
            sleep(Duration::from_millis(delay)).await;
        }
        Ok(())
    }

    /// 按**序列**输入(对应 DP `ele.input(['abc', Keys.ENTER])`):文本片段直接插入、特殊键派发按键。
    pub async fn input_keys(&self, parts: &[KeyInput]) -> Result<()> {
        self.focus().await?;
        for p in parts {
            match p {
                KeyInput::Text(t) => self.core.insert_text(t).await?,
                KeyInput::Key(k) => self.core.press_key(k).await?,
            }
        }
        Ok(())
    }

    /// **修饰组合键 / 热键**(先聚焦本元素):最后一项为主键,其余为修饰键(`Control`/`Ctrl`、
    /// `Shift`、`Alt`、`Meta`/`Cmd`)。CDP 原生 `modifiers` 位掩码,页面读得到 `e.ctrlKey` 等为 `true`。
    ///
    /// ```ignore
    /// ele.shortcut(&[Keys::CONTROL, "a"]).await?; // 选中本输入框全部文本
    /// ```
    pub async fn shortcut(&self, keys: &[&str]) -> Result<()> {
        self.focus().await?;
        self.core.key_combo(keys).await
    }

    /// 清空输入框(focus 后置空并派发 input/change 事件)。
    pub async fn clear(&self) -> Result<()> {
        self.call_value(
            "function(){ this.focus(); if ('value' in this) { this.value=''; \
             this.dispatchEvent(new Event('input',{bubbles:true})); \
             this.dispatchEvent(new Event('change',{bubbles:true})); } \
             else if (this.isContentEditable) { this.textContent=''; } }",
            vec![],
        )
        .await?;
        Ok(())
    }

    /// 为 `<select>` 选择给定 `value` 的选项,并派发 input/change。
    pub async fn select_value(&self, value: &str) -> Result<()> {
        self.call_value(
            "function(val){ this.value=val; \
             this.dispatchEvent(new Event('input',{bubbles:true})); \
             this.dispatchEvent(new Event('change',{bubbles:true})); }",
            vec![json!({ "value": value })],
        )
        .await?;
        Ok(())
    }

    /// 为 `<select>` 按**可见文本**选中选项,并派发 input/change。对齐 camoufox `select_text`。
    pub async fn select_text(&self, text: &str) -> Result<()> {
        self.call_value(
            "function(t){ const opts=Array.from(this.options||[]); \
             const o=opts.find(o=>o.text===t || (o.textContent||'').trim()===t); \
             if(o){ this.value=o.value; \
             this.dispatchEvent(new Event('input',{bubbles:true})); \
             this.dispatchEvent(new Event('change',{bubbles:true})); } }",
            vec![json!({ "value": text })],
        )
        .await?;
        Ok(())
    }

    /// 勾选 / 取消勾选 checkbox 或 radio;状态变化时派发 input/change。
    pub async fn set_checked(&self, checked: bool) -> Result<()> {
        self.call_value(
            "function(c){ if (this.checked!==c) { this.checked=c; \
             this.dispatchEvent(new Event('input',{bubbles:true})); \
             this.dispatchEvent(new Event('change',{bubbles:true})); } }",
            vec![json!({ "value": checked })],
        )
        .await?;
        Ok(())
    }

    /// 给 `<input type=file>` 设置要上传的文件(`Input` 域不涉及;走 `DOM.setFileInputFiles`)。
    ///
    /// `paths` 为本地文件**绝对路径**;多文件需该 input 带 `multiple`。
    pub async fn set_files(&self, paths: &[&str]) -> Result<()> {
        let files: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.core
            .send(
                "DOM.setFileInputFiles",
                json!({ "objectId": self.object_id, "files": files }),
            )
            .await?;
        Ok(())
    }

    /// 给 `<input type=file>` 设置单个上传文件(对齐 camoufox `upload`)。
    pub async fn upload(&self, path: &str) -> Result<()> {
        self.set_files(&[path]).await
    }

    /// **自然上传**:点本元素(触发系统文件框)→ 拦截 `fileChooser` → 自动填入 `paths`。
    /// 对齐 camoufox `click_to_upload`;CDP 走原生 `Page.setInterceptFileChooserDialog`+`fileChooserOpened`。
    pub async fn click_to_upload(&self, paths: &[&str], timeout: Option<Duration>) -> Result<bool> {
        let mut events = self.core.conn.subscribe();
        let sid = self.core.session_id.clone();
        self.core
            .send(
                "Page.setInterceptFileChooserDialog",
                json!({ "enabled": true }),
            )
            .await?;
        // 点击触发文件框。
        self.click().await?;
        let dur = timeout.unwrap_or_else(|| self.core.timeout());
        let backend = tokio::time::timeout(dur, async {
            loop {
                match events.recv().await {
                    Ok(ev)
                        if ev.method == "Page.fileChooserOpened"
                            && ev.session_id.as_deref() == Some(&sid) =>
                    {
                        return ev.params["backendNodeId"].as_i64();
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        let _ = self
            .core
            .send(
                "Page.setInterceptFileChooserDialog",
                json!({ "enabled": false }),
            )
            .await;
        let Some(bn) = backend else {
            return Ok(false);
        };
        let files: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.core
            .send(
                "DOM.setFileInputFiles",
                json!({ "files": files, "backendNodeId": bn }),
            )
            .await?;
        Ok(true)
    }

    /// 元素截图,返回 PNG 字节。先把元素**滚到视口中央**再按其页面矩形裁剪。
    pub async fn screenshot_bytes(&self) -> Result<Vec<u8>> {
        self.scroll_into_view().await?;
        let v = self
            .call_value(
                "function(){ const r=this.getBoundingClientRect(); \
                 return [r.left+scrollX, r.top+scrollY, Math.max(r.width,1), Math.max(r.height,1)]; }",
                vec![],
            )
            .await?;
        let x = v.get(0).and_then(Value::as_f64).unwrap_or(0.0);
        let y = v.get(1).and_then(Value::as_f64).unwrap_or(0.0);
        let w = v.get(2).and_then(Value::as_f64).unwrap_or(1.0);
        let h = v.get(3).and_then(Value::as_f64).unwrap_or(1.0);
        let clip = json!({ "x": x, "y": y, "width": w, "height": h, "scale": 1 });
        let r = self
            .core
            .send(
                "Page.captureScreenshot",
                json!({ "format": "png", "clip": clip, "captureBeyondViewport": true }),
            )
            .await?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 无元素截图数据"))?;
        crate::util::base64_decode(data).ok_or_else(|| Error::msg("CDP: 元素截图 base64 解码失败"))
    }

    /// 元素截图并保存到 `path`。返回写入路径。
    pub async fn get_screenshot(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let path = path.as_ref().to_path_buf();
        let bytes = self.screenshot_bytes().await?;
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| Error::msg(format!("写入截图 {} 失败: {e}", path.display())))?;
        Ok(path)
    }
}

/// 元素级等待句柄(由 [`ChromiumElement::wait`] 返回),对齐 camoufox `ElementWait`。
/// 各方法在 `timeout`(`None` 用标签默认超时)内轮询,**超时返回 `false` 而非报错**。
pub struct ChromiumElementWait {
    ele: ChromiumElement,
}

impl ChromiumElementWait {
    /// 等元素变为可见。
    pub async fn displayed(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, |e| async move {
            e.is_displayed().await.unwrap_or(false)
        })
        .await
    }

    /// 等元素变为不可见(仍在 DOM 但隐藏)。
    pub async fn hidden(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, |e| async move {
            !e.is_displayed().await.unwrap_or(false)
        })
        .await
    }

    /// 等元素从 DOM 删除(`isConnected===false` 或句柄失效)。
    pub async fn deleted(&self, timeout: Option<Duration>) -> Result<bool> {
        let deadline =
            std::time::Instant::now() + timeout.unwrap_or_else(|| self.ele.core.timeout());
        loop {
            let gone = match self
                .ele
                .call_value("function(){ return this.isConnected===false; }", vec![])
                .await
            {
                Ok(v) => v.as_bool().unwrap_or(false),
                Err(_) => true, // objectId 失效 = 已删除
            };
            if gone {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }

    /// 等元素变为可点击(可见 + 可用 + 在视口 + 未遮挡)。
    pub async fn clickable(&self, timeout: Option<Duration>) -> Result<bool> {
        self.poll(timeout, |e| async move {
            e.is_clickable().await.unwrap_or(false)
        })
        .await
    }

    /// 轮询直到 `pred(ele)` 为真或超时。
    async fn poll<F, Fut>(&self, timeout: Option<Duration>, pred: F) -> Result<bool>
    where
        F: Fn(ChromiumElement) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline =
            std::time::Instant::now() + timeout.unwrap_or_else(|| self.ele.core.timeout());
        loop {
            if pred(self.ele.clone()).await {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(80)).await;
        }
    }
}

/// 元素几何信息(由 [`ChromiumElement::rect`] 返回)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementRect {
    /// 左上角页面坐标 X(含滚动偏移)。
    pub x: f64,
    /// 左上角页面坐标 Y(含滚动偏移)。
    pub y: f64,
    /// 左上角视口坐标 X。
    pub viewport_x: f64,
    /// 左上角视口坐标 Y。
    pub viewport_y: f64,
    /// 宽。
    pub width: f64,
    /// 高。
    pub height: f64,
}

/// 把 DP 选择器解析成在某节点上 `querySelector(All)` / `document.evaluate` 的函数声明 + 参数。
/// `single=true` 取单个、否则取数组。
pub(crate) fn query_decl(selector: &str, single: bool) -> (String, Value) {
    match locator::parse(selector) {
        Query::Css(sel) => {
            let decl = if single {
                "function(s){ return this.querySelector(s); }"
            } else {
                "function(s){ return Array.from(this.querySelectorAll(s)); }"
            };
            (decl.to_string(), json!({ "value": sel }))
        }
        Query::Xpath(xp) => {
            let decl = if single {
                "function(xp){ return document.evaluate(xp, this, null, 9, null).singleNodeValue; }"
            } else {
                "function(xp){ const it=document.evaluate(xp, this, null, 7, null); \
                 const a=[]; for (let i=0;i<it.snapshotLength;i++) a.push(it.snapshotItem(i)); return a; }"
            };
            (decl.to_string(), json!({ "value": xp }))
        }
    }
}
