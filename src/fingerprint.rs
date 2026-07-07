//! 读取浏览器**实时指纹快照**(后端无关):一次性 dump 当前页面暴露给 JS 的核心指纹信号
//! —— UA / platform / 语言 / 时区 / 屏幕 / `devicePixelRatio` / 硬件并发 / 设备内存 /
//! WebGL `UNMASKED_RENDERER` / canvas 像素哈希。
//!
//! 与 `cdp::fingerprint` / `pool::fingerprint`(**设定 / 伪装**指纹)相对:本模块只**读取**指纹结果,
//! 用于"验证指纹确实换了"、有头/无头 diff、多画像对比、身份一致性诊断等。两后端(CDP [`ChromiumTab`](crate::cdp::ChromiumTab) /
//! Camoufox [`Tab`](crate::browser::Tab))共用同一段探针 JS,经各自的 `run_js` 求值。
//!
//! ```no_run
//! # async fn f(tab: &(impl drission::prelude::FingerprintProbe + Sync)) -> drission::Result<()> {
//! use drission::prelude::*;
//! let fp = tab.fingerprint_snapshot().await?;          // 当前需已在某文档上(canvas/webgl 探针要 DOM)
//! println!("UA={} canvas#={} webgl={}", fp.ua, fp.canvas_hash, fp.webgl_renderer);
//! let report = fp.diagnose();                          // 指纹 / 身份一致性评分
//! println!("identity score = {}", report.score);
//! let link = fp.linkability_to(&fp);                    // 两套画像是否容易被关联
//! println!("linkability score = {}", link.score);
//! # Ok(()) }
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;

/// 浏览器实时指纹快照(由 [`FingerprintProbe::fingerprint_snapshot`] 采集)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintSnapshot {
    /// `navigator.userAgent`。
    #[serde(alias = "userAgent")]
    pub ua: String,
    /// `navigator.platform`。
    pub platform: String,
    /// `navigator.userAgentData.platform`(Chromium 系可用;Firefox/Safari 通常为空)。
    #[serde(default, alias = "uaDataPlatform")]
    pub ua_data_platform: String,
    /// `navigator.userAgentData.mobile`(Chromium 系可用;缺失时为 `false`)。
    #[serde(default, alias = "uaDataMobile")]
    pub ua_data_mobile: bool,
    /// `navigator.webdriver === true`。
    #[serde(default)]
    pub webdriver: bool,
    /// `navigator.languages`(逗号连接)。
    pub languages: String,
    /// `navigator.maxTouchPoints`。
    #[serde(default, alias = "maxTouchPoints")]
    pub max_touch_points: u32,
    /// `navigator.hardwareConcurrency`(逻辑核数;未知为 0)。
    #[serde(alias = "hardwareConcurrency")]
    pub hardware_concurrency: u32,
    /// `navigator.deviceMemory`(GB;浏览器未暴露为 0)。
    #[serde(alias = "deviceMemory")]
    pub device_memory: f64,
    /// 屏幕分辨率 `"宽x高"`。
    pub screen: String,
    /// `window.devicePixelRatio`。
    #[serde(alias = "devicePixelRatio")]
    pub device_pixel_ratio: f64,
    /// IANA 时区(`Intl.DateTimeFormat().resolvedOptions().timeZone`)。
    pub timezone: String,
    /// WebGL `UNMASKED_RENDERER_WEBGL`(无 WebGL 为 `none`、出错为 `err`)。
    #[serde(alias = "webglRenderer")]
    pub webgl_renderer: String,
    /// canvas 渲染像素哈希(8 位 hex;同机同浏览器稳定、跨指纹应不同)。
    #[serde(alias = "canvasHash")]
    pub canvas_hash: String,
}

impl FingerprintSnapshot {
    /// 从探针返回值解析(探针用 `JSON.stringify` 返回字符串;也兼容后端直接返回对象)。纯函数,便于测试。
    fn from_probe(v: &Value) -> Self {
        let parsed;
        let o: &Value = if let Some(s) = v.as_str() {
            parsed = serde_json::from_str(s).unwrap_or(Value::Null);
            &parsed
        } else {
            v
        };
        FingerprintSnapshot {
            ua: o["ua"].as_str().unwrap_or_default().to_string(),
            platform: o["platform"].as_str().unwrap_or_default().to_string(),
            ua_data_platform: o["uaDataPlatform"].as_str().unwrap_or_default().to_string(),
            ua_data_mobile: o["uaDataMobile"].as_bool().unwrap_or(false),
            webdriver: o["webdriver"].as_bool().unwrap_or(false),
            languages: o["languages"].as_str().unwrap_or_default().to_string(),
            max_touch_points: o["maxTouchPoints"].as_u64().unwrap_or(0) as u32,
            hardware_concurrency: o["hardwareConcurrency"].as_u64().unwrap_or(0) as u32,
            device_memory: o["deviceMemory"].as_f64().unwrap_or(0.0),
            screen: o["screen"].as_str().unwrap_or_default().to_string(),
            device_pixel_ratio: o["devicePixelRatio"].as_f64().unwrap_or(0.0),
            timezone: o["timezone"].as_str().unwrap_or_default().to_string(),
            webgl_renderer: o["webglRenderer"].as_str().unwrap_or_default().to_string(),
            canvas_hash: o["canvasHash"].as_str().unwrap_or_default().to_string(),
        }
    }

    /// 对当前快照做**身份/指纹一致性诊断**。
    ///
    /// 这是启发式报告,用于尽早发现明显露馅点:如 `HeadlessChrome`、`navigator.webdriver=true`、
    /// UA / platform / WebGL OS 互相矛盾、软件渲染、移动端触摸点缺失、语言与时区强冲突等。
    pub fn diagnose(&self) -> IdentityReport {
        IdentityReport::from_snapshot(self)
    }

    /// 比较两份快照的**可关联性**:分数越高,越像同一台机器 / 同一身份画像。
    ///
    /// 用于代理 / 指纹轮换自检:如果两个账号的 UA、WebGL、canvas、屏幕、时区等稳定信号仍高度相同,
    /// 即使 cookie / IP 已隔离,也可能被风控侧关联。
    pub fn linkability_to(&self, other: &FingerprintSnapshot) -> LinkabilityReport {
        LinkabilityReport::compare(self, other)
    }

    /// 比较同一账号 / 同一 profile 在两轮采集之间的画像漂移。
    ///
    /// 与 [`FingerprintSnapshot::linkability_to`] 不同,这里假设两份快照本应代表同一身份画像:
    /// WebGL、canvas、UA/OS、locale 等稳定字段突然变化,通常意味着浏览器升级、补环境失效、
    /// 代理画像错配或 profile 被污染。
    pub fn drift_to(&self, current: &FingerprintSnapshot) -> IdentityDriftReport {
        IdentityDriftReport::compare(self, current)
    }

    /// 当前快照的稳定哈希。字段顺序固定且会做轻量归一化,适合跨机器 / 多次采集对账。
    pub fn stable_hash(&self) -> String {
        format!("{:016x}", stable_hash64(&self.hash_material()))
    }

    /// 当前快照的短身份 ID,用于日志、baseline 和账号池报告中引用。
    pub fn identity_id(&self) -> String {
        format!("fp_{}", self.stable_hash())
    }

    fn hash_material(&self) -> String {
        [
            ("ua", norm_hash_value(&self.ua)),
            ("platform", norm_hash_value(&self.platform)),
            ("ua_data_platform", norm_hash_value(&self.ua_data_platform)),
            ("ua_data_mobile", self.ua_data_mobile.to_string()),
            ("webdriver", self.webdriver.to_string()),
            ("languages", norm_hash_value(&self.languages)),
            ("max_touch_points", self.max_touch_points.to_string()),
            (
                "hardware_concurrency",
                self.hardware_concurrency.to_string(),
            ),
            ("device_memory", format_float(self.device_memory)),
            ("screen", norm_hash_value(&self.screen)),
            ("device_pixel_ratio", format_float(self.device_pixel_ratio)),
            ("timezone", norm_hash_value(&self.timezone)),
            ("webgl_renderer", norm_hash_value(&self.webgl_renderer)),
            ("canvas_hash", norm_hash_value(&self.canvas_hash)),
        ]
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// 指纹一致性风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentitySeverity {
    /// 信息项,不扣分。
    Info,
    /// 轻微风险:不一定是问题,但值得检查。
    Low,
    /// 中等风险:常见检测会利用该信号。
    Medium,
    /// 高风险:通常意味着身份画像明显不自洽。
    High,
}

impl IdentitySeverity {
    fn penalty(self) -> u8 {
        match self {
            IdentitySeverity::Info => 0,
            IdentitySeverity::Low => 7,
            IdentitySeverity::Medium => 15,
            IdentitySeverity::High => 30,
        }
    }
}

/// 单条身份一致性诊断问题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityIssue {
    pub severity: IdentitySeverity,
    /// 稳定机器码,便于上层筛选 / JSON 协议消费。
    pub code: String,
    /// 人类可读问题描述。
    pub message: String,
    /// 修复方向。
    pub suggestion: String,
}

/// 结构化身份修复动作优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityFixPriority {
    #[default]
    Low,
    Medium,
    High,
}

/// 结构化身份修复动作影响的配置层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityFixTarget {
    UserAgent,
    ClientHints,
    Stealth,
    ProfileOs,
    LocaleProxy,
    Touch,
    Hardware,
    Viewport,
    GpuWebgl,
    Canvas,
}

/// 一条可供 Agent / 调度器消费的身份修复动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityFixAction {
    /// 稳定动作码,用于上层策略匹配。
    pub code: String,
    /// 需要改动的配置层。
    pub target: IdentityFixTarget,
    /// 动作优先级,由触发问题的最高严重度决定。
    pub priority: IdentityFixPriority,
    /// 人类可读标题。
    pub title: String,
    /// 机器可展示的修复说明。
    pub detail: String,
    /// 该动作涉及的快照字段或配置键。
    pub fields: Vec<String>,
    /// 触发该动作的 issue code 列表。
    pub issue_codes: Vec<String>,
}

/// 身份修复计划:把零散 issue 合并成机器可执行的动作列表。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityFixPlan {
    pub action_count: usize,
    pub high_priority_count: usize,
    pub targets: Vec<IdentityFixTarget>,
    pub actions: Vec<IdentityFixAction>,
}

impl IdentityFixPlan {
    /// 从诊断问题生成结构化修复计划。
    pub fn from_issues(issues: &[IdentityIssue]) -> Self {
        build_identity_fix_plan(issues)
    }

    /// 没有需要修复的结构化动作。
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// 浏览器身份一致性诊断报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityReport {
    /// 快照稳定 ID,等价于 [`FingerprintSnapshot::identity_id`]。
    #[serde(default, rename = "identityId", alias = "identity_id")]
    pub identity_id: String,
    /// 快照稳定哈希,等价于 [`FingerprintSnapshot::stable_hash`]。
    #[serde(default, rename = "stableHash", alias = "stable_hash")]
    pub stable_hash: String,
    /// 0-100 分,越高越自洽。
    pub score: u8,
    /// 采集到的原始快照。
    pub snapshot: FingerprintSnapshot,
    /// 发现的问题,按严重度从高到低排列。
    pub issues: Vec<IdentityIssue>,
    /// 面向 Agent / 调度器的结构化修复计划。
    #[serde(default, rename = "fixPlan", alias = "fix_plan")]
    pub fix_plan: IdentityFixPlan,
}

impl IdentityReport {
    /// 从实时指纹快照生成诊断报告。
    pub fn from_snapshot(snapshot: &FingerprintSnapshot) -> Self {
        let mut issues = Vec::new();
        diagnose_identity(snapshot, &mut issues);
        issues.sort_by_key(|i| match i.severity {
            IdentitySeverity::High => 0,
            IdentitySeverity::Medium => 1,
            IdentitySeverity::Low => 2,
            IdentitySeverity::Info => 3,
        });
        let penalty = issues
            .iter()
            .map(|i| i.severity.penalty() as u16)
            .sum::<u16>()
            .min(100);
        let fix_plan = IdentityFixPlan::from_issues(&issues);
        let stable_hash = snapshot.stable_hash();
        Self {
            identity_id: format!("fp_{stable_hash}"),
            stable_hash,
            score: 100u8.saturating_sub(penalty as u8),
            snapshot: snapshot.clone(),
            issues,
            fix_plan,
        }
    }

    /// 是否没有高风险问题且分数达到 80。
    pub fn is_healthy(&self) -> bool {
        self.score >= 80 && !self.has_high_risk()
    }

    /// 是否存在高风险问题。
    pub fn has_high_risk(&self) -> bool {
        self.issues
            .iter()
            .any(|i| i.severity == IdentitySeverity::High)
    }
}

/// 两份浏览器身份快照的关联强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkabilityStrength {
    /// 弱关联:单独看不危险,但多个弱信号叠加会形成稳定画像。
    Weak,
    /// 中等关联:常用于聚类,需要结合其它信号判断。
    Medium,
    /// 强关联:高度稳定或高熵信号相同,通常应认为可关联。
    Strong,
}

impl LinkabilityStrength {
    fn weight(self) -> u8 {
        match self {
            LinkabilityStrength::Weak => 8,
            LinkabilityStrength::Medium => 18,
            LinkabilityStrength::Strong => 30,
        }
    }
}

/// 单个可关联信号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkabilitySignal {
    pub strength: LinkabilityStrength,
    /// 稳定机器码,便于 JSON 协议 / CLI / 上层 SDK 筛选。
    pub code: String,
    /// 人类可读描述。
    pub message: String,
    /// 降低关联性的修复方向。
    pub suggestion: String,
}

/// 两份指纹快照的可关联性报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkabilityReport {
    /// 0-100 分,越高越容易被认为是同一画像。
    pub score: u8,
    /// `true` 表示当前启发式规则认为两份快照容易被关联。
    pub same_identity_likely: bool,
    /// 第一份快照。
    pub left: FingerprintSnapshot,
    /// 第二份快照。
    pub right: FingerprintSnapshot,
    /// 命中的可关联信号,按强度从高到低排列。
    pub signals: Vec<LinkabilitySignal>,
}

/// 同一账号 / profile 跨轮画像漂移的严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IdentityDriftSeverity {
    /// 无稳定字段变化。
    None,
    /// 低风险变化:可能是浏览器小版本、硬件低熵字段或窗口形态变化。
    Low,
    /// 中风险变化:locale、timezone、screen/DPR、Client Hints 等画像层变化。
    Medium,
    /// 高风险变化:OS 画像、WebGL、canvas、webdriver 等核心身份信号变化。
    High,
}

impl IdentityDriftSeverity {
    fn penalty(self) -> u8 {
        match self {
            IdentityDriftSeverity::None => 0,
            IdentityDriftSeverity::Low => 7,
            IdentityDriftSeverity::Medium => 15,
            IdentityDriftSeverity::High => 30,
        }
    }
}

/// 单个漂移信号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDriftSignal {
    pub severity: IdentityDriftSeverity,
    /// 稳定机器码,便于 CLI / 调度器筛选。
    pub code: String,
    /// 上一轮值。
    pub before: String,
    /// 当前轮值。
    pub after: String,
    /// 人类可读描述。
    pub message: String,
    /// 调度器或人工排查建议。
    pub suggestion: String,
}

/// 跨轮漂移修复动作影响的配置层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityDriftRemediationTarget {
    Admission,
    Baseline,
    UserAgent,
    ClientHints,
    Stealth,
    ProfileOs,
    LocaleProxy,
    Touch,
    Hardware,
    Viewport,
    GpuWebgl,
    Canvas,
}

/// 一条同账号画像漂移修复动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDriftRemediationAction {
    /// 稳定动作码,用于调度器匹配。
    pub code: String,
    /// 需要调整的配置层。
    pub target: IdentityDriftRemediationTarget,
    /// 动作优先级。
    pub priority: IdentityFixPriority,
    /// 人类可读标题。
    pub title: String,
    /// 修复说明。
    pub detail: String,
    /// 涉及的快照字段或配置键。
    pub fields: Vec<String>,
    /// 触发该动作的 drift signal codes。
    pub signal_codes: Vec<String>,
    /// 旧值样本。
    pub before_values: Vec<String>,
    /// 新值样本。
    pub after_values: Vec<String>,
}

/// 同账号跨轮漂移修复计划。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDriftRemediationPlan {
    pub action_count: usize,
    pub high_priority_count: usize,
    pub targets: Vec<IdentityDriftRemediationTarget>,
    pub actions: Vec<IdentityDriftRemediationAction>,
}

