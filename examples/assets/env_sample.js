// 示例补环境(占位)。这是 `tab.dump_env().export_project(...)` 产出的 env.js 的**最小占位版**,
// 仅为让 `env_signer` 示例开箱即编、自检通过(navigator/screen/location 与同目录 seed_sample.json 一致)。
//
// 实际使用:把本文件与 seed_sample.json 换成你自己 `dump-env` 出的 env.js + seed.json
// (含 canvas/webgl/audio 指纹回放),或把 env_signer.rs 里的 include_str! 路径指向你的工程目录。
(function (g) {
  g.navigator = {
    userAgent:
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    platform: "Win32",
    language: "zh-CN",
    hardwareConcurrency: 8,
    deviceMemory: 8,
    vendor: "Google Inc.",
    maxTouchPoints: 0,
  };
  g.screen = {
    width: 1920,
    height: 1080,
    availWidth: 1920,
    availHeight: 1040,
    colorDepth: 24,
    pixelDepth: 24,
  };
  g.location = { host: "example.com", origin: "https://example.com" };
})(globalThis);
