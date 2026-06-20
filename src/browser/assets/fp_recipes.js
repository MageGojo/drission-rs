/*
 * drission 指纹采集配方(共享)。被探针(采集 seed)与 verify 快照(Node 回放重算)同时内联,
 * 单一来源,保证「录制时的配方」与「回放/重算时的配方」严格一致——这是 canvas/webgl 指纹
 * 能在 Node 补环境里被忠实回放并逐字段对齐的前提。
 *
 * 仅含函数声明(无 IIFE),供宿主脚本内联后调用:
 *   __fpCanvas()        -> { supported, dataURL }                同步,固定 2D 绘制配方
 *   __fpWebGL()         -> { supported, parameters, unmaskedVendor, unmaskedRenderer, extensions }  同步
 *   __fpAudioAsync(cb)  -> cb({ supported, sampleRate, sum, slice })   异步,OfflineAudioContext 配方
 *   __fpFonts()         -> { supported, detected, widths, hash }   同步,measureText 宽度法字体枚举
 *   __fpCanvasPixels()  -> { supported, width, height, hash, data } 同步,getImageData 像素级 canvas
 *   __fpRtc()           -> { supported, codecsHash, audioCodecs, videoCodecs } 同步,WebRTC 能力
 *   __fpHash(str) / __fpHashBytes(arr) -> 8 位十六进制 FNV-1a(浏览器 / Node 同算)
 *
 * 这些函数只依赖 document.createElement('canvas') / OfflineAudioContext / RTCPeerConnection —— 在浏览器里
 * 走真实实现,在 Node 的补环境(env_template.js 注入的假 DOM)里走回放实现,二者返回值因此一致。
 */

// 32 位 FNV-1a(>>>0 锁 32 位,浏览器与 Node 的 V8/SpiderMonkey 结果一致),输出 8 位十六进制。
function __fpHash(str) {
  str = String(str);
  var h = 0x811c9dc5;
  for (var i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i) & 0xff;
    h = (h + ((h << 1) + (h << 4) + (h << 7) + (h << 8) + (h << 24))) >>> 0;
  }
  return ("0000000" + h.toString(16)).slice(-8);
}

function __fpHashBytes(bytes) {
  var h = 0x811c9dc5;
  var n = bytes ? bytes.length : 0;
  for (var i = 0; i < n; i++) {
    h ^= bytes[i] & 0xff;
    h = (h + ((h << 1) + (h << 4) + (h << 7) + (h << 8) + (h << 24))) >>> 0;
  }
  return ("0000000" + h.toString(16)).slice(-8);
}

function __fpCanvas() {
  try {
    var c = document.createElement("canvas");
    c.width = 280;
    c.height = 60;
    var ctx = c.getContext("2d");
    if (!ctx) return { supported: false, dataURL: "" };
    ctx.textBaseline = "top";
    ctx.font = "14px 'Arial'";
    ctx.textBaseline = "alphabetic";
    ctx.fillStyle = "#f60";
    ctx.fillRect(125, 1, 62, 20);
    ctx.fillStyle = "#069";
    ctx.fillText("drission,补环境🦊", 2, 15);
    ctx.fillStyle = "rgba(102,204,0,0.7)";
    ctx.fillText("drission,补环境🦊", 4, 17);
    return { supported: true, dataURL: c.toDataURL() };
  } catch (e) {
    return { supported: false, dataURL: "", error: String(e && e.message) };
  }
}

function __fpWebGL() {
  try {
    var c = document.createElement("canvas");
    var gl = c.getContext("webgl") || c.getContext("experimental-webgl");
    if (!gl) return { supported: false };
    // 常被指纹脚本读取的 getParameter 枚举(数值常量,避免依赖 gl.* 名)。
    var pnames = [
      7936, 7937, 7938, 35724, // VENDOR RENDERER VERSION SHADING_LANGUAGE_VERSION
      3379, 34076, 34921, 36347, 36348, // MAX_TEXTURE_SIZE MAX_CUBE.. MAX_VERTEX_ATTRIBS MAX_VERTEX_UNIFORM.. MAX_VARYING..
      35660, 35661, 34930, 36349, // MAX_VERTEX_TEXTURE.. MAX_COMBINED.. MAX_TEXTURE_IMAGE.. MAX_FRAGMENT_UNIFORM..
      3386, 33901, 33902, // MAX_VIEWPORT_DIMS ALIASED_POINT_SIZE_RANGE ALIASED_LINE_WIDTH_RANGE
      3410, 3411, 3412, 3413, 3414, 3415, // RED/GREEN/BLUE/ALPHA/DEPTH/STENCIL_BITS
      34047, 34852, // MAX_TEXTURE_MAX_ANISOTROPY_EXT(若有) MAX_DRAW_BUFFERS_WEBGL(若有)
    ];
    var params = {};
    for (var i = 0; i < pnames.length; i++) {
      try {
        var v = gl.getParameter(pnames[i]);
        if (v && typeof v !== "string" && typeof v.length === "number") {
          v = Array.prototype.slice.call(v); // Int32Array / Float32Array -> 普通数组(可 JSON 化)
        }
        if (v !== null && v !== undefined) params[pnames[i]] = v;
      } catch (e) {}
    }
    var uv = null;
    var ur = null;
    try {
      var dbg = gl.getExtension("WEBGL_debug_renderer_info");
      if (dbg) {
        uv = gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL);
        ur = gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
        if (uv != null) params[37445] = uv;
        if (ur != null) params[37446] = ur;
      }
    } catch (e) {}
    var exts = [];
    try {
      exts = gl.getSupportedExtensions() || [];
    } catch (e) {}
    return {
      supported: true,
      parameters: params,
      unmaskedVendor: uv,
      unmaskedRenderer: ur,
      extensions: exts,
    };
  } catch (e) {
    return { supported: false, error: String(e && e.message) };
  }
}

