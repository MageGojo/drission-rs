//! 点选 OCR **离线精度自评**(无浏览器、无网络;模型走本地缓存):用一批**已标注**的单字 crop,量化
//! 「纯 OCR」、「仅字形样本库模板」、「OCR + 模板融合(即开 `DRISSION_GLYPH_SAMPLES` 第二信号)」三者的
//! **强制 N 选一**识别精度,直接回答「开第二信号到底提了多少精度」。
//!
//! 关键:样本库就是用这些 crop 建的(`build_bank.py`),直接评会**自我匹配**得虚高分。本工具用库的
//! [`SampleBank::similarity_image_excluding`](drission::ocr::SampleBank::similarity_image_excluding)
//! 做**留一法**——按内容哈希排除「待测图自己」那条样本,得到**无泄漏**的诚实数字。
//!
//! 运行(模型已缓存即纯离线):
//!   cargo run --example clickword_eval --features ocr
//! 可调环境变量:
//!   LABELS  标注文件(每行 `文件名\t字`,默认 ../yidun-train/label/labels_seed.txt)
//!   SRC     crop 图目录(默认 yidun_samples)
//!   BANK    字形样本库目录(默认 SRC/bank)
//!   TPL_W   融合权重(combo = aff + W×tpl,默认 1.5,与库内 TEMPLATE_WEIGHT 一致)

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use drission::ocr::{Ocr, SampleBank};

