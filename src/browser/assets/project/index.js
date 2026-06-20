// 补环境工程入口(零依赖,只用 Node 内置 vm/fs/path)。
//
// createEnv()        -> 建一个补好浏览器环境的 vm 沙箱(navigator/screen/location/document/
//                       canvas/webgl/audio 等已就位),返回该沙箱对象(即沙箱内的 globalThis)。
// loadScript(s, f)   -> 把站点签名脚本(相对本目录的路径)在沙箱里执行(纯算还原的关键一步)。
// run(s, code)       -> 在沙箱里执行一段代码并返回结果(用于取签名函数 / 调用它)。
//
// 典型用法见 demo.js。
const vm = require("vm");
const fs = require("fs");
const path = require("path");
const { setup } = require("./env.js");

function createEnv(extra) {
  const sandbox = {};
  setup(sandbox);
  if (extra) Object.assign(sandbox, extra);
  vm.createContext(sandbox);
  return sandbox;
}

function loadScript(sandbox, file) {
  const abs = path.resolve(__dirname, file);
  const code = fs.readFileSync(abs, "utf8");
  return vm.runInContext(code, sandbox, { filename: file });
}

function run(sandbox, code) {
  return vm.runInContext(code, sandbox);
}

module.exports = { createEnv, loadScript, run };
