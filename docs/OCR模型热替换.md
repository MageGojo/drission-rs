# OCR / 检测模型热替换(自训模型上线)· 里程碑 57

> 第三梯队 ① 的**阶段3**:ddddocr 预训练模型读不动易盾这类**艺术体单字**(实测置信常 0.00–0.17)。
> 真正的精度天花板 = 用 **dddd_trainer** 针对目标验证码自训一个模型,再**热替换**进 drission。
> 本库已把"加载自定义模型 + 字符集、运行时热替换、环境变量零改码上线"全部备好,训练在库外用 Python 做。

---

## 1. 为什么需要它

- 检测(YOLOX 找字框)与全局指派(里程碑 55)已稳;瓶颈在**单字识别**。
- ddddocr 通用识别模型没见过易盾的书法/扭曲艺术字 → 偶发"确信误读"。
- 通用解只有两条:① 推理期多信号(TTA / 字形模板,里程碑 56 阶段1/2,提升有限);
  ② **自训模型**(本文,见过该字体后命中率才能拉满)。

## 2. 热替换 API 速查(`feature = "ocr"`,后端无关)

| 能力 | API |
|---|---|
| 自定义模型 + 字符集 | `Ocr::from_model_path_with_charset(onnx, charset)` |
| 模型文件 + 字符集文件 | `Ocr::from_files(&onnx, &charset)` |
| 读字符集文件(三格式自适应) | `ocr::load_charset_file(&path)` |
| **原地热替换**模型(留字符集) | `ocr.set_model(&onnx)?` |
| **原地热替换**模型 + 字符集 | `ocr.set_model_with_charset(&onnx, charset)?` |
| 检测模型热替换 | `det.set_model(&onnx)?` |
| 进程级默认(`tab.ocr_image`)热替换 | `ocr::set_default_ocr(ocr).await`(camoufox) |
| 默认模型缓存路径 | `Ocr::default_model_path().await?` / `Det::default_model_path().await?` |
| 当前字符集大小 | `ocr.charset_len()` |

**环境变量(零改码上线)**——现有 `examples/yidun_click` 不动一行即可吃自训模型:

```bash
DRISSION_DET_MODEL=yidun_det.onnx \
DRISSION_OCR_MODEL=yidun_ocr.onnx \
DRISSION_OCR_CHARSET=yidun_charset.json \
cargo run --example yidun_click --features cdp,ocr
```

`ClickWord::new()` → `Det::new()` / `Ocr::new()` 会自动读这三个变量(缺则用默认 ddddocr 模型)。
演示见 `examples/ocr_hotswap`(`cargo run --example ocr_hotswap --features ocr`)。

## 3. 字符集格式(重要)

`load_charset_file` 自适应三种:
- `{"charset": ["", "甲", "乙", ...]}`(ddddocr / dddd_trainer 的 `charsets.json`)
- 纯 JSON 数组 `["", "甲", "乙", ...]`
- 每行一字的纯文本(首个空行 = blank 占位)

**约定(必须满足,否则识别全乱)**:
- `charset[0]` 是 **CTC blank = 空串 `""`**;
- `charset.len()` == 模型输出类别数(同 ddddocr 解码约定:类别 `i` 对应 `charset[i]`,`0` 为 blank)。
- 若 dddd_trainer 的 charset 不含 blank,自己在最前面补一个 `""`。首项非空时库会 `warn` 提示。

## 4. dddd_trainer 自训全流程

> dddd_trainer:<https://github.com/sml2h3/dddd_trainer>(产出与 ddddocr 兼容的 onnx + charsets.json)。
> 这是**单字识别**模型:输入单字图、输出该字。点选的"框"由本库的 `Det` 负责,不需训检测。

### 4.1 采样(用本库 `Det` 自动切字框)

`examples/yidun_click` 已内置采样:设 `YIDUN_DUMP=目录` 跑几十~上百次,每个检测框裁剪图落盘:

```bash
YIDUN_DUMP=./yidun_samples YIDUN_TRIES=1 \
cargo run --example yidun_click --features cdp,ocr
```

得到一堆 `g_*.png`(单字图)。库内对应能力:`ClickWord::crops(&img) -> Vec<(BBox, png)>`。