#[tokio::main]
async fn main() -> drission::Result<()> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = env_path("SRC").unwrap_or_else(|| manifest.join("yidun_samples"));
    let bank_dir = env_path("BANK").unwrap_or_else(|| src.join("bank"));
    let labels = env_path("LABELS").unwrap_or_else(|| {
        manifest
            .join("..")
            .join("yidun-train")
            .join("label")
            .join("labels_seed.txt")
    });
    let w: f32 = std::env::var("TPL_W")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.5);
    // 置信门控融合阈值:OCR 该框 top 亲和度 ≥ HI 完全信 OCR(模板权重 0);≤ LO 完全靠模板(权重 W);
    // 之间线性。这样易框(OCR 自信)不被模板干扰、难框(OCR≈0)才让模板救场。
    // 默认与库内 TEMPLATE_GATE_HI/LO 一致(src/ocr/mod.rs),开箱即反映线上实际行为。
    let gate_hi: f32 = env_f32("GATE_HI", 0.10);
    let gate_lo: f32 = env_f32("GATE_LO", 0.01);
    let dump = std::env::var("EVAL_DUMP").is_ok();

    // 读标注:每行 `文件名\t字`。
    let text = std::fs::read_to_string(&labels)
        .map_err(|e| drission::Error::msg(format!("读标注 {}: {e}", labels.display())))?;
    let mut items: Vec<(String, char)> = Vec::new();
    for ln in text.lines() {
        let ln = ln.trim();
        if ln.is_empty() {
            continue;
        }
        let mut it = ln.split('\t');
        let (Some(f), Some(c)) = (it.next(), it.next()) else {
            continue;
        };
        if let Some(ch) = c.trim().chars().next() {
            items.push((f.to_string(), ch));
        }
    }
    if items.is_empty() {
        return Err(drission::Error::msg("标注为空,无可评样本"));
    }

    // 候选字表 = 标注里出现过的全部字(小固定字表的「强制 N 选一」,比真实每题 2~4 字更难、更能区分信号)。
    let cand: Vec<char> = items
        .iter()
        .map(|(_, c)| *c)
        .collect::<BTreeSet<char>>()
        .into_iter()
        .collect();

    println!(
        "[eval] 标注 {} 张 · 候选字表 {} 字「{}」",
        items.len(),
        cand.len(),
        cand.iter().collect::<String>()
    );
    println!("[eval] crop 目录 = {}", src.display());
    println!("[eval] 样本库   = {}", bank_dir.display());

    println!("[eval] 加载 ocr 模型(本地缓存)…");
    let ocr = Ocr::new().await?;
    let bank = SampleBank::from_dir(&bank_dir).ok();
    match &bank {
        Some(b) => println!("[eval] 样本库载入 {} 张 · 留一法排除待测图自己", b.len()),
        None => println!("[eval] ⚠ 样本库未载入,仅评纯 OCR(融合=纯OCR)"),
    }
    println!("[eval] 融合权重 W = {w}(combo = aff + W×tpl)\n");

    let (mut n, mut ocr_ok, mut tpl_ok, mut fus_ok, mut gat_ok) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    // 每字 [n, ocr_ok, gated_ok]。
    let mut per: BTreeMap<char, [usize; 3]> = BTreeMap::new();
    let mut rescued: Vec<(String, char, char)> = Vec::new(); // 纯OCR错、门控融合纠正
    let mut gated_wrong: Vec<(String, char, char)> = Vec::new(); // 门控融合仍错(攒样本/自训重点)
    if dump {
        println!("[dump] file  truth  ocrTop(aff)  ocrTruthAff  tplTop(sim)\n");
    }

    for (file, truth) in &items {
        let path = src.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("[eval] 缺图跳过:{}", path.display());
            continue;
        };
        // ① 纯 OCR:受约束识别,对每个候选字给亲和度。
        let Ok(aff) = ocr.char_affinity(&bytes, &cand) else {
            continue;
        };
        // ② 字形模板(留一法,排除自己)。无样本库则全 0 ⇒ 融合退化为纯 OCR。
        let tpl: Vec<f32> = cand
            .iter()
            .map(|&ch| {
                bank.as_ref()
                    .and_then(|b| b.similarity_image_excluding(&bytes, ch).ok())
                    .unwrap_or(0.0)
            })
            .collect();
        // ③ 旧融合(always-on,combo = aff + W×tpl)。
        let fused: Vec<f32> = aff.iter().zip(&tpl).map(|(a, t)| a + w * t).collect();
        // ④ 置信门控融合:模板权重按 OCR 该框 top 亲和度衰减(OCR 自信→不让模板干扰;OCR 没底→模板救场)。
        let ocr_top = aff.iter().cloned().fold(f32::MIN, f32::max).max(0.0);
        let gate = gate_weight(ocr_top, gate_hi, gate_lo);
        let gated: Vec<f32> = aff
            .iter()
            .zip(&tpl)
            .map(|(a, t)| a + w * gate * t)
            .collect();

        let p_ocr = argmax_char(&aff, &cand);
        let p_tpl = argmax_char(&tpl, &cand);
        let p_fus = argmax_char(&fused, &cand);
        let p_gat = argmax_char(&gated, &cand);

        if dump {
            let truth_idx = cand.iter().position(|c| c == truth).unwrap_or(0);
            let tpl_top_i = (0..tpl.len()).max_by(|&a, &b| tpl[a].total_cmp(&tpl[b])).unwrap_or(0);
            println!(
                "[dump] {file}  「{truth}」  「{}」{:.2}  {:.2}  「{}」{:.2}  gate={:.2}",
                p_ocr, ocr_top, aff[truth_idx], cand[tpl_top_i], tpl[tpl_top_i], gate
            );
        }

        n += 1;
        let e = per.entry(*truth).or_default();
        e[0] += 1;
        if p_ocr == *truth {
            ocr_ok += 1;
            e[1] += 1;
        }
        if p_tpl == *truth {
            tpl_ok += 1;
        }
        if p_fus == *truth {
            fus_ok += 1;
        }
        if p_gat == *truth {
            gat_ok += 1;
            e[2] += 1;
        }
        if p_ocr != *truth && p_gat == *truth {
            rescued.push((file.clone(), *truth, p_ocr));
        }
        if p_gat != *truth {
            gated_wrong.push((file.clone(), *truth, p_gat));
        }
    }

    let pct = |k: usize| if n == 0 { 0.0 } else { 100.0 * k as f32 / n as f32 };
    println!("════════════════ 结果(N={n} · {} 选 1)════════════════", cand.len());
    println!("纯 OCR             {ocr_ok:>3}/{n}  ({:.1}%)", pct(ocr_ok));
    println!("仅字形模板         {tpl_ok:>3}/{n}  ({:.1}%)", pct(tpl_ok));
    println!(
        "always-on 融合(W={w})  {fus_ok:>3}/{n}  ({:.1}%)   旧策略 aff+W×tpl",
        pct(fus_ok)
    );
    println!(
        "置信门控融合       {gat_ok:>3}/{n}  ({:.1}%)   ← 推荐(HI={gate_hi} LO={gate_lo})",
        pct(gat_ok)
    );
    println!(
        "门控 vs 纯OCR = {:+.1} 个百分点({:+} 张);always-on vs 纯OCR = {:+.1} 个百分点({:+} 张)",
        pct(gat_ok) - pct(ocr_ok),
        gat_ok as i64 - ocr_ok as i64,
        pct(fus_ok) - pct(ocr_ok),
        fus_ok as i64 - ocr_ok as i64
    );

    println!("\n按字(n / 纯OCR对 / 门控融合对):");
    for (ch, [cn, co, cf]) in &per {
        let warn = if *cn < 2 {
            "   ← 样本<2,留一法下该字模板无援"
        } else {
            ""
        };
        println!("  「{ch}」  {cn} / {co} / {cf}{warn}");
    }

    if !rescued.is_empty() {
        println!("\n门控融合纠正了纯 OCR 的误读({} 例,正是第二信号的价值):", rescued.len());
        for (f, t, o) in &rescued {
            println!("  {f}: 真「{t}」· 纯OCR误判「{o}」→ 门控融合纠正 ✓");
        }
    }
    if !gated_wrong.is_empty() {
        println!("\n门控融合仍判错({} 例 → 攒样本 / 自训的重点目标):", gated_wrong.len());
        for (f, t, p) in &gated_wrong {
            println!("  {f}: 真「{t}」→ 判「{p}」");
        }
    }
    Ok(())
}

/// 置信门控权重 g∈[0,1]:OCR top 亲和度 `c` ≥ `hi` → 0(完全信 OCR);≤ `lo` → 1(完全靠模板);之间线性。
fn gate_weight(c: f32, hi: f32, lo: f32) -> f32 {
    if hi <= lo {
        return 1.0;
    }
    ((hi - c) / (hi - lo)).clamp(0.0, 1.0)
}

/// 取环境变量为 f32(缺/非法用默认)。
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// 取环境变量为路径(空则 None)。
fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// 在分数向量里取最大者对应的候选字(空则取第一个候选字兜底)。
fn argmax_char(v: &[f32], cand: &[char]) -> char {
    let mut bi = 0usize;
    let mut bv = f32::MIN;
    for (i, &x) in v.iter().enumerate() {
        if x > bv {
            bv = x;
            bi = i;
        }
    }
    cand.get(bi).copied().unwrap_or(cand[0])
}
