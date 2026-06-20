// 纯算签名 demo:在补好的浏览器环境里(无浏览器)还原站点签名。
//
//   node demo.js
//
// 步骤:
//   1) 本 demo 先打印补环境是否就位(navigator/canvas/webgl/audio 自检)。
//   2) 按 signers.json 列出的 URL,把站点的【签名脚本】下载保存到 ./signer/ 目录(*.js)。
//   3) 重跑 `node demo.js`:会把 ./signer/ 下脚本依次加载进补环境;然后在下方
//      "调用签名函数" 处按站点实际导出名取到签名函数并调用即可(见注释示例)。
const fs = require("fs");
const path = require("path");
const { createEnv, loadScript, run } = require("./index.js");

const signers = safeRead("./signers.json", []);
const targets = safeRead("./targets.json", []);

const sandbox = createEnv();

// —— 补环境自检 ——
console.log("[env] navigator.userAgent =", run(sandbox, "navigator && navigator.userAgent"));
console.log("[env] navigator.platform  =", run(sandbox, "navigator && navigator.platform"));
console.log("[env] screen              =", run(sandbox, "screen && (screen.width + 'x' + screen.height)"));
console.log("[env] canvas.toDataURL len=", run(sandbox, "document.createElement('canvas').toDataURL().length"));
console.log("[env] webgl vendor        =", run(sandbox, "(function(){var g=document.createElement('canvas').getContext('webgl');return g&&g.getParameter(37445);})()"));
console.log("[env] navigator.plugins   =", run(sandbox, "navigator && navigator.plugins ? navigator.plugins.length : 0"), "项");
console.log("[env] RTCPeerConnection   =", run(sandbox, "typeof RTCPeerConnection"));

// —— 加载签名脚本(纯算还原) ——
const signerDir = path.join(__dirname, "signer");
const files = fs.existsSync(signerDir) ? fs.readdirSync(signerDir).filter((f) => f.endsWith(".js")) : [];

if (!files.length) {
  console.log("\n下一步:把下列签名脚本下载到 ./signer/(命名为 *.js)后重跑 `node demo.js`:");
  if (signers.length) {
    signers.forEach((s) => console.log("  - " + (s.url || s)));
  } else {
    console.log("  (signers.json 为空——若目标站点确有签名请求,确保吐环境时已触发它)");
  }
  if (targets.length) {
    console.log("\n参考:已抓到的目标参数真实上线值(用于核对纯算结果):");
    targets.slice(0, 5).forEach((t) => console.log("  - " + t.key + " = " + String(t.value).slice(0, 48)));
  }
  return;
}

console.log("\n[load] 加载 signer/ 下脚本到补环境:");
for (const f of files) {
  console.log("  - " + f);
  try {
    loadScript(sandbox, path.join("signer", f));
  } catch (e) {
    console.log("    ! 加载失败:" + (e && e.message));
  }
}

// —— 调用签名函数(按目标站点实际导出名修改下面这段) ——
//
// 例如抖音 a_bogus(导出名以实际脚本为准):
//   const sign = run(sandbox, "window.byted_acrawler && window.byted_acrawler.sign");
//   const ab = sign({ url: "https://www.douyin.com/aweme/v1/web/aweme/detail/?...", ... });
//   console.log("a_bogus =", ab);
//
// 取不到导出名时,可在沙箱里枚举可疑全局:
//   console.log(run(sandbox, "Object.keys(this).filter(k=>/sign|bogus|acrawler|bdms|sm|window\\./i.test(k))"));
console.log("\n已加载 signer/ 脚本到补环境。请按 signers.json 找到签名函数名,在 demo.js 末尾调用它。");
console.log("可枚举可疑全局:", run(sandbox, "Object.keys(this).filter(function(k){return /sign|bogus|acrawler|bdms|token/i.test(k);})"));

function safeRead(rel, fallback) {
  try {
    return JSON.parse(fs.readFileSync(path.resolve(__dirname, rel), "utf8"));
  } catch (e) {
    return fallback;
  }
}
