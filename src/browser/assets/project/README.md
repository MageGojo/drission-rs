# __PKG_NAME__

由 [drission](../) 的 `tab.dump_env()` 一键导出的**补环境工程**:在 Node 里**纯算**还原浏览器签名,
**无需浏览器、零依赖**(只用 Node 内置 `vm`/`fs`/`path`)。

## 目录

| 文件 | 说明 |
|---|---|
| `env.js` | 补环境模块(`setup(sandbox)` 把 navigator/screen/location/document/canvas/webgl/audio 等装进沙箱) |
| `index.js` | 入口:`createEnv()` 建补好环境的 vm 沙箱、`loadScript()` 加载站点签名脚本、`run()` 在沙箱执行 |
| `demo.js` | 纯算签名示例:加载 `./signer/` 下脚本并调用签名函数 |
| `verify.js` | 自检:验证 `env.js` 是否忠实回放 `seed.json`(`npm run verify`) |
| `seed.json` | 采集到的完整环境种子(值来源) |
| `signers.json` | 命中签名请求的 writer(URL + 调用栈定位的签名脚本) |
| `targets.json` | 抓到的目标参数真实上线值(用于核对纯算结果) |

## 用法

```bash
node verify.js          # 先确认补环境忠实回放了录制(应输出全部一致)
```

纯算还原签名:

1. 看 `signers.json`,把里面 `url`/`signer` 指向的**站点签名脚本**下载保存到 `./signer/`(命名为 `*.js`)。
2. `node demo.js`:它会把 `./signer/` 下脚本加载进补环境,并打印可疑的签名相关全局名。
3. 在 `demo.js` 末尾按站点实际导出名取到签名函数并调用,例如:

```js
const { createEnv, loadScript, run } = require("./index.js");
const sandbox = createEnv();
loadScript(sandbox, "signer/sign.js");
const sign = run(sandbox, "window.byted_acrawler && window.byted_acrawler.sign");
console.log(sign({ url: "..." }));
```

## 覆盖范围与边界

- ✅ navigator / screen / location / window 度量 / document(cookie + `createElement`)/ localStorage / sessionStorage
- ✅ **canvas 2D**(`toDataURL` 回放录制值)、**WebGL**(`getParameter`/`getExtension`/`getSupportedExtensions` 回放)、
  **AudioContext·OfflineAudioContext**(渲染缓冲回放录制切片,`OfflineAudioContext(1,5000,44100)` 经典配方对齐)
- ⚠ `getImageData` 像素级 canvas 指纹、罕见 WebGL 调用、WebRTC/字体枚举等高强度点可能仍需按目标站点**按需补全**
  (在 `env.js` 的 `setup()` 里追加即可)。
