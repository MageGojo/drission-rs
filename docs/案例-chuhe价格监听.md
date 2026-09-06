# 案例 · 听初和站模型报价

> 上游：[初和 AI](https://chuhe.lk888.ai/apidoc#common/list_models) 文档页右上角「模型价格」  
> 产物：`examples/sites/chuhe_price.rs`

## 这个案例在示范什么

用 **Rust 的 `Tab` API** 操作真实浏览器：`get` → `click` / `input` → `listen`。  
站点自己发带 JiaMi 签名的 XHR，我们只收包、在 Rust 里解析渠道价。

这和浏览器插件不是一类东西：

| | 本案例（drission） | 浏览器插件 / 页内脚本 |
|---|---|---|
| 跑在哪 | 你的 Rust 进程，CDP 遥控浏览器 | 页面 JS 沙箱 |
| 换引擎 | `"engine": "chrome"` / `"lightpanda"` 同一套 API | 绑死某一个 Chrome |
| 听网络 | `tab.listen()`（CDP `Network.*`） | `fetch` hook / webRequest |
| 跟价 | stdout / webhook，给下游改价 | 停在浏览器里 |

所以案例里**不要**再写一大段 `page.fetch(jiageliebiao)`。那是在复刻前端，读者分不清库做了什么。

## 怎么跑（自己跑，dist 内置 Lightpanda）

```bash
# 仓库根目录打一次包（二进制 + 旁边的 lightpanda）
./scripts/pack-chuhe-dist.sh

cd dist/chuhe
./chuhe_price
```

`dist/chuhe/` 里是可执行文件 `chuhe_price` 和同目录的 `lightpanda`，**不用本机 Chrome**。  
配置用旁边的 `chuhe.config.local.json`（账号自己填；打包脚本会把仓库里已有的本地配置拷过去，dist 已 gitignore）。  
听完一轮写出 `dist/chuhe/data/chuhe/models.csv`、`prices.csv` 就退出。

开发调试仍可用：

```bash
cp examples/sites/chuhe.config.json examples/sites/chuhe.config.local.json
cargo run --example chuhe_price --features cdp,ocr
```

登录：过期后 Lightpanda 自己开登录框，`tab.ocr_code` 读验证码图（数字或算式先求值），填完就登。  
取图走 `ele.image_bytes()`（`data:` / canvas / `fetch`），**不靠截图**——Lightpanda 的 `captureScreenshot` 是占位图。  
登录态仍会写入 `profile/storage.json`，下次未过期就跳过登录。强制再登：`CHUHE_FORCE_LOGIN=1`。

`chuhe.config.local.json` 已 gitignore。

## 页面会打哪些接口

点「模型价格」→ `POST /moxing/jiageliebiao`（157 个模型，目录无价）。  
再点一条模型 → `POST /moxing/moxingxiangqing`，价在 `data.渠道分组列表`。

对话（GEM 3.7 flash）按 token，例如 ZG-Anti-GEM-1：输入 0.7379 / 输出 3.6379。  
视频（Omni 1.1）按次：基础 0.103，5 秒 0.515，10 秒 1.03。

变价一行 JSON：

```json
{"ts":1757160000000,"model":"ZG-Anti-GEM-1","field":"输入token价格","old":0.7379,"new":0.8,"url":"...moxingxiangqing"}
```
