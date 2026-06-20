// 自动生成(drission dump_env):基于真实浏览器采集的补环境模块(含 canvas/webgl/audio 指纹回放)。
//
// 用法一(vm 沙箱,推荐 —— 纯算签名的标准姿势,且不受 Node 内置 navigator 干扰):
//   const vm = require('vm'); const { setup } = require('./env.js');
//   const sandbox = {}; setup(sandbox); vm.createContext(sandbox);
//   vm.runInContext(签名脚本源码, sandbox); // 然后调用其签名函数
// 用法二(直接挂当前全局,尽力用 defineProperty 覆盖 Node 内置 navigator):require('./env.js')
//
// 回放范围:navigator(含 plugins / mimeTypes 类数组)/ screen / location / window 度量 /
// document(cookie + createElement)/ localStorage / sessionStorage / canvas 2D(toDataURL 录制值 +
// measureText 字体宽度 + getImageData 像素字节回放)/ WebGL(getParameter/getExtension/getSupportedExtensions)/
// AudioContext·OfflineAudioContext(渲染缓冲录制切片)/ WebRTC(supported 时 RTCPeerConnection + getCapabilities 回放)。
// 注意:罕见 webgl 调用等高强度点可能仍需按目标站点按需补全。
var __SEED__ = __SEED_JSON__;