### 4.2 标注

- 人工把每张单字图重命名/归类为它的真实字(这是唯一绕不开的人工环节;验证码答案拿不到)。
- **半自动加速**:当前管线对清楚字已能 0.8–0.99 置信识别(里程碑 56),可先用本库 `Ocr::recognize`
  给样本打**弱标签**(文件名 = 猜测字 + 置信),只人工**复核纠错**高置信外的少数,标注量大降。
- dddd_trainer 接受的常见组织:`数据集/标签_序号.png`(标签即该字),或图+标签清单文件(按其 README)。

### 4.3 训练 + 导出(Python 侧)

```bash
git clone https://github.com/sml2h3/dddd_trainer && cd dddd_trainer
pip install -r requirements.txt
python app.py create yidun            # 建项目,改 projects/yidun/config.yaml(单字、字符集等)
# 把标注好的样本放进数据集目录
python app.py train yidun             # 训练
python app.py cache yidun             # 导出 onnx
# 产出:yidun_*.onnx + charsets.json(charset[0] 须为 "")
```

### 4.4 上线(回到 drission,零改码)

```bash
DRISSION_OCR_MODEL=projects/yidun/models/yidun_x.x_xxx.onnx \
DRISSION_OCR_CHARSET=projects/yidun/models/charsets.json \
cargo run --example yidun_click --features cdp,ocr
```

或代码里:

```rust
let mut cw = ClickWord::new().await?;
cw.ocr.set_model_with_charset(
    std::path::Path::new("yidun_ocr.onnx"),
    drission::ocr::load_charset_file(std::path::Path::new("yidun_charset.json"))?,
)?;
let hits = cw.solve(&cap_png, &targets)?;   // 之后命中率由自训模型决定
```

## 4.5 实操数据墙 + 真样本模板库(里程碑 59,推荐先用)

实跑 `examples/yidun_collect` 采 190 crop 后发现:**Det 会误检 UI 图标(刷新/耳机)与细条**,真字能高置信标注的仅 ~37 张,**自训 CRNN 数据不足必过拟合**。要把 dddd_trainer 走通,需先:
- **清洗采集**:过滤非字框(按宽高比≈1、最小尺寸、排除工具栏区域);
- **扩量**:采几百~上千张、人工标注(`yidun-train/tools/montage.py` 拼图,~10 类可标)。

鉴于易盾试用**字表只有 ~10 个**,更高 ROI 的等价方案 = **真样本模板库**(无需训练/torch):

```bash
# 1) 采样 + 拼图标注(把 montage 里的字按 {字}/ 归类到 bank 目录)
YIDUN_DUMP=./yidun_samples cargo run --example yidun_collect --features cdp,ocr
python yidun-train/tools/montage.py            # 生成 contact sheet 人工标注
# 整理出 bank/{字}/*.png(每字 10~20 张更稳)
# 2) 直接热接入(零训练):solve 的模板信号会"真样本库优先"
DRISSION_GLYPH_SAMPLES=./yidun_samples/bank cargo run --example yidun_click --features cdp,ocr
```

`SampleBank`(`src/ocr/glyph.rs`)对每个字框取灰度梯度特征,与该字所有真样本做多旋转最大 NCC;
`ClickWord` 自动 `DRISSION_GLYPH_SAMPLES` 加载。实测真样本模板分 0.5~0.6(高于渲染字体 0.2~0.5),
OCR 读不出的字也能被真样本顶起、综合置信度过阈驱动点击。**几张即用**,样本越多越稳——
比小数据自训更划算;数据攒厚后再上 dddd_trainer 亦可。

## 5. 注意

- **tract 算子兼容**:沿用里程碑 41 结论——导出标准 LSTM 的 onnx(beta 路线),不要带
  `DynamicQuantizeLSTM` 等 onnxruntime contrib 自定义算子(tract 不支持)。
- **检测模型**一般无需自训(YOLOX `common_det.onnx` 找字框已够);要换也走 `DRISSION_DET_MODEL` / `det.set_model`。
- **行为风控仍是另一件事**:模型把字读准只解决"识别";易盾的轨迹/指纹/IP 风控属阶段(②)行为轨迹模型化。