impl IdentityDriftRemediationPlan {
    /// 从漂移信号生成结构化修复计划。
    pub fn from_signals(signals: &[IdentityDriftSignal]) -> Self {
        build_identity_drift_remediation_plan(signals)
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// 同一身份画像跨轮稳定性报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDriftReport {
    pub before_id: String,
    pub after_id: String,
    pub before_stable_hash: String,
    pub after_stable_hash: String,
    pub stable_hash_changed: bool,
    /// 0-100 分,越高表示漂移越严重。
    pub score: u8,
    pub severity: IdentityDriftSeverity,
    pub changed_signal_count: usize,
    pub high_risk_signal_count: usize,
    pub before: FingerprintSnapshot,
    pub after: FingerprintSnapshot,
    pub signals: Vec<IdentityDriftSignal>,
    /// 面向账号池调度器的结构化漂移修复计划。
    #[serde(default, rename = "remediationPlan", alias = "remediation_plan")]
    pub remediation_plan: IdentityDriftRemediationPlan,
}

impl IdentityDriftReport {
    /// 比较两轮快照。`before` 应是旧基线,`after` 应是当前采集值。
    pub fn compare(before: &FingerprintSnapshot, after: &FingerprintSnapshot) -> Self {
        let mut signals = Vec::new();
        compare_identity_drift(before, after, &mut signals);
        signals.sort_by(|a, b| {
            drift_severity_rank(b.severity)
                .cmp(&drift_severity_rank(a.severity))
                .then_with(|| a.code.cmp(&b.code))
        });
        let score = signals
            .iter()
            .map(|signal| signal.severity.penalty() as u16)
            .sum::<u16>()
            .min(100) as u8;
        let severity = signals
            .iter()
            .map(|signal| signal.severity)
            .max_by_key(|severity| drift_severity_rank(*severity))
            .unwrap_or(IdentityDriftSeverity::None);
        let high_risk_signal_count = signals
            .iter()
            .filter(|signal| signal.severity == IdentityDriftSeverity::High)
            .count();
        let remediation_plan = IdentityDriftRemediationPlan::from_signals(&signals);
        let before_stable_hash = before.stable_hash();
        let after_stable_hash = after.stable_hash();
        Self {
            before_id: before.identity_id(),
            after_id: after.identity_id(),
            stable_hash_changed: before_stable_hash != after_stable_hash,
            before_stable_hash,
            after_stable_hash,
            score,
            severity,
            changed_signal_count: signals.len(),
            high_risk_signal_count,
            before: before.clone(),
            after: after.clone(),
            signals,
            remediation_plan,
        }
    }

    /// 是否存在中高风险漂移。
    pub fn has_risky_drift(&self) -> bool {
        self.severity == IdentityDriftSeverity::High
            || self.severity == IdentityDriftSeverity::Medium
    }

    /// 是否出现高风险漂移。
    pub fn has_high_risk_drift(&self) -> bool {
        self.high_risk_signal_count > 0
    }

    /// 是否看起来仍是稳定的同一画像。
    pub fn is_stable(&self) -> bool {
        self.score < 20 && !self.has_high_risk_drift()
    }
}

impl LinkabilityReport {
    /// 比较两份快照。
    pub fn compare(left: &FingerprintSnapshot, right: &FingerprintSnapshot) -> Self {
        let mut signals = Vec::new();
        compare_linkability(left, right, &mut signals);
        signals.sort_by_key(|s| match s.strength {
            LinkabilityStrength::Strong => 0,
            LinkabilityStrength::Medium => 1,
            LinkabilityStrength::Weak => 2,
        });
        let strong = signals
            .iter()
            .filter(|s| s.strength == LinkabilityStrength::Strong)
            .count();
        let score = signals
            .iter()
            .map(|s| s.strength.weight() as u16)
            .sum::<u16>()
            .min(100) as u8;
        Self {
            same_identity_likely: score >= 60 || strong >= 2,
            score,
            left: left.clone(),
            right: right.clone(),
            signals,
        }
    }

    /// 是否存在强关联信号。
    pub fn has_strong_signal(&self) -> bool {
        self.signals
            .iter()
            .any(|s| s.strength == LinkabilityStrength::Strong)
    }

    /// 是否看起来已经足够分离。
    pub fn is_distinct(&self) -> bool {
        self.score < 30 && !self.has_strong_signal()
    }
}

/// 身份池里一对可能被关联的快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkabilityPair {
    pub left_index: usize,
    pub right_index: usize,
    pub score: u8,
    pub same_identity_likely: bool,
    pub signals: Vec<LinkabilitySignal>,
}

/// 身份池中重复出现的稳定信号。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityPoolSignal {
    pub strength: LinkabilityStrength,
    pub code: String,
    pub value: String,
    pub count: usize,
    pub indexes: Vec<usize>,
    pub suggestion: String,
}

/// 身份池某个稳定信号下的取值桶。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPoolDiversityBucket {
    pub value: String,
    pub count: usize,
    pub ratio: f64,
    pub indexes: Vec<usize>,
}

/// 身份池单个稳定信号的多样性分布。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPoolDiversitySignal {
    pub strength: LinkabilityStrength,
    pub code: String,
    pub unique_count: usize,
    pub repeated_value_count: usize,
    pub max_bucket_count: usize,
    pub max_bucket_ratio: f64,
    pub buckets: Vec<IdentityPoolDiversityBucket>,
    pub suggestion: String,
}

/// 身份池多样性报告:用于发现 UA / 时区 / WebGL / canvas 等信号过度集中。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPoolDiversityReport {
    pub size: usize,
    pub signal_count: usize,
    pub concentrated_signal_count: usize,
    pub max_concentration_ratio: f64,
    pub average_unique_ratio: f64,
    pub signals: Vec<IdentityPoolDiversitySignal>,
}

impl IdentityPoolDiversityReport {
    pub fn is_diverse(&self) -> bool {
        self.concentrated_signal_count == 0
    }
}

/// 身份池画像熵状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityEntropyStatus {
    /// 样本太少或信号不足,无法判断。
    #[default]
    Unknown,
    /// 多数稳定信号集中在同一个模板上。
    Collapsed,
    /// 有明显集中桶,名义账号数显著高于画像多样性。
    Concentrated,
    /// 有一定分散度,但强信号仍有重复。
    Thin,
    /// 画像分散度较好。
    Diverse,
}

/// 单个稳定信号对身份熵预算的贡献。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityEntropySignalBudget {
    pub strength: LinkabilityStrength,
    pub code: String,
    /// Shannon entropy 归一化到 0..1。越低代表该信号越集中。
    pub normalized_entropy: f64,
    /// 该信号的 Shannon 有效取值数,即 `2^entropy`。
    pub effective_value_count: f64,
    pub unique_count: usize,
    pub max_bucket_ratio: f64,
    pub repeated_value_count: usize,
    pub suggestion: String,
}

/// 身份池画像熵预算:回答"名义 N 个账号,画像多样性大约像多少个真实身份"。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityEntropyBudget {
    pub size: usize,
    pub signal_count: usize,
    pub status: IdentityEntropyStatus,
    /// strong/medium/weak 加权后的平均归一化熵。
    pub weighted_entropy: f64,
    /// 0..100 的直观评分,由 `weighted_entropy` 映射而来。
    pub entropy_score: u8,
    /// 启发式有效身份数。它不是去重结果,而是多稳定信号综合后的画像容量估计。
    pub effective_identity_count: f64,
    pub nominal_to_effective_ratio: f64,
    pub bottleneck_count: usize,
    pub bottleneck_signals: Vec<IdentityEntropySignalBudget>,
}

impl IdentityEntropyBudget {
    pub fn is_healthy(&self) -> bool {
        matches!(self.status, IdentityEntropyStatus::Diverse)
    }
}

/// 身份池容量状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityCapacityStatus {
    #[default]
    Unknown,
    Ready,
    NeedsDiversification,
    Exhausted,
}

/// 身份池容量修复动作。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCapacityAction {
    pub code: String,
    pub priority: IdentityFixPriority,
    pub title: String,
    pub detail: String,
    pub signal_codes: Vec<String>,
    pub estimated_gain: f64,
}

/// 身份池容量计划:把身份熵预算翻译成调度器能消费的扩容/分散建议。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCapacityPlan {
    pub size: usize,
    pub status: IdentityCapacityStatus,
    pub effective_identity_count: f64,
    pub target_effective_identity_count: f64,
    pub missing_effective_identity_count: f64,
    pub nominal_to_effective_ratio: f64,
    pub target_nominal_to_effective_ratio: f64,
    pub additional_distinct_profiles_needed: usize,
    pub bottleneck_count: usize,
    pub bottleneck_signals: Vec<IdentityEntropySignalBudget>,
    pub actions: Vec<IdentityCapacityAction>,
}

/// 身份池里由风险 pair 连成的一组可关联画像。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCluster {
    /// 该风险簇包含的快照下标。
    pub indexes: Vec<usize>,
    /// 风险簇内部命中的 pair 数量。
    pub pair_count: usize,
    /// 风险簇内部最高 pair 关联分。
    pub max_score: u8,
    /// 风险簇内部强关联信号数量。
    pub strong_signal_count: usize,
    /// 风险簇内部命中过的信号码,去重排序。
    pub signal_codes: Vec<String>,
}

/// 身份池中最容易把整池拖成同一画像团的风险下标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityOffender {
    /// 快照下标。
    pub index: usize,
    /// 参与的风险 pair 数量。
    pub pair_count: usize,
    /// 该下标参与的最高 pair 关联分。
    pub max_score: u8,
    /// 该下标参与的强关联信号数量。
    pub strong_signal_count: usize,
    /// 与该下标发生风险关联的其它下标。
    pub linked_indexes: Vec<usize>,
    /// 该下标命中过的信号码,去重排序。
    pub signal_codes: Vec<String>,
}

/// 用贪心覆盖风险 pair 得出的隔离 / 替换建议。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityQuarantinePlan {
    /// 建议优先隔离或替换的快照下标。
    pub indexes: Vec<usize>,
    /// 原始风险 pair 数。
    pub covered_pair_count: usize,
    /// 按建议下标隔离后仍未覆盖的风险 pair 数。
    pub remaining_pair_count: usize,
    /// 建议下标覆盖到的最高风险分。
    pub max_covered_score: u8,
}

/// 身份池准入动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAdmissionAction {
    /// 没有需要隔离的风险画像。
    Accept,
    /// 接收一部分,隔离 / 替换一部分。
    PartialQuarantine,
    /// 全部候选都建议隔离 / 替换。
    RejectAll,
}

/// 基于 quarantine 的可执行准入计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityAdmissionPlan {
    pub action: IdentityAdmissionAction,
    pub accept_indexes: Vec<usize>,
    pub quarantine_indexes: Vec<usize>,
    pub total_count: usize,
    pub accept_count: usize,
    pub quarantine_count: usize,
}

/// 身份池修复动作影响的范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityPoolRemediationTarget {
    Admission,
    UserAgent,
    ClientHints,
    Stealth,
    LocaleProxy,
    Hardware,
    Viewport,
    GpuWebgl,
    Canvas,
}

/// 一条池级修复动作,用于把"画像团"拆开。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPoolRemediationAction {
    /// 稳定动作码,用于上层调度器匹配。
    pub code: String,
    /// 需要调整的池级配置范围。
    pub target: IdentityPoolRemediationTarget,
    /// 动作优先级。
    pub priority: IdentityFixPriority,
    /// 人类可读标题。
    pub title: String,
    /// 修复说明。
    pub detail: String,
    /// 受影响的快照下标。
    pub indexes: Vec<usize>,
    /// 受影响的快照数量。
    pub affected_count: usize,
    /// 该动作覆盖或解释的风险 pair 数量。
    pub pair_count: usize,
    /// 触发该动作的 linkability / duplicate signal codes。
    pub signal_codes: Vec<String>,
    /// 触发该动作的重复稳定值样本。
    pub values: Vec<String>,
}

/// 池级修复计划:把风险 pair、重复稳定信号和隔离建议合并成可执行动作。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPoolRemediationPlan {
    pub action_count: usize,
    pub high_priority_count: usize,
    pub quarantine_indexes: Vec<usize>,
    pub quarantine_count: usize,
    pub targets: Vec<IdentityPoolRemediationTarget>,
    pub actions: Vec<IdentityPoolRemediationAction>,
}

impl IdentityPoolRemediationPlan {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// 一批浏览器身份快照的整体自检报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityPoolReport {
    /// 身份池稳定 ID,由池内 snapshot hashes 排序后生成。
    #[serde(default, rename = "poolId", alias = "pool_id")]
    pub pool_id: String,
    /// 身份池稳定哈希。
    #[serde(default, rename = "stableHash", alias = "stable_hash")]
    pub stable_hash: String,
    /// 池内每份快照的稳定 ID,按输入顺序排列。
    #[serde(default, rename = "snapshotIds", alias = "snapshot_ids")]
    pub snapshot_ids: Vec<String>,
    /// 快照数量。
    pub size: usize,
    /// 任意两份快照之间的最高关联分。
    pub max_linkability: u8,
    /// 每份快照自己的身份一致性报告。
    pub identity_reports: Vec<IdentityReport>,
    /// 达到风险阈值的 pair。
    pub risky_pairs: Vec<LinkabilityPair>,
    /// 全池重复稳定信号聚合。
    pub duplicate_signals: Vec<IdentityPoolSignal>,
    /// 全池稳定信号的多样性分布。
    #[serde(default, rename = "diversity")]
    pub diversity: IdentityPoolDiversityReport,
    /// 池级身份熵预算,把多样性分布压缩成"有效画像容量"。
    #[serde(default, rename = "entropyBudget", alias = "entropy_budget")]
    pub entropy_budget: IdentityEntropyBudget,
    /// 池级容量计划,把熵预算翻译成扩容/分散建议。
    #[serde(default, rename = "capacityPlan", alias = "capacity_plan")]
    pub capacity_plan: IdentityCapacityPlan,
    /// 面向账号池调度器的结构化修复计划。
    #[serde(default, rename = "remediationPlan", alias = "remediation_plan")]
    pub remediation_plan: IdentityPoolRemediationPlan,
}

impl IdentityPoolReport {
    /// 分析一批快照。适合在账号池 / 浏览器池启动后做一次抽样自检。
    pub fn analyze(snapshots: &[FingerprintSnapshot]) -> Self {
        let identity_reports: Vec<_> = snapshots
            .iter()
            .map(IdentityReport::from_snapshot)
            .collect();
        let mut risky_pairs = Vec::new();
        let mut max_linkability = 0u8;

        for i in 0..snapshots.len() {
            for j in (i + 1)..snapshots.len() {
                let pair = LinkabilityReport::compare(&snapshots[i], &snapshots[j]);
                max_linkability = max_linkability.max(pair.score);
                if pair.same_identity_likely || pair.has_strong_signal() || pair.score >= 30 {
                    risky_pairs.push(LinkabilityPair {
                        left_index: i,
                        right_index: j,
                        score: pair.score,
                        same_identity_likely: pair.same_identity_likely,
                        signals: pair.signals,
                    });
                }
            }
        }

        risky_pairs.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.left_index.cmp(&b.left_index))
                .then_with(|| a.right_index.cmp(&b.right_index))
        });

        let snapshot_ids = identity_reports
            .iter()
            .map(|report| report.identity_id.clone())
            .collect::<Vec<_>>();
        let stable_hash = pool_stable_hash(&identity_reports);
        let diversity = build_pool_diversity_report(snapshots);
        let entropy_budget = build_identity_entropy_budget(&diversity);
        let capacity_plan = build_identity_capacity_plan(&entropy_budget);
        let mut report = Self {
            pool_id: format!("pool_{stable_hash}"),
            stable_hash,
            snapshot_ids,
            size: snapshots.len(),
            max_linkability,
            identity_reports,
            risky_pairs,
            duplicate_signals: collect_pool_duplicates(snapshots),
            diversity,
            entropy_budget,
            capacity_plan,
            remediation_plan: IdentityPoolRemediationPlan::default(),
        };
        report.remediation_plan = build_pool_remediation_plan(&report);
        report
    }

    /// 是否这批画像看起来已经足够分散且单体没有高风险问题。
    pub fn is_well_separated(&self) -> bool {
        self.risky_pairs.is_empty()
            && self.max_linkability < 30
            && self.identity_reports.iter().all(IdentityReport::is_healthy)
    }

    /// 是否存在容易被关联的 pair。
    pub fn has_risky_pairs(&self) -> bool {
        !self.risky_pairs.is_empty()
    }

    /// 把风险 pair 合并为连通簇,用于快速定位"哪些账号 / 画像会被归成同一团"。
    pub fn risk_clusters(&self) -> Vec<IdentityCluster> {
        build_identity_clusters(self.size, &self.risky_pairs)
    }

    /// 按风险贡献排序,列出最应该优先替换 / 隔离的画像下标。
    pub fn risk_offenders(&self) -> Vec<IdentityOffender> {
        build_identity_offenders(self.size, &self.risky_pairs)
    }

    /// 基于风险 pair 的贪心隔离计划,用于回答"先替换哪几个画像最划算"。
    pub fn quarantine_plan(&self) -> IdentityQuarantinePlan {
        build_quarantine_plan(self.size, &self.risky_pairs)
    }

    /// 基于隔离计划给出可直接执行的接收 / 隔离决策。
    pub fn admission_plan(&self) -> IdentityAdmissionPlan {
        build_admission_plan(self.size, &self.quarantine_plan())
    }

    /// 面向账号池调度器的结构化修复计划。
    pub fn remediation_plan(&self) -> IdentityPoolRemediationPlan {
        build_pool_remediation_plan(self)
    }

    /// 全池稳定信号多样性分布。
    pub fn diversity_report(&self) -> IdentityPoolDiversityReport {
        self.diversity.clone()
    }

    /// 池级身份熵预算。
    pub fn entropy_budget(&self) -> IdentityEntropyBudget {
        self.entropy_budget.clone()
    }

    /// 池级容量计划。
    pub fn capacity_plan(&self) -> IdentityCapacityPlan {
        self.capacity_plan.clone()
    }
}