function __fpAudioAsync(cb) {
  var done = false;
  function fin(r) {
    if (done) return;
    done = true;
    try {
      cb(r);
    } catch (e) {}
  }
  try {
    var OAC = (typeof OfflineAudioContext !== "undefined" && OfflineAudioContext) ||
      (typeof webkitOfflineAudioContext !== "undefined" && webkitOfflineAudioContext);
    if (!OAC) {
      fin({ supported: false });
      return;
    }
    var ctx = new OAC(1, 5000, 44100);
    var osc = ctx.createOscillator();
    osc.type = "triangle";
    if (osc.frequency && osc.frequency.value !== undefined) osc.frequency.value = 10000;
    var comp = ctx.createDynamicsCompressor();
    try {
      if (comp.threshold) comp.threshold.value = -50;
      if (comp.knee) comp.knee.value = 40;
      if (comp.ratio) comp.ratio.value = 12;
      if (comp.attack) comp.attack.value = 0;
      if (comp.release) comp.release.value = 0.25;
    } catch (e) {}
    osc.connect(comp);
    comp.connect(ctx.destination);
    osc.start(0);
    ctx.oncomplete = function (ev) {
      try {
        var data = ev.renderedBuffer.getChannelData(0);
        var sum = 0;
        var slice = [];
        for (var i = 4500; i < 5000; i++) {
          sum += Math.abs(data[i]);
          slice.push(data[i]);
        }
        fin({ supported: true, sampleRate: ctx.sampleRate, sum: sum, slice: slice });
      } catch (e) {
        fin({ supported: false, error: String(e && e.message) });
      }
    };
    var p = ctx.startRendering();
    if (p && typeof p.then === "function") {
      p.then(function (buf) {
        try {
          var data = buf.getChannelData(0);
          var sum = 0;
          var slice = [];
          for (var i = 4500; i < 5000; i++) {
            sum += Math.abs(data[i]);
            slice.push(data[i]);
          }
          fin({ supported: true, sampleRate: ctx.sampleRate || 44100, sum: sum, slice: slice });
        } catch (e) {
          fin({ supported: false, error: String(e && e.message) });
        }
      });
    }
  } catch (e) {
    fin({ supported: false, error: String(e && e.message) });
  }
}

