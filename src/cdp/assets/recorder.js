// drission 录制脚本:在页面里挂捕获阶段事件钩子,计算 DP 风格选择器,
// 通过 binding window.__drission_record(JSON) 把动作回传宿主(由 ChromiumRecorder 收集)。
// 经 Page.addScriptToEvaluateOnNewDocument 注入到每个框架,每次导航后自动就位。
// 覆盖:click / change(fill/select/check)/ keydown(press)/ mouseover(hover,防抖)/
//       拖拽(HTML5 DnD + 指针按住移动)/ iframe(同源经 frameElement 计算框选择器)。
(() => {
  if (window.__drissionRecInstalled) return;
  window.__drissionRecInstalled = true;

  const esc = (s) =>
    window.CSS && CSS.escape ? CSS.escape(s) : String(s).replace(/[^a-zA-Z0-9_-]/g, "\\$&");
  const uniqIn = (doc, sel) => {
    try {
      return doc.querySelectorAll(sel).length === 1;
    } catch (e) {
      return false;
    }
  };

  // 选择器优先级(在给定 doc 内):#id(唯一) → @name:val(唯一) → css 路径。
  function selectorForIn(el, doc) {
    if (el.id && uniqIn(doc, "#" + esc(el.id))) return "#" + el.id;
    const nm = el.getAttribute && el.getAttribute("name");
    if (nm && uniqIn(doc, el.tagName.toLowerCase() + '[name="' + nm + '"]')) return "@name:" + nm;
    const parts = [];
    let cur = el;
    while (cur && cur.nodeType === 1 && cur !== doc.documentElement) {
      if (cur.id && uniqIn(doc, "#" + esc(cur.id))) {
        parts.unshift("#" + cur.id);
        break;
      }
      let sel = cur.tagName.toLowerCase();
      const par = cur.parentElement;
      if (par) {
        const same = Array.prototype.filter.call(par.children, (c) => c.tagName === cur.tagName);
        if (same.length > 1) sel += ":nth-of-type(" + (same.indexOf(cur) + 1) + ")";
      }
      parts.unshift(sel);
      cur = cur.parentElement;
    }
    return "css:" + parts.join(" > ");
  }
  const selectorFor = (el) => selectorForIn(el, document);

  // 本框架在父文档中的 <iframe> 选择器(同源经 frameElement 可得;顶层/跨源为 null)。
  // 惰性计算:首次交互时框架已完全挂载,frameElement 可靠(顶部即算会遇 const 暂时性死区)。
  const frameSelector = () => {
    try {
      const fe = window.frameElement;
      return fe ? selectorForIn(fe, fe.ownerDocument) : null;
    } catch (e) {
      return null; // 跨源
    }
  };

  const send = (a) => {
    try {
      const f = frameSelector();
      if (f) a.frame = f;
      if (window.__drission_record) window.__drission_record(JSON.stringify(a));
    } catch (e) {}
  };

  const isTextInput = (el) => {
    if (el.tagName === "TEXTAREA") return true;
    if (el.tagName !== "INPUT") return false;
    const t = (el.getAttribute("type") || "text").toLowerCase();
    return ["text", "password", "email", "search", "tel", "url", "number"].includes(t);
  };

  // 拖拽状态:HTML5 DnD 源、指针按住起点、拖拽后抑制误记 click 的时间窗。
  let dndFrom = null;
  let down = null;
  let suppressClickUntil = 0;

  document.addEventListener(
    "click",
    (e) => {
      if (Date.now() < suppressClickUntil) return; // 刚发生指针拖拽,忽略尾随 click
      const el = e.target;
      if (!el || el.nodeType !== 1) return;
      const tag = el.tagName.toLowerCase();
      if (tag === "select") return;
      if (isTextInput(el)) return;
      if (tag === "input") {
        const t = (el.getAttribute("type") || "text").toLowerCase();
        if (t === "checkbox" || t === "radio") return;
      }
      send({ type: "click", selector: selectorFor(el), tag: tag });
    },
    true
  );

  document.addEventListener(
    "change",
    (e) => {
      const el = e.target;
      if (!el || el.nodeType !== 1) return;
      const tag = el.tagName.toLowerCase();
      if (tag === "select") {
        send({ type: "select", selector: selectorFor(el), value: el.value });
        return;
      }
      if (tag === "input") {
        const t = (el.getAttribute("type") || "text").toLowerCase();
        if (t === "checkbox" || t === "radio") {
          send({ type: "check", selector: selectorFor(el), checked: !!el.checked });
          return;
        }
      }
      if (isTextInput(el)) {
        send({ type: "fill", selector: selectorFor(el), value: el.value != null ? String(el.value) : "" });
      }
    },
    true
  );

  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Enter") return;
      const el = e.target;
      if (!el || el.nodeType !== 1) return;
      if (isTextInput(el)) send({ type: "press", selector: selectorFor(el), key: "Enter" });
    },
    true
  );

  // ── 悬停(防抖):指针在"可悬停元素"上停留 ~250ms 即记一次 ───────────────────
  const hoverable = (el) => {
    const tag = el.tagName.toLowerCase();
    if (["a", "button", "summary"].includes(tag)) return true;
    const role = (el.getAttribute("role") || "").toLowerCase();
    if (["button", "menuitem", "tab", "link"].includes(role)) return true;
    if (el.hasAttribute("aria-haspopup") || el.hasAttribute("onmouseenter") || el.hasAttribute("onmouseover"))
      return true;
    try {
      return window.getComputedStyle(el).cursor === "pointer";
    } catch (e) {
      return false;
    }
  };
  let hoverTimer = null;
  let hoverEl = null;
  let lastHover = null;
  document.addEventListener(
    "mouseover",
    (e) => {
      hoverEl = e.target;
      if (hoverTimer) clearTimeout(hoverTimer);
      hoverTimer = setTimeout(() => {
        const el = hoverEl;
        if (!el || el.nodeType !== 1) return;
        if (!hoverable(el)) return;
        const sel = selectorFor(el);
        if (sel === lastHover) return;
        lastHover = sel;
        send({ type: "hover", selector: sel });
      }, 250);
    },
    true
  );

  // ── 拖拽:HTML5 DnD(不产生 click,无需抑制)──────────────────────────────
  document.addEventListener(
    "dragstart",
    (e) => {
      if (e.target && e.target.nodeType === 1) dndFrom = selectorFor(e.target);
    },
    true
  );
  document.addEventListener(
    "drop",
    (e) => {
      if (dndFrom && e.target && e.target.nodeType === 1) {
        send({ type: "drag", from: dndFrom, to: selectorFor(e.target) });
      }
      dndFrom = null;
    },
    true
  );

  // ── 拖拽:指针按住移动(mousedown → 移动 → mouseup 落到别处);尾随 click 短抑制 ──
  document.addEventListener(
    "mousedown",
    (e) => {
      if (e.button !== 0 || !e.target || e.target.nodeType !== 1) {
        down = null;
        return;
      }
      down = { el: e.target, x: e.clientX, y: e.clientY };
    },
    true
  );
  document.addEventListener(
    "mouseup",
    (e) => {
      if (!down || !e.target || e.target.nodeType !== 1) {
        down = null;
        return;
      }
      const dist = Math.hypot(e.clientX - down.x, e.clientY - down.y);
      const up = e.target;
      if (dist > 12 && up !== down.el) {
        send({ type: "drag", from: selectorFor(down.el), to: selectorFor(up) });
        suppressClickUntil = Date.now() + 150; // 仅指针拖拽的尾随 click 抑制
      }
      down = null;
    },
    true
  );
})();