/// 采集 [`FingerprintSnapshot`] 的探针 JS:建临时 canvas 取渲染像素哈希 + 读 WebGL UNMASKED renderer
/// + navigator/screen/Intl 信号,`JSON.stringify` 一次性返回。
const FINGERPRINT_JS: &str = r#"(function(){
  function canvasHash(){
    try{
      var c=document.createElement('canvas'); c.width=220; c.height=50;
      var x=c.getContext('2d');
      x.textBaseline='top'; x.font='14px Arial'; x.fillStyle='#069'; x.fillText('drission-fp-😀',2,2);
      x.fillStyle='rgba(102,200,0,0.7)'; x.fillText('drission-fp',4,17);
      var u=c.toDataURL(); var h=0;
      for(var i=0;i<u.length;i++){ h=(h*31+u.charCodeAt(i))>>>0; }
      return ('00000000'+h.toString(16)).slice(-8);
    }catch(e){ return 'err'; }
  }
  function webgl(){
    try{
      var c=document.createElement('canvas'); var gl=c.getContext('webgl')||c.getContext('experimental-webgl');
      if(!gl) return 'none';
      var dbg=gl.getExtension('WEBGL_debug_renderer_info');
      return dbg? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
    }catch(e){ return 'err'; }
  }
  return JSON.stringify({
    ua: navigator.userAgent,
    platform: navigator.platform,
    uaDataPlatform: navigator.userAgentData ? (navigator.userAgentData.platform||'') : '',
    uaDataMobile: navigator.userAgentData ? !!navigator.userAgentData.mobile : false,
    webdriver: navigator.webdriver === true,
    languages: (navigator.languages||[]).join(','),
    maxTouchPoints: navigator.maxTouchPoints||0,
    hardwareConcurrency: navigator.hardwareConcurrency||0,
    deviceMemory: navigator.deviceMemory||0,
    screen: screen.width+'x'+screen.height,
    devicePixelRatio: window.devicePixelRatio,
    timezone: (Intl.DateTimeFormat().resolvedOptions()||{}).timeZone||'',
    webglRenderer: webgl(),
    canvasHash: canvasHash()
  });
})()"#;

/// **实时指纹读取**能力(CDP / Camoufox 两后端实现;`use drission::prelude::*` 后挂到 `tab` 上)。
#[async_trait::async_trait]
pub trait FingerprintProbe {
    /// 底层求值(各后端委托给固有 `run_js`)。
    async fn fp_eval(&self, js: &str) -> Result<Value>;

    /// 读取当前页面的实时 [`FingerprintSnapshot`]。
    ///
    /// 需当前已在某个文档上(`canvas`/`webgl` 探针要 DOM;`about:blank` 也可,空白页同样能建 canvas)。
    async fn fingerprint_snapshot(&self) -> Result<FingerprintSnapshot> {
        let v = self.fp_eval(FINGERPRINT_JS).await?;
        Ok(FingerprintSnapshot::from_probe(&v))
    }

    /// 读取当前页面指纹并生成身份一致性诊断报告。
    async fn identity_report(&self) -> Result<IdentityReport> {
        Ok(self.fingerprint_snapshot().await?.diagnose())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalOs {
    Windows,
    Mac,
    Linux,
    Android,
    Ios,
    Unknown,
}

fn diagnose_identity(fp: &FingerprintSnapshot, issues: &mut Vec<IdentityIssue>) {
    let ua_os = os_from_ua(&fp.ua);
    let platform_os = os_from_platform(&fp.platform);
    let ua_data_os = os_from_ua_data_platform(&fp.ua_data_platform);
    let mobile_ua = is_mobile_ua(&fp.ua);

    if fp.ua.trim().is_empty() {
        push_issue(
            issues,
            IdentitySeverity::High,
            "ua.empty",
            "navigator.userAgent 为空",
            "保留真实浏览器 UA,或用后端选项设置一条完整且与平台一致的 UA。",
        );
    }
    if fp.ua.contains("HeadlessChrome") {
        push_issue(
            issues,
            IdentitySeverity::High,
            "ua.headless_chrome",
            "UA 暴露 HeadlessChrome",
            "开启无头 UA 伪装,或使用有头模式 / new headless 并补齐 Client Hints。",
        );
    }
    if fp.webdriver {
        push_issue(
            issues,
            IdentitySeverity::High,
            "navigator.webdriver",
            "navigator.webdriver 为 true",
            "启用 stealth 初始化脚本,并避免让页面过早读取 webdriver。",
        );
    }

    if fp.platform.trim().is_empty() {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "platform.empty",
            "navigator.platform 为空",
            "确保 platform 与 UA 中的操作系统保持一致。",
        );
    } else if os_conflict(ua_os, platform_os, fp.max_touch_points) {
        push_issue(
            issues,
            IdentitySeverity::High,
            "platform.os_mismatch",
            format!(
                "UA 操作系统与 navigator.platform 冲突: UA={:?}, platform={:?}",
                ua_os, platform_os
            ),
            "统一 UA、navigator.platform、Client Hints 与 WebGL 画像。",
        );
    }

    if !fp.ua_data_platform.trim().is_empty() && os_conflict(ua_os, ua_data_os, fp.max_touch_points)
    {
        push_issue(
            issues,
            IdentitySeverity::High,
            "client_hints.platform_mismatch",
            format!(
                "UA 操作系统与 userAgentData.platform 冲突: UA={:?}, uaData={:?}",
                ua_os, ua_data_os
            ),
            "补齐 Emulation.setUserAgentOverride 的 userAgentMetadata,不要只改 UA 字符串。",
        );
    } else if looks_chromium(&fp.ua) && fp.ua_data_platform.trim().is_empty() {
        push_issue(
            issues,
            IdentitySeverity::Low,
            "client_hints.missing",
            "Chromium UA 下未读到 userAgentData.platform",
            "若目标站点使用高熵 Client Hints,请验证 UA metadata 是否被启动参数清空。",
        );
    }

    if fp.ua_data_mobile != mobile_ua && looks_chromium(&fp.ua) {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "client_hints.mobile_mismatch",
            format!(
                "UA 移动端标记与 userAgentData.mobile 不一致: ua_mobile={}, uaDataMobile={}",
                mobile_ua, fp.ua_data_mobile
            ),
            "移动端画像要同时设置 UA、viewport、touch、userAgentMetadata.mobile。",
        );
    }
    if mobile_ua && fp.max_touch_points == 0 {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "touch.missing_mobile",
            "移动端 UA 但 maxTouchPoints 为 0",
            "移动端画像需启用触摸模拟并设置合理 viewport。",
        );
    }
    if !mobile_ua && fp.max_touch_points > 5 {
        push_issue(
            issues,
            IdentitySeverity::Low,
            "touch.desktop_many_points",
            format!("桌面 UA 但 maxTouchPoints={} 偏高", fp.max_touch_points),
            "确认是否误用了移动端设备预设或触摸模拟。",
        );
    }

    if fp.languages.trim().is_empty() {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "languages.empty",
            "navigator.languages 为空",
            "保留真实语言列表,或让 locale / Accept-Language / navigator.languages 一起变化。",
        );
    }
    if fp.timezone.trim().is_empty() {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "timezone.empty",
            "Intl 时区为空",
            "设置有效 IANA 时区,并让它与代理出口地区、语言相互自洽。",
        );
    } else if let Some(sev) = language_timezone_conflict(&fp.languages, &fp.timezone) {
        push_issue(
            issues,
            sev,
            "locale.timezone_mismatch",
            format!(
                "语言与时区可能不自洽: languages={}, timezone={}",
                fp.languages, fp.timezone
            ),
            "代理出口、locale、timezone 最好来自同一身份画像。",
        );
    }

    if fp.hardware_concurrency == 0 {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "hardware_concurrency.zero",
            "hardwareConcurrency 为 0",
            "设置合理 CPU 核数,避免被识别为异常环境。",
        );
    } else if fp.hardware_concurrency > 64 {
        push_issue(
            issues,
            IdentitySeverity::Low,
            "hardware_concurrency.large",
            format!("hardwareConcurrency={} 异常偏大", fp.hardware_concurrency),
            "普通消费级画像建议使用 4-16 核范围。",
        );
    }

    if fp.device_pixel_ratio <= 0.0 {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "screen.dpr_invalid",
            "devicePixelRatio 无效",
            "确认页面运行在真实浏览器上下文,并设置合理 viewport/screen。",
        );
    }
    if !valid_screen(&fp.screen) {
        push_issue(
            issues,
            IdentitySeverity::Medium,
            "screen.invalid",
            format!("screen 分辨率无效: {}", fp.screen),
            "设置常见桌面或移动端分辨率,避免 0x0 或异常超大屏幕。",
        );
    }

    let webgl = fp.webgl_renderer.to_ascii_lowercase();
    if fp.webgl_renderer.trim().is_empty() || webgl == "none" || webgl == "err" {
        push_issue(
            issues,
            IdentitySeverity::High,
            "webgl.missing",
            format!("WebGL renderer 不可用: {}", fp.webgl_renderer),
            "在容器/无头环境中启用可用 GPU 或一致的 WebGL 补环境。",
        );
    } else {
        if webgl.contains("swiftshader") || webgl.contains("llvmpipe") || webgl.contains("software")
        {
            push_issue(
                issues,
                IdentitySeverity::High,
                "webgl.software_renderer",
                format!("WebGL 暴露软件渲染: {}", fp.webgl_renderer),
                "优先使用真实 GPU;无 GPU 时使用与 OS 自洽的 renderer 补环境。",
            );
        }
        if webgl_os_conflict(ua_os, &webgl) {
            push_issue(
                issues,
                IdentitySeverity::High,
                "webgl.os_mismatch",
                format!(
                    "WebGL renderer 与 UA 操作系统冲突: UA={:?}, renderer={}",
                    ua_os, fp.webgl_renderer
                ),
                "WebGL vendor/renderer 要与 UA、platform、操作系统画像一致。",
            );
        }
    }

    if fp.canvas_hash.trim().is_empty() || fp.canvas_hash == "err" {
        push_issue(
            issues,
            IdentitySeverity::High,
            "canvas.missing",
            format!("canvas hash 不可用: {}", fp.canvas_hash),
            "确认页面允许 canvas 渲染;不要把 canvas API 破坏成空值或异常。",
        );
    } else if fp.canvas_hash.len() != 8 {
        push_issue(
            issues,
            IdentitySeverity::Low,
            "canvas.hash_shape",
            format!("canvas hash 形态异常: {}", fp.canvas_hash),
            "确认 canvas 探针返回稳定的 8 位 hex 哈希。",
        );
    }
}

fn compare_linkability(
    left: &FingerprintSnapshot,
    right: &FingerprintSnapshot,
    signals: &mut Vec<LinkabilitySignal>,
) {
    if same_norm(&left.ua, &right.ua) {
        push_link_signal(
            signals,
            LinkabilityStrength::Medium,
            "ua.same",
            "两份快照的 userAgent 完全相同",
            "大规模账号池至少应让 UA 与操作系统、浏览器版本、Client Hints 按画像分组轮换。",
        );
    }
    if same_norm(&left.platform, &right.platform) {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "platform.same",
            "navigator.platform 相同",
            "跨画像轮换时要同时调整 platform、UA、WebGL、Client Hints。",
        );
    }
    if same_norm(&left.ua_data_platform, &right.ua_data_platform) {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "client_hints.platform_same",
            "userAgentData.platform 相同",
            "Client Hints 应随身份画像一起变化,不要只改 UA 字符串。",
        );
    }
    if left.ua_data_mobile == right.ua_data_mobile
        && (is_mobile_ua(&left.ua) || is_mobile_ua(&right.ua) || left.ua_data_mobile)
    {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "client_hints.mobile_same",
            "移动端 Client Hints 标记相同",
            "移动 / 桌面画像应成组切换 UA、viewport、touch 与 userAgentMetadata.mobile。",
        );
    }
    if left.webdriver && right.webdriver {
        push_link_signal(
            signals,
            LinkabilityStrength::Strong,
            "webdriver.both_true",
            "两份快照都暴露 navigator.webdriver=true",
            "先消除自动化强信号,否则不同账号会被同一自动化特征聚类。",
        );
    }
    if same_norm(&left.languages, &right.languages) {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "languages.same",
            "navigator.languages 相同",
            "语言、Accept-Language、时区、代理地区最好由同一画像生成。",
        );
    }
    if same_norm(&left.timezone, &right.timezone) {
        push_link_signal(
            signals,
            LinkabilityStrength::Medium,
            "timezone.same",
            "Intl 时区相同",
            "多地区账号池应让代理出口与 timezone 一起轮换。",
        );
    }
    if same_norm(&left.screen, &right.screen) && left.device_pixel_ratio == right.device_pixel_ratio
    {
        push_link_signal(
            signals,
            LinkabilityStrength::Medium,
            "screen.same",
            "screen 分辨率与 devicePixelRatio 都相同",
            "为不同画像分配常见但不完全一致的 viewport/screen/DPR 组合。",
        );
    } else if same_norm(&left.screen, &right.screen) {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "screen.resolution_same",
            "screen 分辨率相同",
            "如果账号池规模较大,不要让所有上下文共享同一个屏幕画像。",
        );
    }
    if left.hardware_concurrency > 0 && left.hardware_concurrency == right.hardware_concurrency {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "hardware_concurrency.same",
            format!("hardwareConcurrency 相同: {}", left.hardware_concurrency),
            "CPU 核数应落在合理范围内,并随画像池分散。",
        );
    }
    if left.device_memory > 0.0 && (left.device_memory - right.device_memory).abs() < f64::EPSILON {
        push_link_signal(
            signals,
            LinkabilityStrength::Weak,
            "device_memory.same",
            format!("deviceMemory 相同: {}", left.device_memory),
            "内存容量是低熵信号,和其它弱信号叠加后仍会增强关联。",
        );
    }
    if same_stable(&left.webgl_renderer, &right.webgl_renderer) {
        push_link_signal(
            signals,
            LinkabilityStrength::Strong,
            "webgl.same",
            "WebGL renderer 完全相同",
            "不同机器画像应分散 WebGL vendor/renderer,并保持与 OS/UA 自洽。",
        );
    }
    if same_stable(&left.canvas_hash, &right.canvas_hash) {
        push_link_signal(
            signals,
            LinkabilityStrength::Strong,
            "canvas.same",
            "canvas hash 完全相同",
            "不同画像应使用稳定但各异的 canvas/audio 噪声种子。",
        );
    }
    if left.ua.contains("HeadlessChrome") && right.ua.contains("HeadlessChrome") {
        push_link_signal(
            signals,
            LinkabilityStrength::Strong,
            "ua.both_headless_chrome",
            "两份快照都暴露 HeadlessChrome",
            "先修正无头 UA 与 Client Hints,再谈多账号隔离。",
        );
    }
}

