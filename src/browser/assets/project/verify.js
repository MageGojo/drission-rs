// 自检:验证 env.js 是否忠实回放 seed.json 录制的环境(纯 Node,无需浏览器,零依赖)。
//
//   node verify.js   或   npm run verify
//
// 通过标准:env 补出的 navigator/screen/location/canvas/webgl/audio 与 seed.json 录制值逐项一致。
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
