/*
 * drission 指纹采集配方(共享)。被探针(采集 seed)与 verify 快照(Node 回放重算)同时内联,
 * 单一来源,保证「录制时的配方」与「回放/重算时的配方」严格一致——这是 canvas/webgl 指纹
 * 能在 Node 补环境里被忠实回放并逐字段对齐的前提。
 *
 * 仅含函数声明(无 IIFE),供宿主脚本内联后调用:
 *   __fpCanvas()        -> { supported, dataURL }                同步,固定 2D 绘制配方
 *   __fpWebGL()         -> { supported, parameters, unmaskedVendor, unmaskedRenderer, extensions }  同步
 *   __fpAudioAsync(cb)  -> cb({ supported, sampleRate, sum, slice })   异步,OfflineAudioContext 配方
 *
 * 这些函数只依赖 document.createElement('canvas') / OfflineAudioContext —— 在浏览器里走真实实现,
 * 在 Node 的补环境(env_template.js 注入的假 DOM)里走回放实现,二者返回值因此一致。
 */

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