fn compare_identity_drift(
    before: &FingerprintSnapshot,
    after: &FingerprintSnapshot,
    signals: &mut Vec<IdentityDriftSignal>,
) {
    let before_ua_os = os_from_ua(&before.ua);
    let after_ua_os = os_from_ua(&after.ua);
    if changed_norm(&before.ua, &after.ua) {
        let os_changed = known_os_changed(before_ua_os, after_ua_os);
        let major_changed = browser_major(&before.ua) != browser_major(&after.ua);
        let severity = if os_changed {
            IdentityDriftSeverity::High
        } else if major_changed {
            IdentityDriftSeverity::Medium
        } else {
            IdentityDriftSeverity::Low
        };
        push_drift_signal(
            signals,
            severity,
            "ua.changed",
            &before.ua,
            &after.ua,
            "userAgent 发生变化",
            if os_changed {
                "同一账号画像不应跨操作系统跳变;检查 profile 分配、浏览器后端和 UA/Client Hints 配置。"
            } else if major_changed {
                "浏览器大版本变化会改变 Client Hints、TLS/HTTP2 与 JS 指纹,建议作为一次受控画像升级记录。"
            } else {
                "小版本变化通常可接受,但应确认 Client Hints 与 UA 同步更新。"
            },
        );
    }

    if changed_norm(&before.platform, &after.platform) {
        let severity = if known_os_changed(
            os_from_platform(&before.platform),
            os_from_platform(&after.platform),
        ) {
            IdentityDriftSeverity::High
        } else {
            IdentityDriftSeverity::Medium
        };
        push_drift_signal(
            signals,
            severity,
            "platform.changed",
            &before.platform,
            &after.platform,
            "navigator.platform 发生变化",
            "同一账号的 OS/platform 应保持稳定;如需迁移设备,应显式生成新身份画像并更新 baseline。",
        );
    }

    if changed_norm(&before.ua_data_platform, &after.ua_data_platform) {
        let severity = if known_os_changed(
            os_from_ua_data_platform(&before.ua_data_platform),
            os_from_ua_data_platform(&after.ua_data_platform),
        ) {
            IdentityDriftSeverity::High
        } else {
            IdentityDriftSeverity::Medium
        };
        push_drift_signal(
            signals,
            severity,
            "client_hints.platform_changed",
            &before.ua_data_platform,
            &after.ua_data_platform,
            "userAgentData.platform 发生变化",
            "Client Hints 应跟 UA/OS 画像一起受控更新,不要只在某一轮丢失或改写 metadata。",
        );
    }

    if before.ua_data_mobile != after.ua_data_mobile {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::High,
            "client_hints.mobile_changed",
            &before.ua_data_mobile.to_string(),
            &after.ua_data_mobile.to_string(),
            "userAgentData.mobile 发生变化",
            "同一账号不应在移动/桌面画像之间随机切换;检查设备预设和 Client Hints metadata。",
        );
    }

    if before.webdriver != after.webdriver {
        let severity = if after.webdriver {
            IdentityDriftSeverity::High
        } else {
            IdentityDriftSeverity::Low
        };
        push_drift_signal(
            signals,
            severity,
            "webdriver.changed",
            &before.webdriver.to_string(),
            &after.webdriver.to_string(),
            "navigator.webdriver 发生变化",
            if after.webdriver {
                "当前轮暴露了 webdriver=true,应立即隔离该画像并修复 stealth 初始化时机。"
            } else {
                "webdriver 从 true 变为 false 是修复结果,仍建议记录为一次画像行为变化。"
            },
        );
    }

    if changed_norm(&before.languages, &after.languages) {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::Medium,
            "languages.changed",
            &before.languages,
            &after.languages,
            "navigator.languages 发生变化",
            "语言应与账号地区、代理出口和 timezone 绑定;跨轮变化需要同步更新整组 locale 画像。",
        );
    }

    if changed_norm(&before.timezone, &after.timezone) {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::Medium,
            "timezone.changed",
            &before.timezone,
            &after.timezone,
            "Intl timezone 发生变化",
            "同一账号的 timezone 不应随代理临时漂移;应按账号画像固定代理地区或更新 baseline。",
        );
    }

    if before.max_touch_points != after.max_touch_points {
        let severity = if is_mobile_ua(&before.ua)
            || is_mobile_ua(&after.ua)
            || before.ua_data_mobile
            || after.ua_data_mobile
        {
            IdentityDriftSeverity::Medium
        } else {
            IdentityDriftSeverity::Low
        };
        push_drift_signal(
            signals,
            severity,
            "touch.changed",
            &before.max_touch_points.to_string(),
            &after.max_touch_points.to_string(),
            "navigator.maxTouchPoints 发生变化",
            "touch 能力应随移动/桌面画像稳定存在;检查设备模拟是否在某轮缺失。",
        );
    }

    if before.hardware_concurrency != after.hardware_concurrency {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::Low,
            "hardware_concurrency.changed",
            &before.hardware_concurrency.to_string(),
            &after.hardware_concurrency.to_string(),
            "hardwareConcurrency 发生变化",
            "CPU 核数是低熵信号,单独变化风险较低,但应避免和其它硬件信号一起漂移。",
        );
    }

    if format_float(before.device_memory) != format_float(after.device_memory) {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::Low,
            "device_memory.changed",
            &format_float(before.device_memory),
            &format_float(after.device_memory),
            "deviceMemory 发生变化",
            "内存容量变化通常来自设备画像切换;与 CPU/WebGL 同时变化时应视为 profile 漂移。",
        );
    }

    let before_screen = screen_dpr_value(before);
    let after_screen = screen_dpr_value(after);
    if before_screen != after_screen {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::Medium,
            "screen.changed",
            &before_screen,
            &after_screen,
            "screen / devicePixelRatio 发生变化",
            "同一账号可有窗口尺寸变化,但 screen 与 DPR 属于设备画像;应避免跨轮随机漂移。",
        );
    }

    if changed_stable(&before.webgl_renderer, &after.webgl_renderer) {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::High,
            "webgl.changed",
            &before.webgl_renderer,
            &after.webgl_renderer,
            "WebGL renderer 发生变化",
            "WebGL renderer 是强稳定信号;同一账号跨轮变化通常意味着机器画像或 GPU 补环境错配。",
        );
    }

    if changed_stable(&before.canvas_hash, &after.canvas_hash) {
        push_drift_signal(
            signals,
            IdentityDriftSeverity::High,
            "canvas.changed",
            &before.canvas_hash,
            &after.canvas_hash,
            "canvas hash 发生变化",
            "同一账号应使用稳定 canvas/audio 噪声种子;漂移会让历史行为像来自新设备。",
        );
    }

    let before_headless = before.ua.contains("HeadlessChrome");
    let after_headless = after.ua.contains("HeadlessChrome");
    if before_headless != after_headless {
        push_drift_signal(
            signals,
            if after_headless {
                IdentityDriftSeverity::High
            } else {
                IdentityDriftSeverity::Medium
            },
            "ua.headless_chrome_changed",
            &before_headless.to_string(),
            &after_headless.to_string(),
            "HeadlessChrome UA 暴露状态发生变化",
            "HeadlessChrome 是高风险自动化信号;出现时应隔离当前画像并修复 UA/Client Hints。",
        );
    }
}

#[derive(Debug, Clone)]
struct DriftRemediationSpec {
    code: &'static str,
    target: IdentityDriftRemediationTarget,
    title: &'static str,
    detail: &'static str,
    fields: &'static [&'static str],
}

#[derive(Debug, Clone)]
struct DriftRemediationAcc {
    spec: DriftRemediationSpec,
    priority: IdentityFixPriority,
    fields: BTreeSet<String>,
    signal_codes: BTreeSet<String>,
    before_values: BTreeSet<String>,
    after_values: BTreeSet<String>,
}