// 字体枚举:measureText 宽度法。对每个候选字体,用 `<candidate>,<generic>` 量同一串的宽度,
// 与该 generic 基线不同即判定已安装。`widths` 按 ctx.font 串记录,供 Node 回放(env.js measureText 查表)
// 复算出相同的 detected/hash。候选/串/字号在浏览器与 Node 完全一致,保证录制==回放。
function __fpFonts() {
  try {
    var bases = ["monospace", "sans-serif", "serif"];
    var probe = "mmmmmmmmmmlli水墨0Oo😀WiQ";
    var size = "72px ";
    var candidates = [
      "Arial", "Arial Black", "Arial Narrow", "Arial Unicode MS", "Calibri", "Cambria",
      "Comic Sans MS", "Consolas", "Courier", "Courier New", "Georgia", "Helvetica",
      "Helvetica Neue", "Impact", "Lucida Console", "Lucida Grande", "Menlo", "Microsoft Sans Serif",
      "Monaco", "MS Gothic", "MS PGothic", "MS Sans Serif", "Palatino", "Palatino Linotype",
      "Segoe UI", "Tahoma", "Times", "Times New Roman", "Trebuchet MS", "Verdana", "Wingdings",
      "PingFang SC", "Hiragino Sans GB", "STHeiti", "Songti SC", "Heiti SC", "Apple Color Emoji",
      "Noto Sans CJK SC", "Source Han Sans", "WenQuanYi Micro Hei", "DejaVu Sans", "Liberation Sans",
      "Ubuntu", "Roboto", "Microsoft YaHei", "SimSun", "SimHei", "KaiTi",
    ];
    var c = document.createElement("canvas");
    var ctx = c.getContext("2d");
    if (!ctx) return { supported: false };
    var widths = {};
    function measure(font) {
      ctx.font = font;
      var w = ctx.measureText(probe).width;
      widths[font] = w;
      return w;
    }
    var baseW = {};
    for (var b = 0; b < bases.length; b++) baseW[bases[b]] = measure(size + bases[b]);
    var detected = [];
    for (var i = 0; i < candidates.length; i++) {
      var present = false;
      for (var b2 = 0; b2 < bases.length; b2++) {
        var w = measure(size + "'" + candidates[i] + "'," + bases[b2]);
        if (w !== baseW[bases[b2]]) present = true;
      }
      if (present) detected.push(candidates[i]);
    }
    return {
      supported: true,
      detected: detected,
      widths: widths,
      hash: __fpHash(JSON.stringify(detected) + "|" + JSON.stringify(widths)),
    };
  } catch (e) {
    return { supported: false, error: String(e && e.message) };
  }
}

// 像素级 canvas 指纹:固定 2D 绘制 -> getImageData 读字节 -> 字节哈希(很多指纹库读 getImageData 而非 toDataURL)。
// 紧凑画布 96x32(12288 字节)控制 env.js 体积;`data` 为字节 base64,供 env.js 回放(尺寸匹配时原样返回)。
function __fpCanvasPixels() {
  try {
    var W = 96, H = 32;
    var c = document.createElement("canvas");
    c.width = W;
    c.height = H;
    var ctx = c.getContext("2d");
    if (!ctx) return { supported: false };
    var g = ctx.createLinearGradient(0, 0, W, H);
    g.addColorStop(0, "#f60");
    g.addColorStop(0.5, "#069");
    g.addColorStop(1, "#0c6");
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, W, H);
    ctx.textBaseline = "top";
    ctx.font = "14px 'Arial'";
    ctx.fillStyle = "rgba(20,30,40,0.85)";
    ctx.fillText("drission px🦊", 2, 2);
    ctx.fillStyle = "rgba(255,255,255,0.6)";
    ctx.beginPath();
    ctx.arc(70, 16, 12, 0, Math.PI * 1.7);
    ctx.fill();
    var im = ctx.getImageData(0, 0, W, H);
    var bytes = im.data;
    var hash = __fpHashBytes(bytes);
    var bin = "";
    for (var i = 0; i < bytes.length; i += 8192) {
      bin += String.fromCharCode.apply(null, Array.prototype.slice.call(bytes, i, i + 8192));
    }
    var data = "";
    try { data = (typeof btoa === "function" ? btoa(bin) : ""); } catch (e) { data = ""; }
    return { supported: true, width: W, height: H, hash: hash, data: data };
  } catch (e) {
    return { supported: false, error: String(e && e.message) };
  }
}

// WebRTC:RTCPeerConnection 是否可用 + 收发编解码能力概要。默认 block_webrtc=true 时 supported:false
// (env.js 回放即"RTCPeerConnection 仍 undefined",正是要补的一致性)。可用时记录 codecs 供回放复算 hash。
function __fpRtc() {
  try {
    var RPC = (typeof RTCPeerConnection !== "undefined" && RTCPeerConnection) ||
      (typeof webkitRTCPeerConnection !== "undefined" && webkitRTCPeerConnection) ||
      (typeof mozRTCPeerConnection !== "undefined" && mozRTCPeerConnection);
    if (!RPC) return { supported: false, codecsHash: "", audioCodecs: [], videoCodecs: [] };
    function caps(kind) {
      var out = [];
      try {
        if (typeof RTCRtpReceiver !== "undefined" && RTCRtpReceiver.getCapabilities) {
          var cp = RTCRtpReceiver.getCapabilities(kind);
          if (cp && cp.codecs) for (var i = 0; i < cp.codecs.length; i++) out.push(cp.codecs[i].mimeType);
        }
      } catch (e) {}
      return out;
    }
    var a = caps("audio"), v = caps("video");
    return {
      supported: true,
      audioCodecs: a,
      videoCodecs: v,
      codecsHash: __fpHash(a.join(",") + "|" + v.join(",")),
    };
  } catch (e) {
    return { supported: false, error: String(e && e.message), audioCodecs: [], videoCodecs: [] };
  }
}
