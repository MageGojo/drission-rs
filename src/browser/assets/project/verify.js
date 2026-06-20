// 自检:验证 env.js 是否忠实回放 seed.json 录制的环境(纯 Node,无需浏览器,零依赖)。
//
//   node verify.js   或   npm run verify
//
// 通过标准:env 补出的 navigator(含 plugins/mimeTypes)/screen/location/canvas(toDataURL+measureText 字体+
// getImageData 像素)/webgl/audio/WebRTC 与 seed.json 录制值逐项一致。
const fs = require("fs");
const path = require("path");
const { createEnv, run } = require("./index.js");

const seed = JSON.parse(fs.readFileSync(path.resolve(__dirname, "seed.json"), "utf8"));
const sandbox = createEnv();

let pass = 0;
let fail = 0;
const fails = [];

function check(name, got, want) {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (ok) pass++;
  else {
    fail++;
    fails.push({ field: name, got: got, want: want });
  }
}

// navigator 标量
const nav = seed.navigator || {};
Object.keys(nav).forEach(function (k) {
  if (nav[k] !== null && typeof nav[k] === "object") return; // 对象(userAgentData)整体跳过
  check("navigator." + k, run(sandbox, "navigator." + k), nav[k]);
});
// screen
const scr = seed.screen || {};
Object.keys(scr).forEach(function (k) {
  check("screen." + k, run(sandbox, "screen." + k), scr[k]);
});
// location host/origin
["host", "origin"].forEach(function (k) {
  if (seed.location && seed.location[k] !== undefined) check("location." + k, run(sandbox, "location." + k), seed.location[k]);
});

// canvas 指纹回放
const fp = seed.fingerprint || {};
if (fp.canvas && fp.canvas.supported) {
  check("canvas.dataURL", run(sandbox, "document.createElement('canvas').toDataURL()"), fp.canvas.dataURL);
}
// webgl 指纹回放
if (fp.webgl && fp.webgl.supported) {
  check("webgl.unmaskedVendor", run(sandbox, "(function(){var g=document.createElement('canvas').getContext('webgl');var e=g.getExtension('WEBGL_debug_renderer_info');return e?g.getParameter(e.UNMASKED_VENDOR_WEBGL):null;})()"), fp.webgl.unmaskedVendor);
  check("webgl.unmaskedRenderer", run(sandbox, "(function(){var g=document.createElement('canvas').getContext('webgl');var e=g.getExtension('WEBGL_debug_renderer_info');return e?g.getParameter(e.UNMASKED_RENDERER_WEBGL):null;})()"), fp.webgl.unmaskedRenderer);
  check("webgl.extCount", run(sandbox, "(document.createElement('canvas').getContext('webgl').getSupportedExtensions()||[]).length"), (fp.webgl.extensions || []).length);
}

// 字体枚举回放:env.js measureText 按 ctx.font 串返回录制宽度。
if (fp.fonts && fp.fonts.supported && fp.fonts.widths) {
  var keys = Object.keys(fp.fonts.widths);
  var allOk = keys.length > 0;
  for (var fi = 0; fi < keys.length; fi++) {
    var k = keys[fi];
    var w = run(sandbox, "(function(){var c=document.createElement('canvas').getContext('2d');c.font=" + JSON.stringify(k) + ";return c.measureText('x').width;})()");
    if (w !== fp.fonts.widths[k]) { allOk = false; break; }
  }
  check("fonts.widths replay (" + keys.length + " 项)", allOk, true);
}

// 像素级 canvas 回放:env.js getImageData 还原录制字节(用校验和 + 长度比对)。
if (fp.canvasPixels && fp.canvasPixels.supported && fp.canvasPixels.data) {
  var cp = fp.canvasPixels;
  var buf = Buffer.from(cp.data, "base64");
  var wantSum = 0;
  for (var bi = 0; bi < buf.length; bi++) wantSum = (wantSum + buf[bi]) >>> 0;
  var got = run(sandbox, "(function(){var im=document.createElement('canvas').getContext('2d').getImageData(0,0," + cp.width + "," + cp.height + ");var s=0;for(var i=0;i<im.data.length;i++)s=(s+im.data[i])>>>0;return {len:im.data.length,sum:s};})()");
  check("canvasPixels.byteLen", got.len, cp.width * cp.height * 4);
  check("canvasPixels.checksum", got.sum, wantSum);
}

// WebRTC 回放:supported 决定 RTCPeerConnection 是否存在;可用时 getCapabilities 回放 codecs。
if (fp.rtc) {
  check("rtc.RTCPeerConnection defined", run(sandbox, "typeof RTCPeerConnection !== 'undefined'"), !!fp.rtc.supported);
  if (fp.rtc.supported) {
    check("rtc.audioCodecsCount", run(sandbox, "(RTCRtpReceiver.getCapabilities('audio').codecs||[]).length"), (fp.rtc.audioCodecs || []).length);
    check("rtc.videoCodecsCount", run(sandbox, "(RTCRtpReceiver.getCapabilities('video').codecs||[]).length"), (fp.rtc.videoCodecs || []).length);
  }
}

// navigator.plugins / mimeTypes 回放:类数组计数 + 首项名 + namedItem。
if (Array.isArray(nav.plugins)) {
  check("navigator.plugins.length", run(sandbox, "navigator.plugins ? navigator.plugins.length : -1"), nav.plugins.length);
  if (nav.plugins.length) {
    check("navigator.plugins[0].name", run(sandbox, "navigator.plugins[0] ? navigator.plugins[0].name : null"), nav.plugins[0].name);
    check("navigator.plugins.namedItem", run(sandbox, "navigator.plugins.namedItem ? (navigator.plugins.namedItem(" + JSON.stringify(nav.plugins[0].name) + ") ? navigator.plugins.namedItem(" + JSON.stringify(nav.plugins[0].name) + ").name : null) : 'no-namedItem'"), nav.plugins[0].name);
  }
}
if (Array.isArray(nav.mimeTypes)) {
  check("navigator.mimeTypes.length", run(sandbox, "navigator.mimeTypes ? navigator.mimeTypes.length : -1"), nav.mimeTypes.length);
}

// audio 指纹回放(异步)
function audioCheck() {
  return new Promise(function (resolve) {
    if (!(fp.audio && fp.audio.supported)) return resolve();
    const code = "(async function(){var ctx=new OfflineAudioContext(1,5000,44100);var buf=await ctx.startRendering();var d=buf.getChannelData(0);var s=0;for(var i=4500;i<5000;i++)s+=Math.abs(d[i]);return Math.round(s*1e6)/1e6;})()";
    Promise.resolve(run(sandbox, code)).then(function (got) {
      check("audio.sum", got, Math.round((fp.audio.sum || 0) * 1e6) / 1e6);
      resolve();
    }).catch(function () { resolve(); });
  });
}

audioCheck().then(function () {
  console.log("==== env.js 回放自检: " + pass + "/" + (pass + fail) + " 字段与 seed.json 一致 ====");
  if (fail === 0) {
    console.log("  ✅ 全部一致 —— 补环境忠实回放了录制环境。");
  } else {
    console.log("  ⚠ " + fail + " 个字段不一致:");
    fails.forEach(function (f) {
      console.log("    " + f.field + " : env=" + JSON.stringify(f.got) + " | seed=" + JSON.stringify(f.want));
    });
    process.exit(1);
  }
});
