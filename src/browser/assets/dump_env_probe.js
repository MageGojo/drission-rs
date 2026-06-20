/*
 * drission 通用吐环境探针(模板)。注入页面后挂在 window.__DUMP__。
 * 由 dump_env.rs 把两个占位符替换为运行配置后注入(故本文件是模板,未替换时不可直接运行):
 *   - 配置占位符 -> {proxy,watch,sig,targets}
 *   - 指纹配方占位符 -> fp_recipes.js 内容(canvas/webgl/audio 配方,与 verify 快照共用)
 * 复用 web-reverse-env 的 collect(种子) / observe(Proxy 吐环境) 思路:
 *   1) (可选)Proxy 包裹 watch 列出的顶层对象(navigator/screen/...),记录目标算法读取的环境路径(access);
 *   2) hook fetch / XMLHttpRequest:命中签名参数(sig)的请求记录 writer(URL+调用栈 -> sinks);按
 *      targets 定向提取 query / header / cookie 指定参数的真实上线值(targets);**并 hook
 *      Function.prototype.toString 让这些 hook 自报 native code**,规避 ('' + fetch) 之类的反 hook 检测;
 *   3) collectSeed():采集完整环境种子(navigator/screen/document/canvas/webgl/audio/storage)——"吐全"的值来源。
 * 整体是一个 IIFE 表达式,供 add_init_script(导航前注入)使用;幂等(重复注入直接返回)。
 */