(function () {
  // 纯 JS base64(vm 沙箱里可能没有 Node 的 Buffer,故不依赖 Buffer)。
  var b64 = (function () {
    var T = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    function btoa(s) {
      s = String(s);
      var out = "", i = 0;
      while (i < s.length) {
        var c1 = s.charCodeAt(i++), c2 = s.charCodeAt(i++), c3 = s.charCodeAt(i++);
        var e1 = c1 >> 2, e2 = ((c1 & 3) << 4) | (c2 >> 4);
        var e3 = isNaN(c2) ? 64 : (((c2 & 15) << 2) | (c3 >> 6)), e4 = isNaN(c3) ? 64 : (c3 & 63);
        out += T.charAt(e1) + T.charAt(e2) + T.charAt(e3) + T.charAt(e4);
      }
      return out;
    }
    function atob(s) {
      s = String(s).replace(/[^A-Za-z0-9+/=]/g, "");
      var out = "", i = 0;
      while (i < s.length) {
        var e1 = T.indexOf(s.charAt(i++)), e2 = T.indexOf(s.charAt(i++));
        var e3 = T.indexOf(s.charAt(i++)), e4 = T.indexOf(s.charAt(i++));
        var c1 = (e1 << 2) | (e2 >> 4), c2 = ((e2 & 15) << 4) | (e3 >> 2), c3 = ((e3 & 3) << 6) | e4;
        out += String.fromCharCode(c1);
        if (e3 !== 64) out += String.fromCharCode(c2);
        if (e4 !== 64) out += String.fromCharCode(c3);
      }
      return out;
    }
    return { atob: atob, btoa: btoa };
  })();

  function defprop(o, k, v) {
    try {
      Object.defineProperty(o, k, { value: v, configurable: true, writable: true });
    } catch (e) {
      try { o[k] = v; } catch (e2) {}
    }
  }

  // 录制的字体宽度(measureText 回放查表)与像素字节(getImageData 回放还原),供 makeCtx2D 闭包用。
  var __fonts = ((__SEED__.fingerprint && __SEED__.fingerprint.fonts) || {}).widths || {};
  var __cpix = (__SEED__.fingerprint && __SEED__.fingerprint.canvasPixels) || null;
  var __cpixBytes; // 懒解码:undefined=未尝试 / null=无 / Uint8ClampedArray
  function pixelBytes() {
    if (__cpixBytes !== undefined) return __cpixBytes;
    __cpixBytes = null;
    try {
      if (__cpix && __cpix.data) {
        var bin = b64.atob(__cpix.data);
        var arr = new Uint8ClampedArray(bin.length);
        for (var i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
        __cpixBytes = arr;
      }
    } catch (e) {}
    return __cpixBytes;
  }

  function makeCtx2D() {
    var noop = function () {};
    var ctx = {
      canvas: null,
      font: "10px sans-serif",
      save: noop, restore: noop, beginPath: noop, closePath: noop,
      fillRect: noop, strokeRect: noop, clearRect: noop,
      fillText: noop, strokeText: noop,
      // 回放录制宽度(按 ctx.font 串查表);未录制的字体串回退旧估算。
      measureText: function (t) {
        var f = this.font;
        if (f !== undefined && Object.prototype.hasOwnProperty.call(__fonts, f)) return { width: __fonts[f] };
        return { width: String(t == null ? "" : t).length * 7 };
      },
      arc: noop, arcTo: noop, ellipse: noop, rect: noop, roundRect: noop,
      moveTo: noop, lineTo: noop, bezierCurveTo: noop, quadraticCurveTo: noop,
      fill: noop, stroke: noop, clip: noop,
      scale: noop, rotate: noop, translate: noop, transform: noop, setTransform: noop, resetTransform: noop,
      drawImage: noop, putImageData: noop, setLineDash: noop, getLineDash: function () { return []; },
      createLinearGradient: function () { return { addColorStop: noop }; },
      createRadialGradient: function () { return { addColorStop: noop }; },
      createPattern: function () { return null; },
      // 回放录制像素(尺寸与录制一致时原样返回字节);否则回退全 0(其它绘制/尺寸不影响)。
      getImageData: function (x, y, w, h) {
        var n = (w * h * 4) | 0;
        var pb = pixelBytes();
        if (pb && __cpix && w === __cpix.width && h === __cpix.height && pb.length === n) {
          return { data: pb, width: w, height: h };
        }
        return { data: new Uint8ClampedArray(n > 0 ? n : 4), width: w, height: h };
      },
      isPointInPath: function () { return false; },
      isPointInStroke: function () { return false; },
    };
    return ctx;
  }

  function makeWebGL(fp) {
    fp = fp || {};
    var params = fp.parameters || {};
    function param(p) {
      var v = params[p];
      if (v === undefined) v = params[String(p)];
      return v === undefined ? null : v;
    }
    var dbgExt = { UNMASKED_VENDOR_WEBGL: 37445, UNMASKED_RENDERER_WEBGL: 37446 };
    var gl = {
      getParameter: function (p) { return param(p); },
      getExtension: function (name) {
        if (name === "WEBGL_debug_renderer_info") return dbgExt;
        return (fp.extensions || []).indexOf(name) >= 0 ? {} : null;
      },
      getSupportedExtensions: function () { return (fp.extensions || []).slice(); },
      getContextAttributes: function () {
        return { alpha: true, antialias: true, depth: true, failIfMajorPerformanceCaveat: false, powerPreference: "default", premultipliedAlpha: true, preserveDrawingBuffer: false, stencil: false };
      },
      getShaderPrecisionFormat: function () { return { rangeMin: 127, rangeMax: 127, precision: 23 }; },
      getProgramParameter: function () { return true; },
      getShaderParameter: function () { return true; },
    };
    var noop = function () { return null; };
    [
      "createBuffer", "bindBuffer", "bufferData", "createProgram", "createShader", "shaderSource",
      "compileShader", "attachShader", "linkProgram", "useProgram", "getAttribLocation",
      "getUniformLocation", "enableVertexAttribArray", "vertexAttribPointer", "uniform1f",
      "uniform2f", "uniform4fv", "drawArrays", "drawElements", "viewport", "clearColor", "clear",
      "enable", "disable", "depthFunc", "deleteShader", "deleteProgram", "deleteBuffer",
      "activeTexture", "bindTexture", "createTexture", "texParameteri", "texImage2D", "pixelStorei",
      "framebufferTexture2D", "readPixels", "finish", "flush", "getError",
    ].forEach(function (m) { if (!gl[m]) gl[m] = noop; });
    return gl;
  }

  function makeCanvasEl() {
    var fp = (__SEED__.fingerprint && __SEED__.fingerprint.canvas) || {};
    var wfp = (__SEED__.fingerprint && __SEED__.fingerprint.webgl) || {};
    var ctx2d = makeCtx2D();
    var glctx = makeWebGL(wfp);
    var el = {
      nodeName: "CANVAS", tagName: "CANVAS", width: 300, height: 150, style: {},
      getContext: function (type) {
        if (type === "2d") return ctx2d;
        if (type === "webgl" || type === "experimental-webgl" || type === "webgl2" || type === "moz-webgl") return wfp.supported === false ? null : glctx;
        return null;
      },
      toDataURL: function () { return fp.dataURL || "data:,"; },
      toBlob: function (cb) { try { cb({ size: (fp.dataURL || "").length, type: "image/png" }); } catch (e) {} },
      getBoundingClientRect: function () { return { x: 0, y: 0, top: 0, left: 0, right: this.width, bottom: this.height, width: this.width, height: this.height }; },
      setAttribute: function () {}, getAttribute: function () { return null; },
      addEventListener: function () {}, removeEventListener: function () {}, appendChild: function () {},
    };
    ctx2d.canvas = el;
    return el;
  }

  function makeAudioBuffer(length, rate) {
    var fp = (__SEED__.fingerprint && __SEED__.fingerprint.audio) || {};
    var len = length || 5000;
    var arr = new Float32Array(len);
    var slice = fp.slice || [];
    var start = len >= 5000 ? 4500 : 0; // 录制时取 [4500,5000),回放放回同一区间
    for (var i = 0; i < slice.length && start + i < len; i++) arr[start + i] = slice[i];
    return {
      length: len, sampleRate: rate || fp.sampleRate || 44100, numberOfChannels: 1, duration: len / (rate || 44100),
      getChannelData: function () { return arr; },
      copyFromChannel: function () {}, copyToChannel: function () {},
    };
  }

  function audioNode() {
    function p() { return { value: 0, defaultValue: 0, minValue: -3.4e38, maxValue: 3.4e38, setValueAtTime: function () { return this; }, linearRampToValueAtTime: function () { return this; }, exponentialRampToValueAtTime: function () { return this; }, setTargetAtTime: function () { return this; } }; }
    return {
      connect: function (dst) { return dst || {}; }, disconnect: function () {},
      start: function () {}, stop: function () {},
      type: "triangle",
      frequency: p(), detune: p(), gain: p(),
      threshold: p(), knee: p(), ratio: p(), attack: p(), release: p(), reduction: 0,
    };
  }

  function makeOfflineAudioCtx() {
    function Ctx(numCh, length, rate) {
      if (typeof numCh === "object" && numCh) { length = numCh.length; rate = numCh.sampleRate; numCh = numCh.numberOfChannels; }
      this.sampleRate = rate || 44100;
      this.length = length || 5000;
      this.numberOfChannels = numCh || 1;
      this.currentTime = 0;
      this.state = "suspended";
      this.destination = audioNode();
      this.oncomplete = null;
    }
    Ctx.prototype.createOscillator = audioNode;
    Ctx.prototype.createDynamicsCompressor = audioNode;
    Ctx.prototype.createGain = audioNode;
    Ctx.prototype.createAnalyser = function () { var n = audioNode(); n.frequencyBinCount = 1024; n.getFloatFrequencyData = function () {}; return n; };
    Ctx.prototype.createBiquadFilter = audioNode;
    Ctx.prototype.createScriptProcessor = audioNode;
    Ctx.prototype.createBuffer = function (ch, len, rate) { return makeAudioBuffer(len, rate); };
    Ctx.prototype.createBufferSource = function () { var n = audioNode(); n.buffer = null; return n; };
    Ctx.prototype.startRendering = function () {
      var self = this;
      var buf = makeAudioBuffer(this.length, this.sampleRate);
      this.state = "running";
      // oncomplete 可能在 startRendering 之后才赋值,用微任务保证已就位(vm 无 setTimeout,故用 Promise)。
      Promise.resolve().then(function () { try { if (self.oncomplete) self.oncomplete({ renderedBuffer: buf }); } catch (e) {} });
      return Promise.resolve(buf);
    };
    Ctx.prototype.suspend = function () { return Promise.resolve(); };
    Ctx.prototype.resume = function () { this.state = "running"; return Promise.resolve(); };
    return Ctx;
  }

  function makeOnlineAudioCtx() {
    var fp = (__SEED__.fingerprint && __SEED__.fingerprint.audio) || {};
    function Ctx() {
      this.sampleRate = fp.sampleRate || 44100;
      this.currentTime = 0;
      this.state = "running";
      this.baseLatency = 0.005;
      this.destination = (function () { var n = audioNode(); n.channelCount = 2; n.maxChannelCount = 2; return n; })();
    }
    Ctx.prototype.createOscillator = audioNode;
    Ctx.prototype.createDynamicsCompressor = audioNode;
    Ctx.prototype.createGain = audioNode;
    Ctx.prototype.createAnalyser = function () { var n = audioNode(); n.frequencyBinCount = 1024; n.getFloatFrequencyData = function () {}; return n; };
    Ctx.prototype.createBuffer = function (ch, len, rate) { return makeAudioBuffer(len, rate); };
    Ctx.prototype.createBufferSource = function () { var n = audioNode(); n.buffer = null; return n; };
    Ctx.prototype.close = function () { return Promise.resolve(); };
    Ctx.prototype.resume = function () { return Promise.resolve(); };
    Ctx.prototype.suspend = function () { return Promise.resolve(); };
    return Ctx;
  }

  function makeStorage(seedObj) {
    var data = {};
    if (seedObj) for (var k in seedObj) if (Object.prototype.hasOwnProperty.call(seedObj, k)) data[k] = String(seedObj[k]);
    return {
      getItem: function (k) { return Object.prototype.hasOwnProperty.call(data, k) ? data[k] : null; },
      setItem: function (k, v) { data[k] = String(v); },
      removeItem: function (k) { delete data[k]; },
      clear: function () { data = {}; },
      key: function (i) { return Object.keys(data)[i] === undefined ? null : Object.keys(data)[i]; },
      get length() { return Object.keys(data).length; },
    };
  }

  // 类数组(PluginArray / MimeTypeArray 回放):length + 下标 + item/namedItem/refresh。
  function arrayLike(items, nameKey) {
    var o = {};
    for (var i = 0; i < items.length; i++) o[i] = items[i];
    o.length = items.length;
    o.item = function (i) { return this[i] === undefined ? null : this[i]; };
    o.namedItem = function (n) { for (var i = 0; i < this.length; i++) { if (this[i] && this[i][nameKey] === n) return this[i]; } return null; };
    o.refresh = function () {};
    return o;
  }

  // navigator.plugins / navigator.mimeTypes 回放为类数组并交叉链接 enabledPlugin。
  function installNavigatorPlugins(nav) {
    if (!nav || typeof nav !== "object") return;
    var rawP = Array.isArray(nav.plugins) ? nav.plugins : null;
    var rawM = Array.isArray(nav.mimeTypes) ? nav.mimeTypes : null;
    if (!rawP && !rawM) return;
    var pluginObjs = (rawP || []).map(function (p) {
      var mimes = (p.mimeTypes || []).map(function (m) { return { type: m.type, suffixes: m.suffixes, description: m.description, enabledPlugin: null }; });
      var po = arrayLike(mimes, "type");
      po.name = p.name; po.filename = p.filename; po.description = p.description;
      return po;
    });
    var mimeObjs = (rawM || []).map(function (m) { return { type: m.type, suffixes: m.suffixes, description: m.description, enabledPlugin: null }; });
    for (var pi = 0; pi < pluginObjs.length; pi++) {
      var pl = pluginObjs[pi];
      for (var mi = 0; mi < pl.length; mi++) {
        pl[mi].enabledPlugin = pl;
        var t = pl[mi].type;
        for (var mj = 0; mj < mimeObjs.length; mj++) if (mimeObjs[mj].type === t && !mimeObjs[mj].enabledPlugin) mimeObjs[mj].enabledPlugin = pl;
      }
    }
    try { defprop(nav, "plugins", arrayLike(pluginObjs, "name")); } catch (e) {}
    try { defprop(nav, "mimeTypes", arrayLike(mimeObjs, "type")); } catch (e) {}
  }

  // WebRTC 回放:supported 才装最小 RTCPeerConnection + RTCRtp*.getCapabilities(回放录制 codecs);
  // 否则不定义,保持 undefined —— 与默认 block_webrtc 浏览器侧一致(__fpRtc 两侧都 supported:false)。
  function installWebRtc(g) {
    var rtc = (__SEED__.fingerprint && __SEED__.fingerprint.rtc) || {};
    if (!rtc.supported) return;
    var RTC = function RTCPeerConnection() { this.localDescription = null; this.remoteDescription = null; this.iceConnectionState = "new"; this.connectionState = "new"; };
    RTC.prototype.createDataChannel = function () { return { close: function () {} }; };
    RTC.prototype.createOffer = function () { return Promise.resolve({ type: "offer", sdp: "" }); };
    RTC.prototype.createAnswer = function () { return Promise.resolve({ type: "answer", sdp: "" }); };
    RTC.prototype.setLocalDescription = function () { return Promise.resolve(); };
    RTC.prototype.setRemoteDescription = function () { return Promise.resolve(); };
    RTC.prototype.addIceCandidate = function () { return Promise.resolve(); };
    RTC.prototype.addEventListener = function () {};
    RTC.prototype.removeEventListener = function () {};
    RTC.prototype.getStats = function () { return Promise.resolve(typeof Map !== "undefined" ? new Map() : {}); };
    RTC.prototype.close = function () {};
    g.RTCPeerConnection = RTC;
    g.webkitRTCPeerConnection = RTC;
    function caps(kind) {
      var list = (kind === "video" ? rtc.videoCodecs : rtc.audioCodecs) || [];
      return { codecs: list.map(function (m) { return { mimeType: m }; }), headerExtensions: [] };
    }
    var Recv = function RTCRtpReceiver() {}; Recv.getCapabilities = caps;
    var Send = function RTCRtpSender() {}; Send.getCapabilities = caps;
    g.RTCRtpReceiver = Recv;
    g.RTCRtpSender = Send;
  }

  function installDocument(g) {
    var sd = __SEED__.document || {};
    var doc = g.document && typeof g.document === "object" ? g.document : {};
    doc.cookie = sd.cookie || "";
    ["referrer", "characterSet", "charset", "compatMode", "contentType", "title", "domain", "URL"].forEach(function (k) {
      if (sd[k] !== undefined) doc[k] = sd[k];
    });
    doc.createElement = function (tag) {
      tag = String(tag || "").toLowerCase();
      if (tag === "canvas") return makeCanvasEl();
      return { nodeName: tag.toUpperCase(), tagName: tag.toUpperCase(), style: {}, setAttribute: function () {}, getAttribute: function () { return null; }, getContext: function () { return null; }, appendChild: function () {}, addEventListener: function () {}, removeEventListener: function () {} };
    };
    doc.createElementNS = function (ns, tag) { return doc.createElement(tag); };
    doc.getElementById = doc.getElementById || function () { return null; };
    doc.getElementsByTagName = doc.getElementsByTagName || function () { return []; };
    doc.getElementsByClassName = doc.getElementsByClassName || function () { return []; };
    doc.querySelector = doc.querySelector || function () { return null; };
    doc.querySelectorAll = doc.querySelectorAll || function () { return []; };
    doc.addEventListener = doc.addEventListener || function () {};
    doc.removeEventListener = doc.removeEventListener || function () {};
    doc.documentElement = doc.documentElement || { style: {}, clientWidth: (__SEED__.windowMetrics || {}).innerWidth || 0, clientHeight: (__SEED__.windowMetrics || {}).innerHeight || 0 };
    doc.body = doc.body || { style: {}, appendChild: function () {}, removeChild: function () {} };
    doc.head = doc.head || { appendChild: function () {} };
    g.document = doc;
  }

  function setup(g) {
    g.window = g; g.self = g; g.globalThis = g; g.top = g; g.parent = g; g.frames = g;

    try { defprop(g, "navigator", __SEED__.navigator); } catch (e) { try { g.navigator = __SEED__.navigator; } catch (e2) {} }
    try { installNavigatorPlugins(g.navigator); } catch (e) {}
    try { defprop(g, "screen", __SEED__.screen); } catch (e) { g.screen = __SEED__.screen; }
    g.location = __SEED__.location || {};

    var wm = __SEED__.windowMetrics || {};
    ["devicePixelRatio", "innerWidth", "innerHeight", "outerWidth", "outerHeight", "screenX", "screenY"].forEach(function (k) {
      if (wm[k] !== undefined) { try { g[k] = wm[k]; } catch (e) {} }
    });
    if (g.pageXOffset === undefined) g.pageXOffset = 0;
    if (g.pageYOffset === undefined) g.pageYOffset = 0;

    installDocument(g);

    try { g.localStorage = makeStorage(__SEED__.localStorage); } catch (e) {}
    try { g.sessionStorage = makeStorage(__SEED__.sessionStorage); } catch (e) {}

    g.OfflineAudioContext = makeOfflineAudioCtx();
    g.webkitOfflineAudioContext = g.OfflineAudioContext;
    g.AudioContext = makeOnlineAudioCtx();
    g.webkitAudioContext = g.AudioContext;

    installWebRtc(g);

    // 让 instanceof / 原型探测不致崩溃(stub 构造器)。
    ["HTMLCanvasElement", "HTMLElement", "Element", "Node", "WebGLRenderingContext", "WebGL2RenderingContext", "CanvasRenderingContext2D"].forEach(function (n) {
      if (g[n] === undefined) { var C = function () {}; C.prototype = {}; g[n] = C; }
    });

    if (typeof g.atob !== "function") g.atob = b64.atob;
    if (typeof g.btoa !== "function") g.btoa = b64.btoa;

    return g;
  }

  try { setup(globalThis); } catch (e) {}
  if (typeof module !== "undefined" && module.exports) {
    module.exports = { seed: __SEED__, setup: setup };
  }
})();