fn build_identity_drift_remediation_plan(
    signals: &[IdentityDriftSignal],
) -> IdentityDriftRemediationPlan {
    if signals.is_empty() {
        return IdentityDriftRemediationPlan::default();
    }

    let mut actions: BTreeMap<String, DriftRemediationAcc> = BTreeMap::new();
    let high_signals = signals
        .iter()
        .filter(|signal| signal.severity == IdentityDriftSeverity::High)
        .collect::<Vec<_>>();
    if !high_signals.is_empty() {
        let spec = DriftRemediationSpec {
            code: "drift.quarantine_current",
            target: IdentityDriftRemediationTarget::Admission,
            title: "先隔离当前漂移画像",
            detail: "同一账号出现高风险画像漂移时,先从可用池隔离当前浏览器/profile,避免继续用漂移身份发起任务。",
            fields: &[],
        };
        let mut acc = DriftRemediationAcc {
            spec,
            priority: IdentityFixPriority::High,
            fields: BTreeSet::new(),
            signal_codes: BTreeSet::new(),
            before_values: BTreeSet::new(),
            after_values: BTreeSet::new(),
        };
        for signal in high_signals {
            acc.signal_codes.insert(signal.code.clone());
        }
        actions.insert(acc.spec.code.to_string(), acc);
    }

    for signal in signals {
        let Some(spec) = drift_action_for_signal(signal.code.as_str()) else {
            continue;
        };
        let priority = drift_fix_priority(signal.severity);
        let acc = actions
            .entry(spec.code.to_string())
            .or_insert_with(|| DriftRemediationAcc {
                spec,
                priority,
                fields: BTreeSet::new(),
                signal_codes: BTreeSet::new(),
                before_values: BTreeSet::new(),
                after_values: BTreeSet::new(),
            });
        if fix_priority_rank(priority) > fix_priority_rank(acc.priority) {
            acc.priority = priority;
        }
        acc.fields
            .extend(acc.spec.fields.iter().map(|field| (*field).to_string()));
        acc.signal_codes.insert(signal.code.clone());
        if !signal.before.trim().is_empty() {
            acc.before_values.insert(signal.before.clone());
        }
        if !signal.after.trim().is_empty() {
            acc.after_values.insert(signal.after.clone());
        }
    }

    let mut actions = actions
        .into_values()
        .map(|acc| IdentityDriftRemediationAction {
            code: acc.spec.code.to_string(),
            target: acc.spec.target,
            priority: acc.priority,
            title: acc.spec.title.to_string(),
            detail: acc.spec.detail.to_string(),
            fields: acc.fields.into_iter().collect(),
            signal_codes: acc.signal_codes.into_iter().collect(),
            before_values: acc.before_values.into_iter().collect(),
            after_values: acc.after_values.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    actions.sort_by(|a, b| {
        fix_priority_rank(b.priority)
            .cmp(&fix_priority_rank(a.priority))
            .then_with(|| a.code.cmp(&b.code))
    });

    let targets = actions
        .iter()
        .map(|action| action.target)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let high_priority_count = actions
        .iter()
        .filter(|action| action.priority == IdentityFixPriority::High)
        .count();

    IdentityDriftRemediationPlan {
        action_count: actions.len(),
        high_priority_count,
        targets,
        actions,
    }
}

fn drift_action_for_signal(code: &str) -> Option<DriftRemediationSpec> {
    Some(match code {
        "canvas.changed" => DriftRemediationSpec {
            code: "drift.restore_canvas_seed",
            target: IdentityDriftRemediationTarget::Canvas,
            title: "恢复账号绑定的 canvas 指纹种子",
            detail: "同一账号跨轮 canvas hash 漂移会像换了设备;应恢复该账号绑定的稳定 canvas/audio 噪声种子。",
            fields: &["canvasHash"],
        },
        "webgl.changed" => DriftRemediationSpec {
            code: "drift.restore_webgl_renderer",
            target: IdentityDriftRemediationTarget::GpuWebgl,
            title: "恢复账号绑定的 WebGL renderer",
            detail: "WebGL renderer 是强稳定信号;应恢复账号原设备画像,或显式记录一次设备迁移后再更新 baseline。",
            fields: &["webglRenderer"],
        },
        "webdriver.changed" => DriftRemediationSpec {
            code: "drift.hide_webdriver",
            target: IdentityDriftRemediationTarget::Stealth,
            title: "修复 webdriver 漂移",
            detail: "当前轮如果暴露 navigator.webdriver=true,应修复 stealth 初始化并重新采样后才能入池。",
            fields: &["webdriver"],
        },
        "ua.headless_chrome_changed" => DriftRemediationSpec {
            code: "drift.remove_headless_ua",
            target: IdentityDriftRemediationTarget::UserAgent,
            title: "移除 HeadlessChrome UA 漂移",
            detail: "HeadlessChrome 暴露状态变化属于高风险自动化漂移;同步修复 UA 与 Client Hints。",
            fields: &["ua"],
        },
        "ua.changed" => DriftRemediationSpec {
            code: "drift.sync_user_agent",
            target: IdentityDriftRemediationTarget::UserAgent,
            title: "同步或确认 UA 版本漂移",
            detail: "小版本漂移可作为受控浏览器升级记录;大版本或 OS 漂移应同步 Client Hints、TLS/HTTP2 与 baseline。",
            fields: &["ua"],
        },
        "platform.changed" => DriftRemediationSpec {
            code: "drift.restore_os_profile",
            target: IdentityDriftRemediationTarget::ProfileOs,
            title: "恢复账号绑定的 OS/platform 画像",
            detail: "同一账号的 OS/platform 不应随机变化;如需迁移设备,应显式生成新画像并更新 baseline。",
            fields: &["platform"],
        },
        "client_hints.platform_changed" => DriftRemediationSpec {
            code: "drift.sync_client_hints",
            target: IdentityDriftRemediationTarget::ClientHints,
            title: "同步 Client Hints 平台画像",
            detail: "Client Hints 应与 UA/OS 一起受控更新,避免某一轮 metadata 缺失或错配。",
            fields: &["uaDataPlatform"],
        },
        "client_hints.mobile_changed" => DriftRemediationSpec {
            code: "drift.restore_device_class",
            target: IdentityDriftRemediationTarget::ClientHints,
            title: "恢复移动/桌面设备类别",
            detail: "同一账号不应在移动和桌面画像之间随机切换;同步 UA、viewport、touch 与 Client Hints。",
            fields: &["uaDataMobile"],
        },
        "languages.changed" | "timezone.changed" => DriftRemediationSpec {
            code: "drift.rebind_locale_proxy",
            target: IdentityDriftRemediationTarget::LocaleProxy,
            title: "恢复 locale / timezone 与代理出口绑定",
            detail: "语言和时区应与账号地区及代理出口绑定;临时代理漂移不应直接污染账号画像。",
            fields: &["languages", "timezone"],
        },
        "touch.changed" => DriftRemediationSpec {
            code: "drift.restore_touch_profile",
            target: IdentityDriftRemediationTarget::Touch,
            title: "恢复触摸能力画像",
            detail: "touch 能力应随移动/桌面画像稳定存在;检查设备模拟是否在当前轮缺失。",
            fields: &["maxTouchPoints"],
        },
        "hardware_concurrency.changed" | "device_memory.changed" => DriftRemediationSpec {
            code: "drift.restore_hardware_profile",
            target: IdentityDriftRemediationTarget::Hardware,
            title: "恢复低熵硬件画像",
            detail: "CPU 核数和内存单独变化风险较低,但与 WebGL/viewport 同时漂移时应按设备画像整体恢复。",
            fields: &["hardwareConcurrency", "deviceMemory"],
        },
        "screen.changed" => DriftRemediationSpec {
            code: "drift.restore_viewport_profile",
            target: IdentityDriftRemediationTarget::Viewport,
            title: "恢复 screen / DPR 画像",
            detail: "同一账号可调整窗口尺寸,但 screen 与 DPR 属于设备画像,应避免跨轮随机变化。",
            fields: &["screen", "devicePixelRatio"],
        },
        _ => return None,
    })
}

fn drift_fix_priority(severity: IdentityDriftSeverity) -> IdentityFixPriority {
    match severity {
        IdentityDriftSeverity::High => IdentityFixPriority::High,
        IdentityDriftSeverity::Medium => IdentityFixPriority::Medium,
        IdentityDriftSeverity::None | IdentityDriftSeverity::Low => IdentityFixPriority::Low,
    }
}

fn build_identity_clusters(size: usize, pairs: &[LinkabilityPair]) -> Vec<IdentityCluster> {
    if size == 0 || pairs.is_empty() {
        return Vec::new();
    }

    let mut parent: Vec<_> = (0..size).collect();
    for pair in pairs {
        if pair.left_index < size && pair.right_index < size {
            union_roots(&mut parent, pair.left_index, pair.right_index);
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for index in 0..size {
        let root = find_root(&mut parent, index);
        groups.entry(root).or_default().push(index);
    }

    let mut clusters = Vec::new();
    for indexes in groups.into_values().filter(|indexes| indexes.len() > 1) {
        let index_set: BTreeSet<_> = indexes.iter().copied().collect();
        let mut pair_count = 0usize;
        let mut max_score = 0u8;
        let mut strong_signal_count = 0usize;
        let mut signal_codes = BTreeSet::new();

        for pair in pairs.iter().filter(|pair| {
            index_set.contains(&pair.left_index) && index_set.contains(&pair.right_index)
        }) {
            pair_count += 1;
            max_score = max_score.max(pair.score);
            for signal in &pair.signals {
                if signal.strength == LinkabilityStrength::Strong {
                    strong_signal_count += 1;
                }
                signal_codes.insert(signal.code.clone());
            }
        }

        if pair_count > 0 {
            clusters.push(IdentityCluster {
                indexes,
                pair_count,
                max_score,
                strong_signal_count,
                signal_codes: signal_codes.into_iter().collect(),
            });
        }
    }

    clusters.sort_by(|a, b| {
        b.max_score
            .cmp(&a.max_score)
            .then_with(|| b.indexes.len().cmp(&a.indexes.len()))
            .then_with(|| a.indexes.cmp(&b.indexes))
    });
    clusters
}

#[derive(Debug, Default)]
struct OffenderAcc {
    pair_count: usize,
    max_score: u8,
    strong_signal_count: usize,
    linked_indexes: BTreeSet<usize>,
    signal_codes: BTreeSet<String>,
}

fn build_identity_offenders(size: usize, pairs: &[LinkabilityPair]) -> Vec<IdentityOffender> {
    if size == 0 || pairs.is_empty() {
        return Vec::new();
    }

    let mut offenders: BTreeMap<usize, OffenderAcc> = BTreeMap::new();
    for pair in pairs {
        if pair.left_index >= size || pair.right_index >= size {
            continue;
        }
        add_offender_pair(
            &mut offenders,
            pair.left_index,
            pair.right_index,
            pair.score,
            &pair.signals,
        );
        add_offender_pair(
            &mut offenders,
            pair.right_index,
            pair.left_index,
            pair.score,
            &pair.signals,
        );
    }

    let mut out = offenders
        .into_iter()
        .map(|(index, acc)| IdentityOffender {
            index,
            pair_count: acc.pair_count,
            max_score: acc.max_score,
            strong_signal_count: acc.strong_signal_count,
            linked_indexes: acc.linked_indexes.into_iter().collect(),
            signal_codes: acc.signal_codes.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.max_score
            .cmp(&a.max_score)
            .then_with(|| b.pair_count.cmp(&a.pair_count))
            .then_with(|| b.strong_signal_count.cmp(&a.strong_signal_count))
            .then_with(|| a.index.cmp(&b.index))
    });
    out
}

fn add_offender_pair(
    offenders: &mut BTreeMap<usize, OffenderAcc>,
    index: usize,
    linked: usize,
    score: u8,
    signals: &[LinkabilitySignal],
) {
    let acc = offenders.entry(index).or_default();
    acc.pair_count += 1;
    acc.max_score = acc.max_score.max(score);
    acc.linked_indexes.insert(linked);
    for signal in signals {
        if signal.strength == LinkabilityStrength::Strong {
            acc.strong_signal_count += 1;
        }
        acc.signal_codes.insert(signal.code.clone());
    }
}

fn build_quarantine_plan(size: usize, pairs: &[LinkabilityPair]) -> IdentityQuarantinePlan {
    if size == 0 || pairs.is_empty() {
        return IdentityQuarantinePlan {
            indexes: Vec::new(),
            covered_pair_count: 0,
            remaining_pair_count: 0,
            max_covered_score: 0,
        };
    }

    let mut remaining: BTreeSet<usize> = (0..pairs.len()).collect();
    let mut indexes = Vec::new();
    let mut max_covered_score = 0u8;

    while !remaining.is_empty() {
        let mut best: Option<(usize, u8, usize, usize)> = None;
        for index in 0..size {
            if indexes.contains(&index) {
                continue;
            }
            let mut cover_count = 0usize;
            let mut best_score = 0u8;
            for pair_index in &remaining {
                let pair = &pairs[*pair_index];
                if pair.left_index == index || pair.right_index == index {
                    cover_count += 1;
                    best_score = best_score.max(pair.score);
                }
            }
            if cover_count == 0 {
                continue;
            }
            let candidate = (cover_count, best_score, usize::MAX - index, index);
            if best.is_none_or(|current| candidate > current) {
                best = Some(candidate);
            }
        }

        let Some((_, _, _, chosen)) = best else {
            break;
        };
        indexes.push(chosen);
        let covered_now = remaining
            .iter()
            .copied()
            .filter(|pair_index| {
                let pair = &pairs[*pair_index];
                pair.left_index == chosen || pair.right_index == chosen
            })
            .collect::<Vec<_>>();
        for pair_index in covered_now {
            max_covered_score = max_covered_score.max(pairs[pair_index].score);
            remaining.remove(&pair_index);
        }
    }

    IdentityQuarantinePlan {
        indexes,
        covered_pair_count: pairs.len().saturating_sub(remaining.len()),
        remaining_pair_count: remaining.len(),
        max_covered_score,
    }
}

fn build_admission_plan(size: usize, quarantine: &IdentityQuarantinePlan) -> IdentityAdmissionPlan {
    let quarantine_set: BTreeSet<_> = quarantine
        .indexes
        .iter()
        .copied()
        .filter(|index| *index < size)
        .collect();
    let accept_indexes = (0..size)
        .filter(|index| !quarantine_set.contains(index))
        .collect::<Vec<_>>();
    let quarantine_indexes = quarantine_set.into_iter().collect::<Vec<_>>();
    let action = if quarantine_indexes.is_empty() {
        IdentityAdmissionAction::Accept
    } else if accept_indexes.is_empty() {
        IdentityAdmissionAction::RejectAll
    } else {
        IdentityAdmissionAction::PartialQuarantine
    };

    IdentityAdmissionPlan {
        action,
        total_count: size,
        accept_count: accept_indexes.len(),
        quarantine_count: quarantine_indexes.len(),
        accept_indexes,
        quarantine_indexes,
    }
}

#[derive(Debug, Clone)]
struct PoolRemediationSpec {
    code: &'static str,
    target: IdentityPoolRemediationTarget,
    title: &'static str,
    detail: &'static str,
}

#[derive(Debug, Clone)]
struct PoolRemediationAcc {
    spec: PoolRemediationSpec,
    priority: IdentityFixPriority,
    indexes: BTreeSet<usize>,
    signal_codes: BTreeSet<String>,
    values: BTreeSet<String>,
    pair_count: usize,
}

fn build_pool_remediation_plan(report: &IdentityPoolReport) -> IdentityPoolRemediationPlan {
    let quarantine = report.quarantine_plan();
    let mut actions: BTreeMap<String, PoolRemediationAcc> = BTreeMap::new();

    if !quarantine.indexes.is_empty() {
        let mut signal_codes = BTreeSet::new();
        for offender in report
            .risk_offenders()
            .into_iter()
            .filter(|offender| quarantine.indexes.contains(&offender.index))
        {
            signal_codes.extend(offender.signal_codes);
        }
        let priority = if quarantine.max_covered_score >= 60 {
            IdentityFixPriority::High
        } else {
            IdentityFixPriority::Medium
        };
        let spec = PoolRemediationSpec {
            code: "pool.quarantine_offenders",
            target: IdentityPoolRemediationTarget::Admission,
            title: "先隔离最能拆掉风险边的画像",
            detail: "按贪心覆盖风险 pair 的结果,优先替换或隔离这些下标,再把剩余画像作为下一轮基线。",
        };
        let mut acc = PoolRemediationAcc {
            spec,
            priority,
            indexes: quarantine.indexes.iter().copied().collect(),
            signal_codes,
            values: BTreeSet::new(),
            pair_count: quarantine.covered_pair_count,
        };
        if acc.signal_codes.is_empty() {
            acc.signal_codes.insert("risky_pairs".to_string());
        }
        actions.insert(acc.spec.code.to_string(), acc);
    }

    for signal in &report.duplicate_signals {
        let Some(spec) = pool_action_for_signal(signal.code.as_str()) else {
            continue;
        };
        let priority = pool_signal_priority(signal);
        let acc = actions
            .entry(spec.code.to_string())
            .or_insert_with(|| PoolRemediationAcc {
                spec,
                priority,
                indexes: BTreeSet::new(),
                signal_codes: BTreeSet::new(),
                values: BTreeSet::new(),
                pair_count: 0,
            });
        if fix_priority_rank(priority) > fix_priority_rank(acc.priority) {
            acc.priority = priority;
        }
        acc.indexes.extend(signal.indexes.iter().copied());
        acc.signal_codes.insert(signal.code.clone());
        acc.values.insert(signal.value.clone());
        acc.pair_count += signal.count.saturating_mul(signal.count.saturating_sub(1)) / 2;
    }

    let mut actions = actions
        .into_values()
        .map(|acc| {
            let indexes = acc.indexes.into_iter().collect::<Vec<_>>();
            IdentityPoolRemediationAction {
                code: acc.spec.code.to_string(),
                target: acc.spec.target,
                priority: acc.priority,
                title: acc.spec.title.to_string(),
                detail: acc.spec.detail.to_string(),
                affected_count: indexes.len(),
                indexes,
                pair_count: acc.pair_count,
                signal_codes: acc.signal_codes.into_iter().collect(),
                values: acc.values.into_iter().collect(),
            }
        })
        .collect::<Vec<_>>();
    actions.sort_by(|a, b| {
        fix_priority_rank(b.priority)
            .cmp(&fix_priority_rank(a.priority))
            .then_with(|| b.pair_count.cmp(&a.pair_count))
            .then_with(|| b.affected_count.cmp(&a.affected_count))
            .then_with(|| a.code.cmp(&b.code))
    });

    let targets = actions
        .iter()
        .map(|action| action.target)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let high_priority_count = actions
        .iter()
        .filter(|action| action.priority == IdentityFixPriority::High)
        .count();

    IdentityPoolRemediationPlan {
        action_count: actions.len(),
        high_priority_count,
        quarantine_count: quarantine.indexes.len(),
        quarantine_indexes: quarantine.indexes,
        targets,
        actions,
    }
}

fn pool_action_for_signal(code: &str) -> Option<PoolRemediationSpec> {
    Some(match code {
        "canvas.same" => PoolRemediationSpec {
            code: "pool.disperse_canvas_seed",
            target: IdentityPoolRemediationTarget::Canvas,
            title: "分散 canvas 指纹种子",
            detail: "同一批画像不要共享同一个 canvas hash;按账号或设备画像分配稳定但不同的 canvas/audio 噪声种子。",
        },
        "webgl.same" => PoolRemediationSpec {
            code: "pool.disperse_webgl_renderer",
            target: IdentityPoolRemediationTarget::GpuWebgl,
            title: "分散 WebGL renderer 画像",
            detail: "不同账号不要全部落到同一个 WebGL renderer;按 OS / 设备池分配自洽 GPU 画像。",
        },
        "webdriver.both_true" => PoolRemediationSpec {
            code: "pool.hide_webdriver",
            target: IdentityPoolRemediationTarget::Stealth,
            title: "先消除全池 webdriver 强信号",
            detail: "任何账号暴露 navigator.webdriver=true 都会把自动化来源聚成一团,应在入池前统一修复。",
        },
        "ua.both_headless_chrome" => PoolRemediationSpec {
            code: "pool.remove_headless_ua",
            target: IdentityPoolRemediationTarget::UserAgent,
            title: "移除 HeadlessChrome UA 聚类",
            detail: "无头 UA 会成为跨账号强信号;使用完整非 Headless UA 并同步 Client Hints。",
        },
        "ua.same" => PoolRemediationSpec {
            code: "pool.rotate_user_agent",
            target: IdentityPoolRemediationTarget::UserAgent,
            title: "分散 UA / 浏览器版本",
            detail: "大规模账号池不应共享完全相同 UA;按 OS、浏览器版本和 Client Hints 成组轮换。",
        },
        "platform.same" => PoolRemediationSpec {
            code: "pool.rotate_platform",
            target: IdentityPoolRemediationTarget::UserAgent,
            title: "分散 navigator.platform",
            detail: "platform 应随 OS 画像变化,避免所有上下文共用同一平台字符串。",
        },
        "client_hints.platform_same" => PoolRemediationSpec {
            code: "pool.rotate_client_hints",
            target: IdentityPoolRemediationTarget::ClientHints,
            title: "分散 Client Hints 平台信号",
            detail: "Client Hints 应随 UA 和 OS 画像一起轮换,不要只改 userAgent 字符串。",
        },
        "languages.same" | "timezone.same" => PoolRemediationSpec {
            code: "pool.rotate_locale_proxy",
            target: IdentityPoolRemediationTarget::LocaleProxy,
            title: "让语言、时区和代理出口分层轮换",
            detail: "多地区账号池应把 locale、Accept-Language、timezone 与代理出口绑定成画像,避免全池同区。",
        },
        "screen.same" => PoolRemediationSpec {
            code: "pool.disperse_viewport",
            target: IdentityPoolRemediationTarget::Viewport,
            title: "分散 screen / DPR 组合",
            detail: "为不同画像分配常见但不完全相同的 screen、viewport 和 devicePixelRatio。",
        },
        "hardware_concurrency.same" | "device_memory.same" => PoolRemediationSpec {
            code: "pool.disperse_hardware",
            target: IdentityPoolRemediationTarget::Hardware,
            title: "分散低熵硬件信号",
            detail: "CPU 核数和内存是低熵信号,但和其它重复项叠加会增强关联,应按设备画像分层分散。",
        },
        _ => return None,
    })
}

fn pool_signal_priority(signal: &IdentityPoolSignal) -> IdentityFixPriority {
    match signal.strength {
        LinkabilityStrength::Strong => IdentityFixPriority::High,
        LinkabilityStrength::Medium if signal.count >= 3 => IdentityFixPriority::High,
        LinkabilityStrength::Medium => IdentityFixPriority::Medium,
        LinkabilityStrength::Weak if signal.count >= 4 => IdentityFixPriority::Medium,
        LinkabilityStrength::Weak => IdentityFixPriority::Low,
    }
}

fn pool_stable_hash(reports: &[IdentityReport]) -> String {
    let mut hashes = reports
        .iter()
        .map(|report| report.stable_hash.as_str())
        .collect::<Vec<_>>();
    hashes.sort_unstable();
    let material = hashes.join("\n");
    format!("{:016x}", stable_hash64(&material))
}

fn stable_hash64(material: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in material.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn norm_hash_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn format_float(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let rounded = format!("{value:.4}");
    rounded
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn union_roots(parent: &mut [usize], a: usize, b: usize) {
    let ra = find_root(parent, a);
    let rb = find_root(parent, b);
    if ra != rb {
        parent[rb] = ra;
    }
}

fn find_root(parent: &mut [usize], mut index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    while parent[index] != index {
        let next = parent[index];
        parent[index] = root;
        index = next;
    }
    root
}

#[derive(Debug, Clone)]
struct PoolSignalAcc {
    strength: LinkabilityStrength,
    suggestion: String,
    indexes: Vec<usize>,
}

#[derive(Debug, Clone)]
struct PoolDiversityAcc {
    strength: LinkabilityStrength,
    suggestion: String,
    buckets: BTreeMap<String, Vec<usize>>,
}

fn collect_pool_duplicates(snapshots: &[FingerprintSnapshot]) -> Vec<IdentityPoolSignal> {
    let mut map: BTreeMap<(String, String), PoolSignalAcc> = BTreeMap::new();
    for (idx, fp) in snapshots.iter().enumerate() {
        add_pool_value(
            &mut map,
            idx,
            "ua.same",
            LinkabilityStrength::Medium,
            &fp.ua,
            "为不同账号画像分散 UA / 浏览器版本 / Client Hints。",
        );
        add_pool_value(
            &mut map,
            idx,
            "platform.same",
            LinkabilityStrength::Weak,
            &fp.platform,
            "platform 应随操作系统画像一起变化。",
        );
        add_pool_value(
            &mut map,
            idx,
            "client_hints.platform_same",
            LinkabilityStrength::Weak,
            &fp.ua_data_platform,
            "Client Hints 应随身份画像一起变化。",
        );
        if fp.webdriver {
            add_pool_value(
                &mut map,
                idx,
                "webdriver.both_true",
                LinkabilityStrength::Strong,
                "true",
                "先消除 navigator.webdriver=true,否则会形成稳定自动化聚类。",
            );
        }
        add_pool_value(
            &mut map,
            idx,
            "languages.same",
            LinkabilityStrength::Weak,
            &fp.languages,
            "语言、时区、代理地区应成组轮换。",
        );
        add_pool_value(
            &mut map,
            idx,
            "timezone.same",
            LinkabilityStrength::Medium,
            &fp.timezone,
            "多地区账号池应让代理出口与 timezone 一起轮换。",
        );
        if !fp.screen.trim().is_empty() && fp.device_pixel_ratio > 0.0 {
            let value = format!("{}@{}", fp.screen.trim(), fp.device_pixel_ratio);
            add_pool_value(
                &mut map,
                idx,
                "screen.same",
                LinkabilityStrength::Medium,
                &value,
                "为不同画像分配常见但不完全一致的 screen / DPR 组合。",
            );
        }
        if fp.hardware_concurrency > 0 {
            add_pool_value(
                &mut map,
                idx,
                "hardware_concurrency.same",
                LinkabilityStrength::Weak,
                &fp.hardware_concurrency.to_string(),
                "CPU 核数应落在合理范围内,并随画像池分散。",
            );
        }
        if fp.device_memory > 0.0 {
            add_pool_value(
                &mut map,
                idx,
                "device_memory.same",
                LinkabilityStrength::Weak,
                &fp.device_memory.to_string(),
                "内存容量是低熵信号,和其它弱信号叠加后仍会增强关联。",
            );
        }
        add_pool_stable_value(
            &mut map,
            idx,
            "webgl.same",
            LinkabilityStrength::Strong,
            &fp.webgl_renderer,
            "不同机器画像应分散 WebGL vendor/renderer,并保持与 OS/UA 自洽。",
        );
        add_pool_stable_value(
            &mut map,
            idx,
            "canvas.same",
            LinkabilityStrength::Strong,
            &fp.canvas_hash,
            "不同画像应使用稳定但各异的 canvas/audio 噪声种子。",
        );
        if fp.ua.contains("HeadlessChrome") {
            add_pool_value(
                &mut map,
                idx,
                "ua.both_headless_chrome",
                LinkabilityStrength::Strong,
                "HeadlessChrome",
                "先修正无头 UA 与 Client Hints,再谈多账号隔离。",
            );
        }
    }

    let mut out: Vec<_> = map
        .into_iter()
        .filter_map(|((code, value), acc)| {
            let mut indexes = acc.indexes;
            indexes.sort_unstable();
            indexes.dedup();
            (indexes.len() > 1).then_some(IdentityPoolSignal {
                strength: acc.strength,
                code,
                value,
                count: indexes.len(),
                indexes,
                suggestion: acc.suggestion,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        strength_rank(b.strength)
            .cmp(&strength_rank(a.strength))
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| a.value.cmp(&b.value))
    });
    out
}

fn build_pool_diversity_report(snapshots: &[FingerprintSnapshot]) -> IdentityPoolDiversityReport {
    let size = snapshots.len();
    if size == 0 {
        return IdentityPoolDiversityReport::default();
    }

    let mut map: BTreeMap<String, PoolDiversityAcc> = BTreeMap::new();
    for (idx, fp) in snapshots.iter().enumerate() {
        add_diversity_value(
            &mut map,
            idx,
            "ua.same",
            LinkabilityStrength::Medium,
            &fp.ua,
            "为不同账号画像分散 UA / 浏览器版本 / Client Hints。",
        );
        add_diversity_value(
            &mut map,
            idx,
            "platform.same",
            LinkabilityStrength::Weak,
            &fp.platform,
            "platform 应随操作系统画像一起变化。",
        );
        add_diversity_value(
            &mut map,
            idx,
            "client_hints.platform_same",
            LinkabilityStrength::Weak,
            &fp.ua_data_platform,
            "Client Hints 应随身份画像一起变化。",
        );
        if fp.webdriver {
            add_diversity_value(
                &mut map,
                idx,
                "webdriver.both_true",
                LinkabilityStrength::Strong,
                "true",
                "先消除 navigator.webdriver=true,否则会形成稳定自动化聚类。",
            );
        }
        add_diversity_value(
            &mut map,
            idx,
            "languages.same",
            LinkabilityStrength::Weak,
            &fp.languages,
            "语言、时区、代理地区应成组轮换。",
        );
        add_diversity_value(
            &mut map,
            idx,
            "timezone.same",
            LinkabilityStrength::Medium,
            &fp.timezone,
            "多地区账号池应让代理出口与 timezone 一起轮换。",
        );
        if !fp.screen.trim().is_empty() && fp.device_pixel_ratio > 0.0 {
            let value = format!("{}@{}", fp.screen.trim(), fp.device_pixel_ratio);
            add_diversity_value(
                &mut map,
                idx,
                "screen.same",
                LinkabilityStrength::Medium,
                &value,
                "为不同画像分配常见但不完全一致的 screen / DPR 组合。",
            );
        }
        if fp.hardware_concurrency > 0 {
            add_diversity_value(
                &mut map,
                idx,
                "hardware_concurrency.same",
                LinkabilityStrength::Weak,
                &fp.hardware_concurrency.to_string(),
                "CPU 核数应落在合理范围内,并随画像池分散。",
            );
        }
        if fp.device_memory > 0.0 {
            add_diversity_value(
                &mut map,
                idx,
                "device_memory.same",
                LinkabilityStrength::Weak,
                &fp.device_memory.to_string(),
                "内存容量是低熵信号,和其它弱信号叠加后仍会增强关联。",
            );
        }
        add_diversity_stable_value(
            &mut map,
            idx,
            "webgl.same",
            LinkabilityStrength::Strong,
            &fp.webgl_renderer,
            "不同机器画像应分散 WebGL vendor/renderer,并保持与 OS/UA 自洽。",
        );
        add_diversity_stable_value(
            &mut map,
            idx,
            "canvas.same",
            LinkabilityStrength::Strong,
            &fp.canvas_hash,
            "不同画像应使用稳定但各异的 canvas/audio 噪声种子。",
        );
        if fp.ua.contains("HeadlessChrome") {
            add_diversity_value(
                &mut map,
                idx,
                "ua.both_headless_chrome",
                LinkabilityStrength::Strong,
                "HeadlessChrome",
                "先修正无头 UA 与 Client Hints,再谈多账号隔离。",
            );
        }
    }

    let mut total_unique_ratio = 0.0f64;
    let mut max_concentration_ratio = 0.0f64;
    let mut signals = map
        .into_iter()
        .filter_map(|(code, acc)| {
            if acc.buckets.is_empty() {
                return None;
            }
            let mut buckets = acc
                .buckets
                .into_iter()
                .map(|(value, mut indexes)| {
                    indexes.sort_unstable();
                    indexes.dedup();
                    let count = indexes.len();
                    IdentityPoolDiversityBucket {
                        value,
                        count,
                        ratio: ratio(count, size),
                        indexes,
                    }
                })
                .collect::<Vec<_>>();
            buckets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            let unique_count = buckets.len();
            let repeated_value_count = buckets.iter().filter(|bucket| bucket.count > 1).count();
            let max_bucket_count = buckets.first().map_or(0, |bucket| bucket.count);
            let max_bucket_ratio = ratio(max_bucket_count, size);
            total_unique_ratio += ratio(unique_count, size);
            max_concentration_ratio = max_concentration_ratio.max(max_bucket_ratio);
            Some(IdentityPoolDiversitySignal {
                strength: acc.strength,
                code,
                unique_count,
                repeated_value_count,
                max_bucket_count,
                max_bucket_ratio,
                buckets,
                suggestion: acc.suggestion,
            })
        })
        .collect::<Vec<_>>();

    signals.sort_by(|a, b| {
        strength_rank(b.strength)
            .cmp(&strength_rank(a.strength))
            .then_with(|| b.max_bucket_count.cmp(&a.max_bucket_count))
            .then_with(|| b.repeated_value_count.cmp(&a.repeated_value_count))
            .then_with(|| a.code.cmp(&b.code))
    });
    let signal_count = signals.len();
    let concentrated_signal_count = signals
        .iter()
        .filter(|signal| signal.repeated_value_count > 0)
        .count();

    IdentityPoolDiversityReport {
        size,
        signal_count,
        concentrated_signal_count,
        max_concentration_ratio,
        average_unique_ratio: if signal_count == 0 {
            0.0
        } else {
            total_unique_ratio / signal_count as f64
        },
        signals,
    }
}

fn build_identity_entropy_budget(diversity: &IdentityPoolDiversityReport) -> IdentityEntropyBudget {
    if diversity.size == 0 || diversity.signal_count == 0 {
        return IdentityEntropyBudget::default();
    }

    let mut weighted_entropy_total = 0.0f64;
    let mut weight_total = 0.0f64;
    let mut signals = Vec::new();
    for signal in &diversity.signals {
        let entropy = shannon_entropy(&signal.buckets);
        let max_entropy = if diversity.size <= 1 {
            0.0
        } else {
            (diversity.size as f64).log2()
        };
        let normalized_entropy = if max_entropy <= 0.0 {
            1.0
        } else {
            (entropy / max_entropy).clamp(0.0, 1.0)
        };
        let effective_value_count = 2.0f64.powf(entropy).clamp(1.0, diversity.size as f64);
        let weight = entropy_signal_weight(signal.strength);
        weighted_entropy_total += normalized_entropy * weight;
        weight_total += weight;
        signals.push(IdentityEntropySignalBudget {
            strength: signal.strength,
            code: signal.code.clone(),
            normalized_entropy,
            effective_value_count,
            unique_count: signal.unique_count,
            max_bucket_ratio: signal.max_bucket_ratio,
            repeated_value_count: signal.repeated_value_count,
            suggestion: signal.suggestion.clone(),
        });
    }

    signals.sort_by(|a, b| {
        a.normalized_entropy
            .partial_cmp(&b.normalized_entropy)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| strength_rank(b.strength).cmp(&strength_rank(a.strength)))
            .then_with(|| {
                b.max_bucket_ratio
                    .partial_cmp(&a.max_bucket_ratio)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.code.cmp(&b.code))
    });

    let weighted_entropy = if weight_total <= 0.0 {
        0.0
    } else {
        (weighted_entropy_total / weight_total).clamp(0.0, 1.0)
    };
    let effective_identity_count = if diversity.size <= 1 {
        diversity.size as f64
    } else {
        1.0 + (diversity.size.saturating_sub(1) as f64 * weighted_entropy)
    };
    let bottleneck_signals = signals
        .into_iter()
        .filter(|signal| {
            signal.normalized_entropy < 0.75
                || signal.max_bucket_ratio >= 0.70
                || (signal.repeated_value_count > 0
                    && signal.strength == LinkabilityStrength::Strong)
        })
        .take(8)
        .collect::<Vec<_>>();
    let bottleneck_count = bottleneck_signals.len();
    let entropy_score = (weighted_entropy * 100.0).round().clamp(0.0, 100.0) as u8;
    let nominal_to_effective_ratio = if effective_identity_count <= 0.0 {
        0.0
    } else {
        diversity.size as f64 / effective_identity_count
    };

    IdentityEntropyBudget {
        size: diversity.size,
        signal_count: diversity.signal_count,
        status: entropy_status(
            diversity.size,
            weighted_entropy,
            diversity.max_concentration_ratio,
            bottleneck_count,
        ),
        weighted_entropy,
        entropy_score,
        effective_identity_count,
        nominal_to_effective_ratio,
        bottleneck_count,
        bottleneck_signals,
    }
}

fn build_identity_capacity_plan(entropy: &IdentityEntropyBudget) -> IdentityCapacityPlan {
    if entropy.size == 0 || entropy.signal_count == 0 {
        return IdentityCapacityPlan::default();
    }

    let target_effective_identity_count = entropy.size as f64;
    let missing_effective_identity_count =
        (target_effective_identity_count - entropy.effective_identity_count).max(0.0);
    let additional_distinct_profiles_needed = missing_effective_identity_count.ceil() as usize;
    let target_nominal_to_effective_ratio = 1.0;
    let status = capacity_status(
        entropy.size,
        entropy.effective_identity_count,
        missing_effective_identity_count,
        entropy.status,
    );
    let bottleneck_signals = entropy.bottleneck_signals.clone();
    let actions = build_capacity_actions(&bottleneck_signals, missing_effective_identity_count);

    IdentityCapacityPlan {
        size: entropy.size,
        status,
        effective_identity_count: entropy.effective_identity_count,
        target_effective_identity_count,
        missing_effective_identity_count,
        nominal_to_effective_ratio: entropy.nominal_to_effective_ratio,
        target_nominal_to_effective_ratio,
        additional_distinct_profiles_needed,
        bottleneck_count: bottleneck_signals.len(),
        bottleneck_signals,
        actions,
    }
}

fn capacity_status(
    size: usize,
    effective_identity_count: f64,
    missing_effective_identity_count: f64,
    entropy_status: IdentityEntropyStatus,
) -> IdentityCapacityStatus {
    if size < 2 {
        IdentityCapacityStatus::Unknown
    } else if missing_effective_identity_count <= 0.5
        && matches!(entropy_status, IdentityEntropyStatus::Diverse)
    {
        IdentityCapacityStatus::Ready
    } else if effective_identity_count <= (size as f64 * 0.5) {
        IdentityCapacityStatus::Exhausted
    } else {
        IdentityCapacityStatus::NeedsDiversification
    }
}

fn build_capacity_actions(
    bottlenecks: &[IdentityEntropySignalBudget],
    missing_effective_identity_count: f64,
) -> Vec<IdentityCapacityAction> {
    let mut grouped: BTreeMap<&'static str, Vec<&IdentityEntropySignalBudget>> = BTreeMap::new();
    for signal in bottlenecks {
        grouped
            .entry(capacity_action_code(&signal.code))
            .or_default()
            .push(signal);
    }

    let mut actions = grouped
        .into_iter()
        .map(|(code, signals)| {
            let signal_codes = signals
                .iter()
                .map(|signal| signal.code.clone())
                .collect::<Vec<_>>();
            let strongest = signals
                .iter()
                .map(|signal| signal.strength)
                .max_by_key(|strength| strength_rank(*strength))
                .unwrap_or(LinkabilityStrength::Weak);
            let min_entropy = signals
                .iter()
                .map(|signal| signal.normalized_entropy)
                .fold(1.0f64, f64::min);
            let estimated_gain = (missing_effective_identity_count * (1.0 - min_entropy))
                .max(0.0)
                .min(missing_effective_identity_count);
            IdentityCapacityAction {
                code: code.to_string(),
                priority: capacity_action_priority(strongest, min_entropy),
                title: capacity_action_title(code).to_string(),
                detail: capacity_action_detail(code).to_string(),
                signal_codes,
                estimated_gain,
            }
        })
        .collect::<Vec<_>>();

    actions.sort_by(|a, b| {
        fix_priority_rank(b.priority)
            .cmp(&fix_priority_rank(a.priority))
            .then_with(|| {
                b.estimated_gain
                    .partial_cmp(&a.estimated_gain)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.code.cmp(&b.code))
    });
    actions
}

fn capacity_action_code(signal_code: &str) -> &'static str {
    if signal_code.contains("canvas") {
        "capacity.disperse_canvas_seed"
    } else if signal_code.contains("webgl") {
        "capacity.disperse_webgl_renderer"
    } else if signal_code.contains("timezone")
        || signal_code.contains("languages")
        || signal_code.contains("locale")
    {
        "capacity.rotate_locale_proxy"
    } else if signal_code.contains("ua") || signal_code.contains("client_hints") {
        "capacity.rotate_browser_persona"
    } else if signal_code.contains("screen")
        || signal_code.contains("hardware")
        || signal_code.contains("device_memory")
    {
        "capacity.disperse_device_shape"
    } else {
        "capacity.add_distinct_profiles"
    }
}

fn capacity_action_priority(
    strength: LinkabilityStrength,
    normalized_entropy: f64,
) -> IdentityFixPriority {
    if strength == LinkabilityStrength::Strong || normalized_entropy < 0.35 {
        IdentityFixPriority::High
    } else if strength == LinkabilityStrength::Medium || normalized_entropy < 0.65 {
        IdentityFixPriority::Medium
    } else {
        IdentityFixPriority::Low
    }
}

fn capacity_action_title(code: &str) -> &'static str {
    match code {
        "capacity.disperse_canvas_seed" => "分散 canvas/audio 噪声种子",
        "capacity.disperse_webgl_renderer" => "分散 WebGL vendor/renderer 画像",
        "capacity.rotate_locale_proxy" => "按代理地区重绑 locale/timezone",
        "capacity.rotate_browser_persona" => "轮换浏览器 UA / Client Hints persona",
        "capacity.disperse_device_shape" => "分散屏幕与硬件低熵形状",
        _ => "补充新的独立画像",
    }
}

fn capacity_action_detail(code: &str) -> &'static str {
    match code {
        "capacity.disperse_canvas_seed" => {
            "当前 canvas/audio 稳定值过度集中,应为新增或修复 profile 分配不同但稳定的噪声种子。"
        }
        "capacity.disperse_webgl_renderer" => {
            "当前 WebGL renderer 形成容量瓶颈,应按 OS/UA persona 分散 GPU vendor/renderer。"
        }
        "capacity.rotate_locale_proxy" => {
            "当前语言、时区或代理地域集中,应把 locale/timezone/出口 IP 作为一组重新分配。"
        }
        "capacity.rotate_browser_persona" => {
            "当前 UA 或 Client Hints 模板集中,应轮换浏览器版本、平台与高熵 Client Hints。"
        }
        "capacity.disperse_device_shape" => {
            "当前 screen/DPR/CPU/内存等低熵设备形状集中,应按常见真实设备簇分散。"
        }
        _ => "当前有效画像容量不足,应补充新的独立 profile persona 后再放量。",
    }
}

fn shannon_entropy(buckets: &[IdentityPoolDiversityBucket]) -> f64 {
    buckets.iter().fold(0.0, |entropy, bucket| {
        if bucket.ratio <= 0.0 {
            entropy
        } else {
            entropy - bucket.ratio * bucket.ratio.log2()
        }
    })
}

fn entropy_signal_weight(strength: LinkabilityStrength) -> f64 {
    match strength {
        LinkabilityStrength::Weak => 1.0,
        LinkabilityStrength::Medium => 2.0,
        LinkabilityStrength::Strong => 3.0,
    }
}

fn entropy_status(
    size: usize,
    weighted_entropy: f64,
    max_concentration_ratio: f64,
    bottleneck_count: usize,
) -> IdentityEntropyStatus {
    if size < 2 {
        IdentityEntropyStatus::Unknown
    } else if weighted_entropy < 0.25 || max_concentration_ratio >= 0.95 {
        IdentityEntropyStatus::Collapsed
    } else if weighted_entropy < 0.55 || max_concentration_ratio >= 0.80 {
        IdentityEntropyStatus::Concentrated
    } else if weighted_entropy < 0.80 || bottleneck_count > 0 {
        IdentityEntropyStatus::Thin
    } else {
        IdentityEntropyStatus::Diverse
    }
}

fn add_pool_value(
    map: &mut BTreeMap<(String, String), PoolSignalAcc>,
    idx: usize,
    code: &str,
    strength: LinkabilityStrength,
    value: &str,
    suggestion: &str,
) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let key = (code.to_string(), value.to_ascii_lowercase());
    let acc = map.entry(key).or_insert_with(|| PoolSignalAcc {
        strength,
        suggestion: suggestion.to_string(),
        indexes: Vec::new(),
    });
    acc.indexes.push(idx);
}

fn add_pool_stable_value(
    map: &mut BTreeMap<(String, String), PoolSignalAcc>,
    idx: usize,
    code: &str,
    strength: LinkabilityStrength,
    value: &str,
    suggestion: &str,
) {
    if stable_value(value) {
        add_pool_value(map, idx, code, strength, value, suggestion);
    }
}

fn add_diversity_value(
    map: &mut BTreeMap<String, PoolDiversityAcc>,
    idx: usize,
    code: &str,
    strength: LinkabilityStrength,
    value: &str,
    suggestion: &str,
) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let acc = map
        .entry(code.to_string())
        .or_insert_with(|| PoolDiversityAcc {
            strength,
            suggestion: suggestion.to_string(),
            buckets: BTreeMap::new(),
        });
    acc.buckets
        .entry(value.to_ascii_lowercase())
        .or_default()
        .push(idx);
}

fn add_diversity_stable_value(
    map: &mut BTreeMap<String, PoolDiversityAcc>,
    idx: usize,
    code: &str,
    strength: LinkabilityStrength,
    value: &str,
    suggestion: &str,
) {
    if stable_value(value) {
        add_diversity_value(map, idx, code, strength, value, suggestion);
    }
}

fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 { 0.0 } else { n as f64 / d as f64 }
}

fn strength_rank(strength: LinkabilityStrength) -> u8 {
    match strength {
        LinkabilityStrength::Weak => 0,
        LinkabilityStrength::Medium => 1,
        LinkabilityStrength::Strong => 2,
    }
}

fn push_link_signal(
    signals: &mut Vec<LinkabilitySignal>,
    strength: LinkabilityStrength,
    code: impl Into<String>,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) {
    signals.push(LinkabilitySignal {
        strength,
        code: code.into(),
        message: message.into(),
        suggestion: suggestion.into(),
    });
}

fn push_drift_signal(
    signals: &mut Vec<IdentityDriftSignal>,
    severity: IdentityDriftSeverity,
    code: impl Into<String>,
    before: &str,
    after: &str,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) {
    signals.push(IdentityDriftSignal {
        severity,
        code: code.into(),
        before: before.to_string(),
        after: after.to_string(),
        message: message.into(),
        suggestion: suggestion.into(),
    });
}

fn push_issue(
    issues: &mut Vec<IdentityIssue>,
    severity: IdentitySeverity,
    code: impl Into<String>,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) {
    issues.push(IdentityIssue {
        severity,
        code: code.into(),
        message: message.into(),
        suggestion: suggestion.into(),
    });
}

#[derive(Debug, Clone, Copy)]
struct FixActionSpec {
    code: &'static str,
    target: IdentityFixTarget,
    title: &'static str,
    detail: &'static str,
    fields: &'static [&'static str],
}

fn build_identity_fix_plan(issues: &[IdentityIssue]) -> IdentityFixPlan {
    let mut actions: BTreeMap<String, IdentityFixAction> = BTreeMap::new();
    for issue in issues {
        let Some(spec) = fix_action_for_issue(issue.code.as_str()) else {
            continue;
        };
        let priority = fix_priority(issue.severity);
        let action = actions
            .entry(spec.code.to_string())
            .or_insert_with(|| IdentityFixAction {
                code: spec.code.to_string(),
                target: spec.target,
                priority,
                title: spec.title.to_string(),
                detail: spec.detail.to_string(),
                fields: spec
                    .fields
                    .iter()
                    .map(|field| (*field).to_string())
                    .collect(),
                issue_codes: Vec::new(),
            });
        if fix_priority_rank(priority) > fix_priority_rank(action.priority) {
            action.priority = priority;
        }
        if !action.issue_codes.iter().any(|code| code == &issue.code) {
            action.issue_codes.push(issue.code.clone());
            action.issue_codes.sort();
        }
    }

    let mut actions = actions.into_values().collect::<Vec<_>>();
    actions.sort_by(|a, b| {
        fix_priority_rank(b.priority)
            .cmp(&fix_priority_rank(a.priority))
            .then_with(|| a.code.cmp(&b.code))
    });
    let targets = actions
        .iter()
        .map(|action| action.target)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let high_priority_count = actions
        .iter()
        .filter(|action| action.priority == IdentityFixPriority::High)
        .count();

    IdentityFixPlan {
        action_count: actions.len(),
        high_priority_count,
        targets,
        actions,
    }
}

fn fix_action_for_issue(code: &str) -> Option<FixActionSpec> {
    Some(match code {
        "ua.empty" | "ua.headless_chrome" => FixActionSpec {
            code: "ua.normalize",
            target: IdentityFixTarget::UserAgent,
            title: "使用完整且非 Headless 的浏览器 UA",
            detail: "重新生成 UA,并确保浏览器版本、操作系统、Client Hints 与真实启动环境一致。",
            fields: &["ua"],
        },
        "navigator.webdriver" => FixActionSpec {
            code: "stealth.webdriver_false",
            target: IdentityFixTarget::Stealth,
            title: "隐藏自动化 webdriver 强信号",
            detail: "在页面脚本执行前注入 stealth 初始化,确保 navigator.webdriver 不暴露 true。",
            fields: &["webdriver"],
        },
        "platform.os_mismatch" | "webgl.os_mismatch" => FixActionSpec {
            code: "profile.align_os",
            target: IdentityFixTarget::ProfileOs,
            title: "统一 UA / platform / WebGL 操作系统画像",
            detail: "从同一个身份画像生成 UA、navigator.platform、Client Hints、WebGL vendor/renderer 与触摸能力。",
            fields: &[
                "ua",
                "platform",
                "uaDataPlatform",
                "webglRenderer",
                "maxTouchPoints",
            ],
        },
        "client_hints.missing"
        | "client_hints.platform_mismatch"
        | "client_hints.mobile_mismatch" => FixActionSpec {
            code: "client_hints.sync",
            target: IdentityFixTarget::ClientHints,
            title: "同步 Chromium Client Hints",
            detail: "用 Emulation.setUserAgentOverride 同步 userAgentMetadata,不要只替换 UA 字符串。",
            fields: &["uaDataPlatform", "uaDataMobile"],
        },
        "touch.missing_mobile" | "touch.desktop_many_points" => FixActionSpec {
            code: "touch.align_device",
            target: IdentityFixTarget::Touch,
            title: "让触摸能力与移动 / 桌面画像一致",
            detail: "移动画像启用触摸和合理 viewport;桌面画像避免异常高 maxTouchPoints。",
            fields: &["ua", "uaDataMobile", "maxTouchPoints", "screen"],
        },
        "languages.empty" | "timezone.empty" | "locale.timezone_mismatch" => FixActionSpec {
            code: "locale.align_proxy",
            target: IdentityFixTarget::LocaleProxy,
            title: "让语言、时区和代理出口同源",
            detail: "按代理出口地区生成 locale、Accept-Language、navigator.languages 与 IANA timezone。",
            fields: &["languages", "timezone"],
        },
        "hardware_concurrency.zero" | "hardware_concurrency.large" => FixActionSpec {
            code: "hardware.normalize",
            target: IdentityFixTarget::Hardware,
            title: "设置合理硬件并发画像",
            detail: "为画像分配常见 CPU 核数与内存容量,避免 0 或明显偏离消费级设备。",
            fields: &["hardwareConcurrency", "deviceMemory"],
        },
        "screen.dpr_invalid" | "screen.invalid" => FixActionSpec {
            code: "viewport.normalize",
            target: IdentityFixTarget::Viewport,
            title: "设置常见 viewport / screen / DPR 组合",
            detail: "使用与设备类型匹配的分辨率和 devicePixelRatio,避免 0x0 或异常尺寸。",
            fields: &["screen", "devicePixelRatio"],
        },
        "webgl.missing" | "webgl.software_renderer" => FixActionSpec {
            code: "gpu.enable_webgl",
            target: IdentityFixTarget::GpuWebgl,
            title: "恢复可用且自洽的 WebGL renderer",
            detail: "优先使用真实 GPU;无 GPU 时选择与 OS 自洽的 renderer 补环境,避免 SwiftShader/llvmpipe 暴露。",
            fields: &["webglRenderer"],
        },
        "canvas.missing" | "canvas.hash_shape" => FixActionSpec {
            code: "canvas.restore_hash",
            target: IdentityFixTarget::Canvas,
            title: "恢复稳定但分散的 canvas 指纹",
            detail: "不要让 canvas API 返回空值或异常;按身份画像分配稳定且不同的噪声种子。",
            fields: &["canvasHash"],
        },
        _ => return None,
    })
}

fn fix_priority(severity: IdentitySeverity) -> IdentityFixPriority {
    match severity {
        IdentitySeverity::High => IdentityFixPriority::High,
        IdentitySeverity::Medium => IdentityFixPriority::Medium,
        IdentitySeverity::Info | IdentitySeverity::Low => IdentityFixPriority::Low,
    }
}

fn fix_priority_rank(priority: IdentityFixPriority) -> u8 {
    match priority {
        IdentityFixPriority::Low => 0,
        IdentityFixPriority::Medium => 1,
        IdentityFixPriority::High => 2,
    }
}

fn drift_severity_rank(severity: IdentityDriftSeverity) -> u8 {
    match severity {
        IdentityDriftSeverity::None => 0,
        IdentityDriftSeverity::Low => 1,
        IdentityDriftSeverity::Medium => 2,
        IdentityDriftSeverity::High => 3,
    }
}

fn same_norm(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    !a.is_empty() && !b.is_empty() && a.eq_ignore_ascii_case(b)
}

fn changed_norm(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    (!a.is_empty() || !b.is_empty()) && !a.eq_ignore_ascii_case(b)
}

fn same_stable(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    stable_value(a) && stable_value(b) && a.eq_ignore_ascii_case(b)
}

fn changed_stable(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    stable_value(a) && stable_value(b) && !a.eq_ignore_ascii_case(b)
}

fn stable_value(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty() && !v.eq_ignore_ascii_case("err") && !v.eq_ignore_ascii_case("none")
}

fn known_os_changed(before: SignalOs, after: SignalOs) -> bool {
    before != SignalOs::Unknown && after != SignalOs::Unknown && before != after
}

fn browser_major(ua: &str) -> Option<u32> {
    for marker in ["Chrome/", "Chromium/", "Firefox/", "Edg/", "Version/"] {
        if let Some(rest) = ua.split(marker).nth(1) {
            let major = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if let Ok(major) = major.parse() {
                return Some(major);
            }
        }
    }
    None
}

fn screen_dpr_value(fp: &FingerprintSnapshot) -> String {
    format!(
        "{}@{}",
        fp.screen.trim().to_ascii_lowercase(),
        format_float(fp.device_pixel_ratio)
    )
}

fn os_from_ua(ua: &str) -> SignalOs {
    let l = ua.to_ascii_lowercase();
    if l.contains("android") {
        SignalOs::Android
    } else if l.contains("iphone") || l.contains("ipad") || l.contains("ipod") {
        SignalOs::Ios
    } else if l.contains("windows") || l.contains("win64") || l.contains("win32") {
        SignalOs::Windows
    } else if l.contains("macintosh") || l.contains("mac os x") {
        SignalOs::Mac
    } else if l.contains("linux") || l.contains("x11") {
        SignalOs::Linux
    } else {
        SignalOs::Unknown
    }
}

fn os_from_platform(platform: &str) -> SignalOs {
    let l = platform.to_ascii_lowercase();
    if l.contains("android") {
        SignalOs::Android
    } else if l.contains("iphone") || l.contains("ipad") || l.contains("ipod") {
        SignalOs::Ios
    } else if l.contains("win") {
        SignalOs::Windows
    } else if l.contains("mac") {
        SignalOs::Mac
    } else if l.contains("linux") || l.contains("x11") {
        SignalOs::Linux
    } else {
        SignalOs::Unknown
    }
}

fn os_from_ua_data_platform(platform: &str) -> SignalOs {
    let l = platform.to_ascii_lowercase();
    if l.contains("android") {
        SignalOs::Android
    } else if l.contains("ios") {
        SignalOs::Ios
    } else if l.contains("windows") {
        SignalOs::Windows
    } else if l.contains("mac") {
        SignalOs::Mac
    } else if l.contains("linux") || l.contains("chrome os") {
        SignalOs::Linux
    } else {
        SignalOs::Unknown
    }
}

fn os_conflict(ua_os: SignalOs, other: SignalOs, max_touch_points: u32) -> bool {
    use SignalOs::*;
    match (ua_os, other) {
        (Unknown, _) | (_, Unknown) => false,
        (Android, Linux) => false,
        (Ios, Mac) if max_touch_points > 1 => false,
        (a, b) => a != b,
    }
}

fn is_mobile_ua(ua: &str) -> bool {
    let l = ua.to_ascii_lowercase();
    l.contains("mobile") || l.contains("android") || l.contains("iphone") || l.contains("ipad")
}

fn looks_chromium(ua: &str) -> bool {
    (ua.contains("Chrome/") || ua.contains("Chromium/") || ua.contains("Edg/"))
        && !ua.contains("Firefox/")
}

fn language_timezone_conflict(languages: &str, timezone: &str) -> Option<IdentitySeverity> {
    let lang = languages
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let tz = timezone.to_ascii_lowercase();
    let cjk_off_asia = (lang.starts_with("zh") || lang.starts_with("ja") || lang.starts_with("ko"))
        && !tz.starts_with("asia/");
    if cjk_off_asia {
        Some(IdentitySeverity::Medium)
    } else if lang.starts_with("en-us") && !(tz.starts_with("america/") || tz.starts_with("us/")) {
        Some(IdentitySeverity::Low)
    } else {
        None
    }
}

fn valid_screen(screen: &str) -> bool {
    let Some((w, h)) = screen.split_once('x') else {
        return false;
    };
    let Ok(w) = w.parse::<u32>() else {
        return false;
    };
    let Ok(h) = h.parse::<u32>() else {
        return false;
    };
    (200..=10000).contains(&w) && (200..=10000).contains(&h)
}

fn webgl_os_conflict(ua_os: SignalOs, webgl: &str) -> bool {
    match ua_os {
        SignalOs::Windows => webgl.contains("apple") || webgl.contains("metal"),
        SignalOs::Mac | SignalOs::Ios => {
            webgl.contains("direct3d") || webgl.contains("d3d11") || webgl.contains("d3d12")
        }
        SignalOs::Linux | SignalOs::Android => {
            webgl.contains("direct3d") || webgl.contains("d3d11") || webgl.contains("metal")
        }
        SignalOs::Unknown => false,
    }
}

#[cfg(feature = "camoufox")]
#[async_trait::async_trait]
impl FingerprintProbe for crate::browser::Tab {
    async fn fp_eval(&self, js: &str) -> Result<Value> {
        self.run_js(js).await
    }
}

#[cfg(feature = "cdp")]
#[async_trait::async_trait]
impl FingerprintProbe for crate::cdp::ChromiumTab {
    async fn fp_eval(&self, js: &str) -> Result<Value> {
        self.run_js(js).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_json_string_from_probe() {
        // 探针实际返回的是 JSON.stringify 的字符串值。
        let raw = json!(
            r#"{"ua":"Mozilla/5.0 X","platform":"Win32","uaDataPlatform":"Windows","uaDataMobile":false,"webdriver":false,"languages":"en-US,en","maxTouchPoints":0,"hardwareConcurrency":12,"deviceMemory":8,"screen":"2560x1440","devicePixelRatio":1.5,"timezone":"America/New_York","webglRenderer":"ANGLE (NVIDIA)","canvasHash":"a1b2c3d4"}"#
        );
        let fp = FingerprintSnapshot::from_probe(&raw);
        assert_eq!(fp.ua, "Mozilla/5.0 X");
        assert_eq!(fp.platform, "Win32");
        assert_eq!(fp.ua_data_platform, "Windows");
        assert!(!fp.ua_data_mobile);
        assert!(!fp.webdriver);
        assert_eq!(fp.languages, "en-US,en");
        assert_eq!(fp.max_touch_points, 0);
        assert_eq!(fp.hardware_concurrency, 12);
        assert_eq!(fp.device_memory, 8.0);
        assert_eq!(fp.screen, "2560x1440");
        assert_eq!(fp.device_pixel_ratio, 1.5);
        assert_eq!(fp.timezone, "America/New_York");
        assert_eq!(fp.webgl_renderer, "ANGLE (NVIDIA)");
        assert_eq!(fp.canvas_hash, "a1b2c3d4");
    }

    #[test]
    fn parses_object_from_probe_and_defaults_missing() {
        // 兼容后端直接返回对象;缺字段走默认值,不 panic。
        let obj = json!({ "ua": "UA", "hardwareConcurrency": 4 });
        let fp = FingerprintSnapshot::from_probe(&obj);
        assert_eq!(fp.ua, "UA");
        assert_eq!(fp.hardware_concurrency, 4);
        assert_eq!(fp.device_memory, 0.0);
        assert_eq!(fp.canvas_hash, "");
        assert_eq!(fp.webgl_renderer, "");
    }

    #[test]
    fn identity_report_scores_healthy_desktop() {
        let fp = win_profile();
        let report = fp.diagnose();
        assert!(report.is_healthy(), "{report:#?}");
        assert_eq!(report.score, 100);
        assert_eq!(report.identity_id, fp.identity_id());
        assert_eq!(report.stable_hash, fp.stable_hash());
        assert!(report.identity_id.starts_with("fp_"));
        assert!(report.issues.is_empty());
        assert!(report.fix_plan.is_empty());
    }

    #[test]
    fn fingerprint_identity_id_is_stable_and_normalized() {
        let a = win_profile();
        let mut b = win_profile();
        b.ua = format!("  {}  ", b.ua.to_ascii_uppercase());
        b.timezone = b.timezone.to_ascii_uppercase();

        assert_eq!(a.stable_hash(), b.stable_hash());
        assert_eq!(a.identity_id(), b.identity_id());
    }

    #[test]
    fn identity_report_flags_obvious_bot_signals() {
        let fp = FingerprintSnapshot {
            ua: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/149.0.0.0 Safari/537.36".into(),
            platform: "MacIntel".into(),
            ua_data_platform: "macOS".into(),
            ua_data_mobile: true,
            webdriver: true,
            languages: "zh-CN,zh".into(),
            max_touch_points: 0,
            hardware_concurrency: 0,
            device_memory: 0.0,
            screen: "0x0".into(),
            device_pixel_ratio: 0.0,
            timezone: "America/New_York".into(),
            webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)".into(),
            canvas_hash: "err".into(),
        };
        let report = fp.diagnose();
        let codes: Vec<_> = report.issues.iter().map(|i| i.code.as_str()).collect();
        assert!(report.has_high_risk(), "{report:#?}");
        assert_eq!(report.score, 0);
        assert!(codes.contains(&"ua.headless_chrome"));
        assert!(codes.contains(&"navigator.webdriver"));
        assert!(codes.contains(&"platform.os_mismatch"));
        assert!(codes.contains(&"webgl.os_mismatch"));
        assert!(codes.contains(&"canvas.missing"));
        assert!(codes.contains(&"locale.timezone_mismatch"));
        assert!(!report.fix_plan.is_empty());
        assert_eq!(report.fix_plan.action_count, report.fix_plan.actions.len());
        assert!(report.fix_plan.actions.iter().any(|action| {
            action.code == "profile.align_os"
                && action.priority == IdentityFixPriority::High
                && action
                    .issue_codes
                    .iter()
                    .any(|code| code == "webgl.os_mismatch")
        }));
        assert!(
            report
                .fix_plan
                .actions
                .iter()
                .any(|action| action.target == IdentityFixTarget::Stealth
                    && action.code == "stealth.webdriver_false")
        );
        assert!(report.fix_plan.targets.contains(&IdentityFixTarget::Canvas));
    }

    #[test]
    fn linkability_report_flags_shared_stable_signals() {
        let a = win_profile();
        let report = a.linkability_to(&a);
        let codes: Vec<_> = report.signals.iter().map(|s| s.code.as_str()).collect();
        assert!(report.same_identity_likely, "{report:#?}");
        assert_eq!(report.score, 100);
        assert!(report.has_strong_signal());
        assert!(codes.contains(&"webgl.same"));
        assert!(codes.contains(&"canvas.same"));
    }

    #[test]
    fn linkability_report_allows_distinct_profiles() {
        let a = win_profile();
        let b = mac_profile();
        let report = LinkabilityReport::compare(&a, &b);
        assert!(report.is_distinct(), "{report:#?}");
        assert!(!report.same_identity_likely);
        assert_eq!(report.score, 0);
        assert!(report.signals.is_empty());
    }

    #[test]
    fn identity_drift_report_accepts_stable_profile() {
        let before = win_profile();
        let after = win_profile();
        let report = before.drift_to(&after);

        assert!(report.is_stable(), "{report:#?}");
        assert_eq!(report.score, 0);
        assert_eq!(report.severity, IdentityDriftSeverity::None);
        assert!(!report.stable_hash_changed);
        assert!(report.signals.is_empty());
    }

    #[test]
    fn identity_drift_report_flags_high_risk_profile_drift() {
        let before = win_profile();
        let mut after = win_profile();
        after.webgl_renderer = "ANGLE (Apple, ANGLE Metal Renderer: Apple M2)".into();
        after.canvas_hash = "ffffffff".into();
        after.timezone = "Asia/Tokyo".into();

        let report = before.drift_to(&after);
        let codes = report
            .signals
            .iter()
            .map(|signal| signal.code.as_str())
            .collect::<Vec<_>>();

        assert!(report.has_risky_drift(), "{report:#?}");
        assert!(report.has_high_risk_drift(), "{report:#?}");
        assert_eq!(report.severity, IdentityDriftSeverity::High);
        assert!(report.score >= 60);
        assert!(report.stable_hash_changed);
        assert!(codes.contains(&"webgl.changed"));
        assert!(codes.contains(&"canvas.changed"));
        assert!(codes.contains(&"timezone.changed"));
        assert!(!report.remediation_plan.is_empty());
        assert!(
            report
                .remediation_plan
                .actions
                .iter()
                .any(|action| action.code == "drift.quarantine_current"
                    && action.priority == IdentityFixPriority::High)
        );
        assert!(
            report
                .remediation_plan
                .actions
                .iter()
                .any(|action| action.code == "drift.restore_canvas_seed"
                    && action.target == IdentityDriftRemediationTarget::Canvas)
        );
        assert!(
            report
                .remediation_plan
                .targets
                .contains(&IdentityDriftRemediationTarget::GpuWebgl)
        );
    }

    #[test]
    fn identity_pool_report_flags_duplicate_profiles() {
        let a = win_profile();
        let report = IdentityPoolReport::analyze(&[a.clone(), a.clone()]);
        let duplicate_codes: Vec<_> = report
            .duplicate_signals
            .iter()
            .map(|s| s.code.as_str())
            .collect();
        assert_eq!(report.size, 2);
        assert!(report.pool_id.starts_with("pool_"));
        assert_eq!(report.snapshot_ids.len(), 2);
        assert_eq!(report.snapshot_ids[0], a.identity_id());
        assert_eq!(report.diversity.size, 2);
        assert!(report.diversity.concentrated_signal_count > 0);
        assert!(report.diversity.max_concentration_ratio >= 1.0);
        assert_eq!(report.entropy_budget.size, 2);
        assert_eq!(
            report.entropy_budget.status,
            IdentityEntropyStatus::Collapsed
        );
        assert_eq!(report.entropy_budget.entropy_score, 0);
        assert_eq!(report.entropy_budget.effective_identity_count, 1.0);
        assert!(report.entropy_budget.bottleneck_count > 0);
        assert!(
            report
                .entropy_budget
                .bottleneck_signals
                .iter()
                .any(|signal| signal.code == "canvas.same" && signal.normalized_entropy == 0.0)
        );
        assert_eq!(report.capacity_plan.size, 2);
        assert_eq!(
            report.capacity_plan.status,
            IdentityCapacityStatus::Exhausted
        );
        assert_eq!(report.capacity_plan.additional_distinct_profiles_needed, 1);
        assert_eq!(report.capacity_plan.effective_identity_count, 1.0);
        assert!(
            report
                .capacity_plan
                .actions
                .iter()
                .any(|action| action.code == "capacity.disperse_canvas_seed"
                    && action.priority == IdentityFixPriority::High)
        );
        assert!(
            report
                .diversity
                .signals
                .iter()
                .any(|signal| signal.code == "canvas.same"
                    && signal.unique_count == 1
                    && signal.max_bucket_count == 2)
        );
        assert!(report.has_risky_pairs(), "{report:#?}");
        assert_eq!(report.risky_pairs.len(), 1);
        assert_eq!(report.max_linkability, 100);
        assert!(!report.is_well_separated());
        let clusters = report.risk_clusters();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].indexes, vec![0, 1]);
        assert_eq!(clusters[0].pair_count, 1);
        assert_eq!(clusters[0].max_score, 100);
        assert!(clusters[0].signal_codes.iter().any(|c| c == "canvas.same"));
        let offenders = report.risk_offenders();
        assert_eq!(offenders.len(), 2);
        assert_eq!(offenders[0].index, 0);
        assert_eq!(offenders[0].linked_indexes, vec![1]);
        assert_eq!(offenders[0].max_score, 100);
        assert!(offenders[0].signal_codes.iter().any(|c| c == "webgl.same"));
        let quarantine = report.quarantine_plan();
        assert_eq!(quarantine.indexes, vec![0]);
        assert_eq!(quarantine.covered_pair_count, 1);
        assert_eq!(quarantine.remaining_pair_count, 0);
        let admission = report.admission_plan();
        assert_eq!(admission.action, IdentityAdmissionAction::PartialQuarantine);
        assert_eq!(admission.accept_indexes, vec![1]);
        assert_eq!(admission.quarantine_indexes, vec![0]);
        let remediation = report.remediation_plan();
        assert!(!remediation.is_empty());
        assert_eq!(remediation.quarantine_indexes, vec![0]);
        assert_eq!(
            remediation.action_count,
            report.remediation_plan.action_count
        );
        assert!(
            remediation
                .actions
                .iter()
                .any(|action| action.code == "pool.quarantine_offenders"
                    && action.target == IdentityPoolRemediationTarget::Admission)
        );
        assert!(
            remediation
                .actions
                .iter()
                .any(|action| action.code == "pool.disperse_canvas_seed"
                    && action.priority == IdentityFixPriority::High
                    && action.indexes == vec![0, 1])
        );
        assert!(duplicate_codes.contains(&"webgl.same"));
        assert!(duplicate_codes.contains(&"canvas.same"));
    }

    #[test]
    fn identity_pool_report_accepts_distinct_healthy_profiles() {
        let win = win_profile();
        let mac = mac_profile();
        let report = IdentityPoolReport::analyze(&[win.clone(), mac.clone()]);
        let reversed = IdentityPoolReport::analyze(&[mac, win]);
        assert_eq!(report.size, 2);
        assert_eq!(report.stable_hash, reversed.stable_hash);
        assert_eq!(report.pool_id, reversed.pool_id);
        assert_eq!(report.diversity.size, 2);
        assert!(report.diversity.signal_count > 0);
        assert!(report.entropy_budget.entropy_score > 70);
        assert!(report.entropy_budget.effective_identity_count > 1.7);
        assert!(
            report.entropy_budget.status == IdentityEntropyStatus::Thin
                || report.entropy_budget.status == IdentityEntropyStatus::Diverse
        );
        assert!(report.capacity_plan.effective_identity_count > 1.7);
        assert!(report.capacity_plan.additional_distinct_profiles_needed <= 1);
        assert!(
            report
                .diversity
                .signals
                .iter()
                .any(|signal| signal.code == "canvas.same"
                    && signal.unique_count == 2
                    && signal.repeated_value_count == 0)
        );
        assert!(report.is_well_separated(), "{report:#?}");
        assert_eq!(report.max_linkability, 0);
        assert!(report.risky_pairs.is_empty());
        assert!(report.risk_clusters().is_empty());
        assert!(report.risk_offenders().is_empty());
        assert!(report.quarantine_plan().indexes.is_empty());
        assert!(report.remediation_plan.is_empty());
        assert_eq!(
            report.admission_plan().action,
            IdentityAdmissionAction::Accept
        );
        assert!(report.duplicate_signals.is_empty());
    }

    fn win_profile() -> FingerprintSnapshot {
        FingerprintSnapshot {
            ua: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36".into(),
            platform: "Win32".into(),
            ua_data_platform: "Windows".into(),
            ua_data_mobile: false,
            webdriver: false,
            languages: "en-US,en".into(),
            max_touch_points: 0,
            hardware_concurrency: 12,
            device_memory: 8.0,
            screen: "1920x1080".into(),
            device_pixel_ratio: 1.0,
            timezone: "America/New_York".into(),
            webgl_renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into(),
            canvas_hash: "a1b2c3d4".into(),
        }
    }

    fn mac_profile() -> FingerprintSnapshot {
        FingerprintSnapshot {
            ua: "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 Safari/605.1.15"
                .into(),
            platform: "MacIntel".into(),
            ua_data_platform: String::new(),
            ua_data_mobile: false,
            webdriver: false,
            languages: "ja-JP,ja".into(),
            max_touch_points: 0,
            hardware_concurrency: 8,
            device_memory: 4.0,
            screen: "2560x1440".into(),
            device_pixel_ratio: 2.0,
            timezone: "Asia/Tokyo".into(),
            webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M2)".into(),
            canvas_hash: "ffeeddcc".into(),
        }
    }
}