(function () {
  if (window.__DUMP__) return "already";
  var CFG = __DUMP_CFG__;
  var WATCH = (CFG && CFG.watch) || ["navigator", "screen"];
  var SIG = (CFG && CFG.sig) || ["a_bogus", "X-Bogus", "x-bogus", "msToken", "_signature", "verifyFp", "mssdk", "webid"];
  var TARGETS = (CFG && CFG.targets) || [];
  var USE_PROXY = !!(CFG && CFG.proxy);
  var MAX = 8000;
  var D = (window.__DUMP__ = { access: {}, accessOrder: [], sinks: [], targets: [], seed: null, audioFp: null, cfg: CFG });

  // 指纹配方(__fpCanvas / __fpWebGL / __fpAudioAsync),与 verify 快照同源,保证录制==回放配方。
  __FP_RECIPES__

  function note(path) {
    if (!(path in D.access) && D.accessOrder.length < MAX) D.accessOrder.push(path);
    D.access[path] = (D.access[path] || 0) + 1;
  }

  // 0) 反 hook 检测:hook Function.prototype.toString,让被标记为 native 的函数自报 [native code]。
  //    很多签名/风控脚本用 ('' + fetch) / fetch.toString() / Function.prototype.toString.call(fetch)
  //    检测 fetch/XHR 是否被改写;标记后这些检测一律看到原生形态。
  var markNative;
  try {
    var realToString = Function.prototype.toString;
    var nativeMap = new WeakMap();
    var tsHook = function toString() {
      try {
        if (nativeMap.has(this)) return nativeMap.get(this);
      } catch (e) {}
      return realToString.call(this);
    };
    markNative = function (fn, name) {
      try {
        nativeMap.set(fn, "function " + name + "() {\n    [native code]\n}");
      } catch (e) {}
    };
    markNative(tsHook, "toString"); // toString 自身也得装成原生
    nativeMap.set(realToString, "function toString() {\n    [native code]\n}");
    Object.defineProperty(Function.prototype, "toString", { value: tsHook, configurable: true, writable: true });
  } catch (e) {
    markNative = function () {};
  }

  // 1) Proxy 吐环境(可选):包裹后返回真实值,只在读取时记录路径。
  //    默认【关闭】——对抖音等强检测站点替换 navigator 会被识破、导致不发签名请求;
  //    仅 proxy:true 时开启,用于诊断"算法读了哪些环境字段",据此裁出 Accessed 关键集。
  function observe(real, label) {
    try {
      return new Proxy(real, {
        get: function (t, k, r) {
          try { note(label + "." + String(k)); } catch (e) {}
          var v = Reflect.get(t, k, r);
          return typeof v === "function" ? v.bind(t) : v;
        },
        has: function (t, k) {
          try { note(label + ".(in)" + String(k)); } catch (e) {}
          return Reflect.has(t, k);
        },
      });
    } catch (e) {
      return real;
    }
  }
  if (USE_PROXY) {
    WATCH.forEach(function (name) {
      try {
        var real = window[name];
        if (!real || typeof real !== "object") return;
        var p = observe(real, name);
        Object.defineProperty(window, name, { configurable: true, get: function () { return p; } });
      } catch (e) { D.sinks.push({ note: name + " proxy 失败: " + (e && e.message) }); }
    });
  }

  // 2) sink hook(记录签名请求 writer + 原始调用栈) + 定向目标提取(query/header/cookie 指定参数的真实值)。
  function hasSig(u) {
    u = String(u || "");
    for (var i = 0; i < SIG.length; i++) if (u.indexOf(SIG[i]) >= 0) return true;
    return false;
  }
  function pushSink(o) { if (D.sinks.length < MAX) D.sinks.push(o); }
  function qval(url, key) {
    try {
      var qs = String(url).split("?")[1];
      if (!qs) return null;
      var ps = qs.split("&");
      for (var i = 0; i < ps.length; i++) {
        var eq = ps[i].indexOf("=");
        var k = eq < 0 ? ps[i] : ps[i].slice(0, eq);
        var v = eq < 0 ? "" : ps[i].slice(eq + 1);
        if (k === key || (function () { try { return decodeURIComponent(k) === key; } catch (e) { return false; } })()) return v;
      }
    } catch (e) {}
    return null;
  }
  function cval(key) {
    try {
      var cs = (document.cookie || "").split(";");
      for (var i = 0; i < cs.length; i++) {
        var s = cs[i].trim();
        var eq = s.indexOf("=");
        var k = eq < 0 ? s : s.slice(0, eq);
        if (k === key) return eq < 0 ? "" : s.slice(eq + 1);
      }
    } catch (e) {}
    return null;
  }
  function hval(init, xhrHeaders, key) {
    var lk = String(key).toLowerCase();
    try {
      if (init && init.headers) {
        var h = init.headers;
        if (typeof Headers !== "undefined" && h instanceof Headers) {
          var got = h.get(key);
          if (got != null) return got;
        } else if (Array.isArray(h)) {
          for (var i = 0; i < h.length; i++) if (String(h[i][0]).toLowerCase() === lk) return h[i][1];
        } else {
          for (var k in h) if (k.toLowerCase() === lk) return h[k];
        }
      }
    } catch (e) {}
    if (xhrHeaders) {
      for (var j = 0; j < xhrHeaders.length; j++) if (String(xhrHeaders[j][0]).toLowerCase() === lk) return xhrHeaders[j][1];
    }
    return null;
  }
  function extractTargets(url, init, xhrHeaders) {
    if (!TARGETS.length) return;
    try {
      var stack = (new Error()).stack;
      for (var i = 0; i < TARGETS.length; i++) {
        var t = TARGETS[i], val = null;
        if (t.kind === "query") val = qval(url, t.key);
        else if (t.kind === "cookie") val = cval(t.key);
        else if (t.kind === "header") val = hval(init, xhrHeaders, t.key);
        if (val != null && D.targets.length < MAX) {
          D.targets.push({ kind: t.kind, key: t.key, value: val, url: String(url || ""), stack: stack });
        }
      }
    } catch (e) {}
  }
  try {
    var of = window.fetch;
    if (of) {
      var fetchHook = function (input, init) {
        try {
          var u = typeof input === "string" ? input : ((input && input.url) || "");
          if (hasSig(u)) pushSink({ type: "fetch", url: u, stack: (new Error()).stack });
          extractTargets(u, init, null);
        } catch (e) {}
        return of.apply(this, arguments);
      };
      markNative(fetchHook, "fetch");
      window.fetch = fetchHook;
    }
  } catch (e) {}
  try {
    var oo = XMLHttpRequest.prototype.open;
    var osh = XMLHttpRequest.prototype.setRequestHeader;
    var os = XMLHttpRequest.prototype.send;
    var openHook = function (m, u) {
      try { this.__d_url = u; this.__d_m = m; this.__d_h = []; } catch (e) {}
      return oo.apply(this, arguments);
    };
    var headerHook = function (k, v) {
      try { (this.__d_h = this.__d_h || []).push([k, v]); } catch (e) {}
      return osh.apply(this, arguments);
    };
    var sendHook = function (b) {
      try {
        if (hasSig(this.__d_url)) pushSink({ type: "xhr", method: this.__d_m || "GET", url: String(this.__d_url), stack: (new Error()).stack });
        extractTargets(this.__d_url, null, this.__d_h);
      } catch (e) {}
      return os.apply(this, arguments);
    };
    markNative(openHook, "open");
    markNative(headerHook, "setRequestHeader");
    markNative(sendHook, "send");
    XMLHttpRequest.prototype.open = openHook;
    XMLHttpRequest.prototype.setRequestHeader = headerHook;
    XMLHttpRequest.prototype.send = sendHook;
  } catch (e) {}

  // 3) 环境种子采集(collect-browser-env 改造):全量真实值,作为 env.js 的值来源。
  function sc(fn, fb) { try { var v = fn(); return v === undefined ? fb : v; } catch (e) { return fb; } }
  function dumpStorage(s) {
    var o = {};
    try { for (var i = 0; i < s.length; i++) { var k = s.key(i); o[k] = s.getItem(k); } } catch (e) {}
    return o;
  }

  // 异步预算 audio 指纹(OfflineAudioContext 渲染),就绪后落到 D.audioFp;collect 时读取。
  try { __fpAudioAsync(function (r) { D.audioFp = r; }); } catch (e) {}

  D.collectSeed = function () {
    var seed = {
      timestamp: new Date().toISOString(),
      location: {
        href: sc(function () { return location.href; }, ""),
        origin: sc(function () { return location.origin; }, ""),
        host: sc(function () { return location.host; }, ""),
        protocol: sc(function () { return location.protocol; }, ""),
        hostname: sc(function () { return location.hostname; }, ""),
        port: sc(function () { return location.port; }, ""),
        pathname: sc(function () { return location.pathname; }, ""),
        search: sc(function () { return location.search; }, ""),
        hash: sc(function () { return location.hash; }, ""),
      },
      navigator: {
        userAgent: sc(function () { return navigator.userAgent; }, ""),
        appVersion: sc(function () { return navigator.appVersion; }, ""),
        appName: sc(function () { return navigator.appName; }, ""),
        appCodeName: sc(function () { return navigator.appCodeName; }, ""),
        product: sc(function () { return navigator.product; }, ""),
        productSub: sc(function () { return navigator.productSub; }, ""),
        platform: sc(function () { return navigator.platform; }, ""),
        vendor: sc(function () { return navigator.vendor; }, ""),
        vendorSub: sc(function () { return navigator.vendorSub; }, ""),
        language: sc(function () { return navigator.language; }, ""),
        languages: sc(function () { return Array.from(navigator.languages || []); }, []),
        webdriver: sc(function () { return navigator.webdriver; }, false),
        hardwareConcurrency: sc(function () { return navigator.hardwareConcurrency; }, 0),
        deviceMemory: sc(function () { return navigator.deviceMemory; }, undefined),
        maxTouchPoints: sc(function () { return navigator.maxTouchPoints; }, 0),
        cookieEnabled: sc(function () { return navigator.cookieEnabled; }, true),
        doNotTrack: sc(function () { return navigator.doNotTrack; }, null),
        oscpu: sc(function () { return navigator.oscpu; }, undefined),
        pdfViewerEnabled: sc(function () { return navigator.pdfViewerEnabled; }, undefined),
        userAgentData: sc(function () {
          return navigator.userAgentData
            ? { brands: navigator.userAgentData.brands, mobile: navigator.userAgentData.mobile, platform: navigator.userAgentData.platform }
            : null;
        }, null),
        // 插件 / MIME 列表(常被读做指纹)。回放在 env.js 里包成类数组(length/下标/namedItem)。
        plugins: sc(function () {
          var out = [], ps = navigator.plugins || [];
          for (var i = 0; i < ps.length; i++) {
            var p = ps[i], mts = [];
            for (var j = 0; j < p.length; j++) { var m = p[j]; mts.push({ type: m.type, suffixes: m.suffixes, description: m.description }); }
            out.push({ name: p.name, filename: p.filename, description: p.description, length: p.length, mimeTypes: mts });
          }
          return out;
        }, []),
        mimeTypes: sc(function () {
          var out = [], ms = navigator.mimeTypes || [];
          for (var i = 0; i < ms.length; i++) {
            var m = ms[i];
            out.push({ type: m.type, suffixes: m.suffixes, description: m.description, enabledPlugin: (m.enabledPlugin && m.enabledPlugin.name) || null });
          }
          return out;
        }, []),
      },
      screen: {
        width: sc(function () { return screen.width; }, 0),
        height: sc(function () { return screen.height; }, 0),
        availWidth: sc(function () { return screen.availWidth; }, 0),
        availHeight: sc(function () { return screen.availHeight; }, 0),
        availLeft: sc(function () { return screen.availLeft; }, 0),
        availTop: sc(function () { return screen.availTop; }, 0),
        colorDepth: sc(function () { return screen.colorDepth; }, 0),
        pixelDepth: sc(function () { return screen.pixelDepth; }, 0),
      },
      windowMetrics: {
        devicePixelRatio: sc(function () { return devicePixelRatio; }, 1),
        innerWidth: sc(function () { return innerWidth; }, 0),
        innerHeight: sc(function () { return innerHeight; }, 0),
        outerWidth: sc(function () { return outerWidth; }, 0),
        outerHeight: sc(function () { return outerHeight; }, 0),
        screenX: sc(function () { return screenX; }, 0),
        screenY: sc(function () { return screenY; }, 0),
      },
      document: {
        referrer: sc(function () { return document.referrer; }, ""),
        cookie: sc(function () { return document.cookie; }, ""),
        characterSet: sc(function () { return document.characterSet; }, ""),
        charset: sc(function () { return document.charset; }, ""),
        compatMode: sc(function () { return document.compatMode; }, ""),
        contentType: sc(function () { return document.contentType; }, ""),
        title: sc(function () { return document.title; }, ""),
        domain: sc(function () { return document.domain; }, ""),
        URL: sc(function () { return document.URL; }, ""),
      },
      // 不透明源(about:blank / data:)下直接读 storage 会抛 SecurityError,故用 sc 包裹兜底。
      localStorage: sc(function () { return dumpStorage(window.localStorage); }, {}),
      sessionStorage: sc(function () { return dumpStorage(window.sessionStorage); }, {}),
      fingerprint: {
        canvas: sc(function () { return __fpCanvas(); }, { supported: false }),
        webgl: sc(function () { return __fpWebGL(); }, { supported: false }),
        audio: D.audioFp || { supported: false, pending: true },
        fonts: sc(function () { return __fpFonts(); }, { supported: false }),
        canvasPixels: sc(function () { return __fpCanvasPixels(); }, { supported: false }),
        rtc: sc(function () { return __fpRtc(); }, { supported: false }),
      },
    };
    D.seed = seed;
    return seed;
  };

  return "installed";
})()
