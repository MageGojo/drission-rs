# drs CLI / MCP

`drs` 是 `drission` 的同仓库命令行与 MCP 入口。它面向 AI Agent 和自动化脚本:浏览器由一个本地 daemon 持有,普通命令通过本地 JSONL 协议驱动同一组标签;MCP 模式默认也 **attach 到同一个常驻 daemon 浏览器**,所以 CLI 与 MCP 共享标签和登录态,MCP server 重启不会丢浏览器。`serve` / `ensure-serve` / `mcp` 默认使用一个固定的用户 profile 目录(`<cache>/drission/cli/profile`),cookie 和登录态跨重启存活。

## 安装

从 crates.io 安装:

```bash
cargo install drission-cli --bin drs
```

默认构建 CDP/Chrome 后端。按需启用能力:

```bash
cargo install drission-cli --bin drs --features cdp,ocr
cargo install drission-cli --bin drs --no-default-features --features camoufox
```

开发期从本仓库安装:

```bash
cargo install --path crates/drission-cli --bin drs
```

## Daemon 模式

启动:

```bash
drs serve --backend cdp --headless
```

若 daemon 未运行,可自动后台拉起:

```bash
drs ensure-serve --backend cdp --headless
# 或在任意 daemon 命令前加全局 flag:
drs --ensure-serve --ensure-headless --json open https://example.com
```

连接信息写入用户缓存目录下的 `drission/cli/drs-server.json`,包含 `host`、`port`、`token`、`pid` 和 `backend`。其它命令会读取该文件并带 token 调用本地 daemon。

常用命令:

```bash
drs --json status
drs --json open https://example.com
drs --json extract https://example.com --save-out ./page.json
drs --json extract https://example.com --pass-cf --wait-selector "h1" --timeout-ms 5000
drs --json title
drs --json url
drs --json tabs
drs --json use 1
drs ax --outline
drs --json ax --json
drs html
drs text h1
drs eval "document.title"
drs click "text:登录"
drs type "#kw" "drission"
drs press Enter --selector "#kw"
drs wait "#result" --timeout-ms 5000
drs screenshot --out /tmp/page.png --full
drs listen start /api/ --xhr-only
drs --json listen wait --count 3 --timeout-ms 5000
drs listen stop
drs pass-cf --timeout-ms 30000
drs --json identity
drs --json identity --pool
drs --json identity --pool --snapshots-out ./snapshots.json
drs --json identity --pool --snapshots-out ./snapshots.ndjson --append-snapshots
drs --json identity --pool --gate-preset balanced
drs --json identity --pool --min-score 80 --max-linkability 25 --fail-on-risky-pairs
drs --json identity-pool ./snapshots.json --max-linkability 25
drs --json identity-pool ./candidates.json --against ./baseline.ndjson --gate-preset strict
drs --json identity-pool ./candidates.json --policy ./identity-policy.json --against ./baseline.ndjson
drs --json identity-pool ./candidates.json --against ./baseline.ndjson --accept-out ./accepted.json --quarantine-out ./quarantine.json --baseline-out ./next-baseline.json --ledger-out ./ledger.json
drs --json identity-pool ./candidates.json --actions-out ./pool-actions.ndjson --append-actions
drs --json identity-drift ./baseline.json ./current.json --match-by label --max-drift-score 20 --fail-on-high-risk-drift --actions-out ./drift-actions.ndjson --append-actions
drs --json identity-lifecycle ./baseline.json ./current.json --policy ./identity-policy.json --actions-out ./lifecycle-actions.ndjson --append-actions
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles --journal-out ./apply.ndjson --append-journal
drs close
drs stop
```

机器读取建议始终使用 `drs --json ...`。成功响应:

```json
{ "ok": true, "data": {} }
```

失败响应:

```json
{ "ok": false, "error": { "code": "daemon_not_running", "message": "...", "hint": "..." } }
```

## 身份 / 指纹诊断

当前标签身份一致性:

```bash
drs --json identity
```

输出 `report.identityId`、`report.stableHash`、`report.score`、原始 `snapshot`、按严重度排序的 `issues` 和结构化 `report.fixPlan`,用于检查 `HeadlessChrome`、`navigator.webdriver`、UA / platform / Client Hints / WebGL OS 冲突、软件渲染、语言与时区强冲突等明显露馅点。`fixPlan.actions[]` 会给出稳定动作码、目标配置层、优先级、涉及字段和触发 issue codes,适合 AI Agent / 调度器自动生成修复配置。

把 daemon 中全部标签作为账号 / 浏览器身份池做关联性自检:

```bash
drs --json identity --pool
```

输出 `report.poolId`、`report.snapshotIds`、`report.max_linkability`、`risky_pairs` 和 `duplicate_signals`。`tabs[].index` 对应报告里的 `left_index` / `right_index`,方便把风险 pair 映射回具体标签;`snapshotIds[]` 用于跨轮 baseline / 日志追踪。

保存在线采集到的快照:

```bash
drs --json identity --snapshots-out ./one-tab.json
drs --json identity --pool --snapshots-out ./snapshots.json
drs --json identity --pool --snapshots-out ./snapshots.ndjson --append-snapshots
```

默认覆盖写 `FingerprintSnapshot` JSON 数组;`--append-snapshots` 会追加 NDJSON,每行一个 snapshot。成功响应会包含 `snapshotsOut.path`、`snapshotsOut.count` 和实际格式。

身份诊断也可以作为账号池准入门禁。设置任意 gate 条件后,CLI 会在报告照常输出的同时,在 `gate.passed=false` 时返回退出码 `2`:

```bash
drs --json identity --pool --gate-preset balanced
drs --json identity-pool ./snapshots.json --gate-preset strict
drs --json identity --min-score 80 --fail-on-high-risk
drs --json identity --pool --min-score 80 --max-linkability 25 --fail-on-risky-pairs
```

可用条件:

- `--gate-preset lenient|balanced|strict`:准入策略预设。
- `--min-score N`:每个标签身份一致性分数必须不低于 `N`。
- `--max-linkability N`:身份池最高关联分必须不高于 `N`。
- `--max-concentration-ratio R`:池内任一稳定信号最大集中桶占比必须不高于 `R`,例如 `0.8`。
- `--max-concentrated-signals N`:池内出现重复值的稳定信号数量必须不高于 `N`。
- `--min-entropy-score N`:池级身份熵评分必须不低于 `N`。
- `--min-effective-identities N`:池级有效身份数必须不低于 `N`,可用小数。
- `--max-nominal-to-effective-ratio R`:名义账号数 / 有效身份数必须不高于 `R`。
- `--fail-on-high-risk`:任一标签存在高风险身份问题即失败。
- `--fail-on-risky-pairs`:身份池存在风险 pair 即失败。

预设阈值:

| preset | min-score | max-linkability | max-concentrated-signals | min-entropy-score | max-nominal/effective | high-risk | risky-pairs |
|---|---:|---:|---:|---:|---:|---|---|
| `lenient` | 70 | 60 | - | - | - | fail | allow |
| `balanced` | 80 | 30 | 8 | 55 | - | fail | fail |
| `strict` | 90 | 20 | 4 | 70 | 2.0 | fail | fail |

显式传入的 `--min-score` / `--max-linkability` / `--max-concentration-ratio` / `--max-concentrated-signals` / `--min-entropy-score` / `--min-effective-identities` / `--max-nominal-to-effective-ratio` 会覆盖 preset 中的数值;布尔失败条件会与 preset 合并。

离线分析无需启动 daemon:

```bash
drs --json identity-pool ./snapshots.json --max-linkability 25 --fail-on-risky-pairs
drs --json identity-pool ./snapshots.json --max-concentration-ratio 0.8 --max-concentrated-signals 3 --min-entropy-score 60 --max-nominal-to-effective-ratio 2
drs --json identity-pool ./candidates.json --against ./baseline.ndjson --gate-preset strict
drs --json identity-pool ./candidates.json --policy ./identity-policy.json --against ./baseline.ndjson
drs --json identity-pool ./candidates.json --against ./baseline.ndjson --accept-out ./accepted.json --quarantine-out ./quarantine.json --baseline-out ./next-baseline.json --ledger-out ./ledger.json
drs --json identity-pool ./candidates.json --accept-out ./accepted.ndjson --quarantine-out ./quarantine.ndjson --append-split
drs --json identity-pool ./candidates.json --ledger-out ./ledger.ndjson --append-ledger
drs --json identity-pool ./candidates.json --actions-out ./pool-actions.ndjson --append-actions
```

`snapshots.json` 支持以下形状:

- `[{...}, {...}]`: `FingerprintSnapshot` 数组。
- `{ "snapshots": [{...}] }`:带外层字段的快照文件。
- `{"ua": "...", ...}`:单个快照。
- NDJSON:每行一个快照或一个 `drs` 输出对象。
- `drs --json identity` / `drs --json identity --pool` 保存下来的完整输出。

离线入口同样返回 `gate`,同样在 gate 失败时返回退出码 `2`。
传 `--against baseline.json` 时,`snapshots.json` 会作为候选池,`baseline.json` 作为已有身份池逐一比较。输出会增加:

- `againstPath`:基线文件路径。
- `againstReport.maxLinkability`:候选与基线之间的最高关联分。
- `againstReport.candidateIds` / `baselineIds`:候选 / 基线画像稳定 ID,顺序与输入快照一致。
- `againstReport.riskyPairs`:跨池风险 pair,字段为 `candidateIndex` / `candidateId` / `baselineIndex` / `baselineId`。
- `againstReport.clusters`:跨池风险簇,字段为 `candidateIndexes` / `candidateIds` / `baselineIndexes` / `baselineIds`。
- `againstReport.candidateOffenders` / `baselineOffenders`:按风险贡献排序的候选 / 基线下标与 `identityId`。
- `againstReport.candidateQuarantine`:建议优先隔离 / 替换的候选下标与 `candidateIds`,用于覆盖跨池风险 pair。

gate 的 `--max-linkability` 与 `--fail-on-risky-pairs` 会同时检查候选池内部和候选 vs baseline 的跨池风险。
`ledger` 是候选级身份账本,字段为 `candidateCount`、`acceptedCount`、`quarantineCount`、`knownBaselineCount`、`duplicateCandidateCount`、`riskyInternalCount`、`riskyBaselineCount` 和 `entries`。每个 entry 都包含 `index`、`identityId`、`decision`、`knownInBaseline`、`duplicateInBatch`、内部/基线关联下标与 ID、最高内部/基线关联分、`signalCodes` 和 `reasons`;调度器可优先按 ledger entry 执行。
`diversity` 是池级稳定信号多样性报告,字段为 `size`、`signalCount`、`concentratedSignalCount`、`maxConcentrationRatio`、`averageUniqueRatio` 和 `signals`;每个 signal 会列出唯一值数量、重复值数量、最大桶占比和 buckets。`--max-concentration-ratio` 与 `--max-concentrated-signals` 会把这份报告变成准入门禁,用于拦截"整批画像太像同一模板"的账号池。在线/MCP 池报告里同一对象位于 `report.diversity`。
`entropyBudget` 是池级身份熵预算,字段为 `effectiveIdentityCount`、`nominalToEffectiveRatio`、`entropyScore`、`status`、`bottleneckSignals` 等。它基于 `diversity.signals[].buckets` 做 Shannon entropy 估计,并按 strong / medium / weak 信号加权,用于回答"名义 N 个账号实际画像容量像多少个身份"。`--min-entropy-score`、`--min-effective-identities`、`--max-nominal-to-effective-ratio` 会把它变成硬门禁。在线/MCP 池报告里同一对象位于 `report.entropyBudget`。
`capacityPlan` 会把 `entropyBudget` 翻译成扩容计划,字段为 `missingEffectiveIdentityCount`、`additionalDistinctProfilesNeeded`、`status`、`bottleneckSignals` 和 `actions`。动作码包括 `capacity.disperse_canvas_seed`、`capacity.disperse_webgl_renderer`、`capacity.rotate_locale_proxy`、`capacity.rotate_browser_persona` 等,用于告诉调度器优先增加哪类画像差异。离线 `identity-pool` 会把这些动作以 `source=capacity` 合并进顶层 `actionQueue.actions[]`;在线/MCP 池报告里同一对象位于 `report.capacityPlan`。
普通在线/离线池报告还会返回 `clusters`,把风险 pair 自动合成连通簇;每个簇包含 `indexes`、`pairCount`、`maxScore`、`strongSignalCount` 和 `signalCodes`,用于快速看出哪些账号/画像会被归成一团。
`offenders` 会把每个风险下标按 `maxScore`、`pairCount`、`strongSignalCount` 排序,用于决定优先替换或隔离哪几个画像。
`quarantine` 是基于风险 pair 的贪心隔离计划,字段为 `indexes`、`coveredPairCount`、`remainingPairCount`、`maxCoveredScore`;它回答的是"先动哪几个画像能最快拆掉风险边"。
`remediation` 是池级修复计划,字段为 `actionCount`、`highPriorityCount`、`quarantineIndexes`、`targets` 和 `actions`;每个 action 会给出稳定动作码、目标范围、优先级、受影响下标、触发 signal codes 和重复值样本。在线/MCP 池报告里同一对象位于 `report.remediationPlan`。
`actionQueue` 是离线池审计的扁平动作队列,会把 `admission` 里的候选隔离动作、`remediation` 里的池级修复动作和 `capacity` 里的扩容/分散动作合并到 `actions[]`;每条 action 都带 `source`、`actionCode`、`target`、`priority`、受影响下标、稳定 `identityIds`、原因、触发 signal codes、关联到的内部/基线 ID 和最高关联分。`source=capacity` 的动作还会带 `estimatedGain`,表示该动作预计可补回的有效画像容量。传 `--actions-out actions.json` 会覆盖写完整 action queue;加 `--append-actions` 会逐行追加 `actionQueue.actions[]` 为 NDJSON。响应会包含 `actionsOut.path`、`actionsOut.count`、`actionsOut.append` 和 `actionsOut.format`。
`admission` 是最终准入计划,字段为 `action`、`acceptIndexes`、`acceptIds`、`quarantineIndexes`、`quarantineIds`、`acceptCount`、`quarantineCount`;离线 `--against` 模式会同时合并候选池内部风险和 baseline 撞库风险。
传 `--accept-out` / `--quarantine-out` 时,CLI 会按 `admission.acceptIndexes` / `admission.quarantineIndexes` 直接写出 snapshots 文件;默认覆盖写 JSON 数组,`--append-split` 则追加 NDJSON。响应会包含 `splitOut.accepted` / `splitOut.quarantine` 的路径和数量。
传 `--baseline-out next-baseline.json` 时,CLI 会写出下一轮完整基线:有 `--against` 时为"旧 baseline + 本轮 accepted";无 `--against` 时为本轮 accepted。该文件固定覆盖写 JSON 数组,响应会包含 `baselineOut.path`、`baselineOut.count`、`baselineOut.baselineCount`、`baselineOut.acceptedAdded` 和 `baselineOut.format`。
传 `--ledger-out ledger.json` 时,CLI 会覆盖写完整候选级 ledger JSON report;加 `--append-ledger` 时会把 `ledger.entries[]` 逐行追加为 NDJSON。响应会包含 `ledgerOut.path`、`ledgerOut.count`、`ledgerOut.append` 和 `ledgerOut.format`。

身份治理策略可以写成 JSON 文件,在离线 `identity-pool`、`identity-drift`、`identity-lifecycle`、运行态 `identity-assets-health` 和 `identity-job run` 中通过 `--policy file.json` 复用。策略文件是默认规则,命令行显式传入的数值和 true flag 会覆盖本次运行:

```json
{
  "gatePreset": "balanced",
  "maxLinkability": 25,
  "maxConcentratedSignals": 6,
  "minEntropyScore": 60,
  "minEffectiveIdentities": 20,
  "maxNominalToEffectiveRatio": 2,
  "drift": {
    "matchBy": "label",
    "maxDriftScore": 20,
    "failOnHighRiskDrift": true
  },
  "lifecycle": {
    "failOnMissingCurrent": true,
    "failOnNewCurrent": true,
    "nextBaselinePolicy": "conservative"
  },
  "health": {
    "windowSeconds": 86400,
    "repairThreshold": 3,
    "quarantineThreshold": 5,
    "cooldownSeconds": 1800
  },
  "job": {
    "preset": "publish_conservative",
    "desiredConcurrency": 5,
    "limit": 5,
    "leaseSeconds": 900,
    "maxWaitSeconds": 600,
    "allowWait": false,
    "perAsset": true,
    "childConcurrency": 5,
    "runtimeRenewIntervalSeconds": 300,
    "childTimeoutSeconds": 1800,
    "childResultDir": "child-results",
    "maxFailedAssets": 1,
    "maxFailedAssetsPerReason": 2,
    "allowState": ["active"],
    "failureCooldownSeconds": 600,
    "failureNextState": "repair",
    "runtimeRiskLedgers": ["runtime-risk.ndjson"],
    "runtimeRiskWindowSeconds": 900,
    "runtimeRiskOut": "runtime-risk.ndjson",
    "appendRuntimeRisk": true,
    "explainOut": "job-explain.json",
    "failureReasonRules": {
      "rate_limited": {
        "cooldownSeconds": 900,
        "nextState": "repair",
        "recommendedAction": "pause_failure_reason",
        "runtimeRiskSeverity": "critical",
        "nextSuggestedLimit": 0,
        "nextSuggestedDesiredConcurrency": 0,
        "runtimeRiskMessage": "pause rate-limited publish jobs",
        "runtimeRiskCooldownSeconds": 1800
      },
      "risk_control": {
        "cooldownSeconds": 3600,
        "nextState": "quarantine"
      }
    }
  }
}
```

也可以把池级 gate 放入 `gate` 分组。`health` 分组用于 runtime release ledger 的自动判责:连续失败达到 repair/quarantine 阈值时,`identity-assets-health --asset-manifest-out` 会把资产写入 `repair` / `quarantine` 并加 cooldown。`job` 分组用于 `identity-job run` 的 sidecar 默认值:业务风险预设、并发、领取数量、每账号子进程并发、运行中自动续租、子进程超时、业务结果文件目录、池级失败熔断、同业务失败原因熔断、runtime risk 建议流水、下一轮 runtime risk 预启动门禁、explain 审计文件、租约、可等待时间、失败冷却和失败后状态都可以沉淀成团队规则,业务命令只传 `--policy identity-policy.json -- python publish.py`。响应会附带 `policy.path`、`policy.format` 和原始 `policy.rules`,让审计 journal 能追溯本轮使用的治理规则。内置 `job.preset` / `--job-preset` 支持 `publish_conservative`、`login_sensitive`、`scrape_aggressive`,分别面向发布类保守、登录态敏感和采集类激进任务;显式 policy 字段和命令行参数仍可覆盖预设里的单项阈值。

跨轮漂移审计用于比较同一批账号 / profile 在两轮采样中的稳定画像是否变化:

```bash
drs --json identity-drift ./baseline.json ./current.json
drs --json identity-drift ./baseline.json ./current.json --match-by label --max-drift-score 20 --fail-on-high-risk-drift
drs --json identity-drift ./baseline.json ./current.json --policy ./identity-policy.json
drs --json identity-drift ./baseline.json ./current.json --match-by label --actions-out ./drift-actions.json
drs --json identity-drift ./baseline.json ./current.json --match-by label --actions-out ./drift-actions.ndjson --append-actions
```

`identity-drift BEFORE AFTER` 默认 `--match-by auto`:两边快照都有标签时按标签匹配,否则按输入顺序对齐。显式传 `--match-by index|label` 可强制指定策略。标签字段支持 `accountId`、`account_id`、`profileId`、`profile_id`、`identityKey`、`label`、`id`、`name`、`key`;每项既可以是 `{ "accountId": "acct-1", "snapshot": {...} }`,也可以是 `{ "accountId": "acct-1", ...snapshot fields... }`。这适合比较"上一轮已入池 baseline"与"本轮启动后采样",即使两轮文件顺序不同也能按账号/profile 对齐。输出包含:

- `beforeCount` / `afterCount` / `pairCount`:两轮文件数量与实际比较的对数。
- `changedCount` / `stableCount` / `highRiskCount` / `maxScore`:漂移汇总。
- `matchBy` / `requestedMatchBy`:实际匹配策略与用户请求策略。
- `missingBeforeIndexes` / `missingAfterIndexes`:按下标匹配且两轮数量不一致时未匹配的下标。
- `missingBeforeLabels` / `missingAfterLabels`:按标签匹配时只出现在一侧的账号/profile。
- `entries[]`:每个匹配项的 `label`、`beforeIndex`、`afterIndex`、`beforeId`、`afterId`、稳定哈希是否变化、`score`、`severity`、`stable`、`highRisk`、`signals` 和 `remediation`。

漂移 signal 会列出 `code`、`before`、`after`、`severity`、`message` 和 `suggestion`。高风险漂移包括 WebGL renderer、canvas hash、OS/platform/Client Hints、移动/桌面标记、`navigator.webdriver` 等核心身份信号突变;中低风险漂移包括 locale、timezone、screen/DPR、硬件低熵字段和 UA 小/大版本变化。`entries[].remediation.actions[]` 会把信号合并成稳定动作码,例如 `drift.quarantine_current`、`drift.restore_canvas_seed`、`drift.restore_webgl_renderer`、`drift.hide_webdriver`、`drift.rebind_locale_proxy`、`drift.sync_user_agent`;每个 action 都包含 `target`、`priority`、`fields`、`signalCodes`、旧值样本和新值样本。响应顶层 `actionQueue` 是这些动作的扁平队列,每条动作都带 `label`、`beforeIndex`、`afterIndex`、`driftScore` 和 `actionCode`;传 `--actions-out file.json` 会覆盖写完整 action queue,加 `--append-actions` 会逐行追加 `actionQueue.actions[]` 为 NDJSON。响应会包含 `actionsOut.path`、`actionsOut.count`、`actionsOut.append` 和 `actionsOut.format`。设置 `--max-drift-score` 或 `--fail-on-high-risk-drift` 后,gate 失败同样返回退出码 `2`。

profile 生命周期治理把漂移审计提升成账号池状态机:

```bash
drs --json identity-lifecycle ./baseline.json ./current.json
drs --json identity-lifecycle ./baseline.json ./current.json --match-by label --max-drift-score 20 --fail-on-high-risk-drift
drs --json identity-lifecycle ./baseline.json ./current.json --match-by label --fail-on-missing-current --fail-on-new-current
drs --json identity-lifecycle ./baseline.json ./current.json --policy ./identity-policy.json
drs --json identity-lifecycle ./baseline.json ./current.json --match-by label --ledger-out ./lifecycle.json --delta-out ./lifecycle-delta.json --journal-out ./lifecycle-journal.ndjson --append-journal --state-out-dir ./lifecycle-states --actions-out ./lifecycle-actions.ndjson --append-actions
drs --json identity-lifecycle ./baseline.json ./current.json --match-by label --next-baseline-out ./next-baseline.json
drs --json identity-lifecycle ./baseline.json ./current.json --match-by label --next-baseline-out ./next-baseline.json --next-baseline-policy active-only
```

`identity-lifecycle BASELINE CURRENT` 默认同样 `--match-by auto`:两边都有标签时按账号/profile 标签匹配,否则按输入顺序匹配。输出 `ledger.entries[]`,每个 profile 会被标成 `active`、`repair`、`quarantine`、`missing_current` 或 `new_current`。`active` 表示当前采样与基线稳定;`repair` 表示有中低风险漂移但可修复;`quarantine` 表示高风险或超过 `--max-drift-score`;`missing_current` 表示基线中存在但本轮采样缺席;`new_current` 表示当前出现但未进入基线。顶层 `summary` 汇总各状态数量,`gate` 可用 `--max-drift-score`、`--fail-on-high-risk-drift`、`--fail-on-missing-current`、`--fail-on-new-current` 做巡检门禁。

生命周期 `actionQueue` 会合并两类动作:一类是状态机动作,例如 `lifecycle.quarantine_profile`、`lifecycle.investigate_missing_current`、`lifecycle.review_new_profile`;另一类是 drift remediation 动作,例如 `drift.restore_canvas_seed`、`drift.restore_webgl_renderer`。传 `--ledger-out file.json` 会覆盖写完整生命周期账本,加 `--append-ledger` 会追加 `ledger.entries[]` 为 NDJSON;传 `--delta-out file.json` 会写出 baseline/current/next-baseline 的变更集,包括 `baseline_retained`、`baseline_updated`、`baseline_removed`、`current_excluded`、`new_current_unadmitted`;传 `--journal-out file.json` 会写出一条本轮审计 run record,加 `--append-journal` 会把 run record 追加为 NDJSON,适合作为多轮审计流水;传 `--state-out-dir dir` 会写出 `active.json`、`repair.json`、`quarantine.json`、`missing_current.json`、`new_current.json`,每个文件都是可重新读取的 labeled snapshot 数组,调度器可以直接按状态消费;传 `--actions-out file.json` 会覆盖写完整动作队列,加 `--append-actions` 会追加动作流水。
传 `--next-baseline-out file.json` 时,CLI 会按生命周期状态写出下一轮可直接读取的 baseline 数组,每项保留 `label` 和 `snapshot`。默认 `--next-baseline-policy conservative`:稳定的 `active` 使用当前快照,`repair` / `missing_current` 保留旧 baseline,`quarantine` / `new_current` 不写入。`active-only` 只保留本轮稳定的当前快照;`accept-current-repair` 会把 `active` 和 `repair` 的当前快照写入,但仍排除 `quarantine`、`missing_current` 和 `new_current`。响应里的 `nextBaseline` 会给出保留数量、来源数量、跳过状态和实际 entries。

动作队列执行器把治理建议变成可审计的 profile 操作:

```bash
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles --journal-out ./apply.json
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles --journal-out ./apply.ndjson --append-journal
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles --execute --journal-out ./apply.ndjson --append-journal
drs --json identity-apply ./pool-actions.json --profile-root ./profiles --profile-map ./profile-map.json --quarantine-dir ./quarantine
drs --json identity-apply ./lifecycle-actions.ndjson --profile-root ./profiles --profile-map ./profile-assets.json --asset-state-out ./asset-state.ndjson --append-asset-state
```

`identity-apply ACTIONS` 支持读取 `identity-pool`、`identity-drift`、`identity-lifecycle` 的 `--actions-out` JSON report、追加式 NDJSON action 流水,也支持完整 `drs --json ...` 输出里的 `data.actionQueue.actions[]`。默认是 dry-run,输出 `operations[]`、`assetPatches[]` 和可选 journal;只有显式传 `--execute` 才会移动文件。

profile 解析顺序:

- 先查 `--profile-map` 中的 label / `identityId` 映射;该文件既可以是 `{ "acct-a": "/path/to/profile" }`,也可以是 `{ "profileAssets": [...] }` / `{ "assets": [...] }` manifest。manifest 条目支持 `accountId`、`profileId`、`identityId`、`label`、`profileDir` / `profilePath` / `userDataDir`、`proxyId`、`fingerprintSeed`、`state`。
- 再查 `--profile-root/<label>` 或 `--profile-root/<identityId>`。
- 默认隔离目录是 `--profile-root/_quarantine`,也可以用 `--quarantine-dir` 指定。
- 如果命中 manifest 资产,响应会把资产详情写进 `operations[].asset`,并把动作翻译成 `assetPatches[]`:隔离动作对应 `nextState=quarantine`,修复动作对应 `nextState=repair`,review/investigate 动作分别对应 `review` / `investigate`。传 `--asset-state-out file.json` 会覆盖写完整状态 patch 报告;加 `--append-asset-state` 会逐行追加 NDJSON patch。

`*.quarantine*` 动作会被解析成 `quarantine_profile` 操作;例如 `pool.quarantine_candidate`、`drift.quarantine_current`、`lifecycle.quarantine_profile`。其它 `drift.restore_*`、`pool.disperse_*`、`lifecycle.review_new_profile` 等动作不会盲目改文件,会以 `skipped` / `action_not_file_mutating` 留在计划里,由上层调度器生成具体修复配置。传 `--journal-out` 默认覆盖写完整 JSON report;加 `--append-journal` 会把每个 operation 逐行追加为 NDJSON,方便长期执行审计。

治理计划汇总器把多份产物合成一次可执行复盘:

```bash
drs --json identity-plan ./pool-actions.ndjson ./lifecycle-actions.ndjson ./apply.ndjson ./asset-state.ndjson
drs --json identity-plan ./pool.json ./drift-actions.ndjson ./asset-state.ndjson --title "Nightly identity audit" --out ./identity-plan.json --html-out ./identity-plan.html --dispatch-out ./dispatch.json
drs --json identity-plan ./pool.json ./asset-state.ndjson --asset-manifest ./profile-assets.json --asset-manifest-out ./next-profile-assets.json
drs --json identity-plan ./pool.json ./asset-state.ndjson --dispatch-out ./dispatch.ndjson --append-dispatch
drs --json identity-dispatch ./dispatch.ndjson --worker worker-a --limit 10 --lease-seconds 900
drs --json identity-dispatch ./dispatch.ndjson --worker worker-a --limit 10 --claim-ledger ./claims.ndjson --claim-out ./claims.ndjson --append-claim
drs --json identity-dispatch-renew ./claims.ndjson --worker worker-a --lease-seconds 900 --claim-out ./claims.ndjson --append-claim
drs --json identity-dispatch-complete ./claims.ndjson --status succeeded --worker worker-a --complete-out ./completed.ndjson --append-complete
drs --json identity-dispatch-reconcile ./profile-assets.json --claim-ledger ./claims.ndjson --completion-ledger ./completed.ndjson --asset-manifest-out ./runtime-profile-assets.json
drs --json identity-assets-validate ./runtime-profile-assets.json --strict --validate-out ./asset-validate.json
drs --json identity-assets-status ./runtime-profile-assets.json --desired-concurrency 5 --status-out ./asset-status.json
drs --json identity-assets-forecast ./runtime-profile-assets.json --desired-concurrency 5 --horizon-seconds 3600 --forecast-out ./asset-forecast.json
drs --json identity-assets-gate ./runtime-profile-assets.json --desired-concurrency 5 --max-wait-seconds 600 --gate-out ./asset-gate.json
drs --json identity-assets-select ./runtime-profile-assets.json --limit 5 --worker worker-a --job publish --asset-manifest-out ./leased-profile-assets.json --selection-out ./selection.json
drs --json identity-assets-release ./leased-profile-assets.json --worker worker-a --job publish --status succeeded --asset-manifest-out ./released-profile-assets.json --release-out ./runtime-release.ndjson --append-release
drs --json identity-assets-reconcile-runtime ./runtime-profile-assets.json --release-ledger ./runtime-release.ndjson --asset-manifest-out ./reconciled-profile-assets.json
drs --json identity-assets-health ./reconciled-profile-assets.json --policy ./identity-policy.json --release-ledger ./runtime-release.ndjson --repair-threshold 3 --quarantine-threshold 5 --asset-manifest-out ./health-profile-assets.json --health-out ./asset-health.json
drs --json identity-assets-sweep ./health-profile-assets.json --asset-manifest-out ./clean-profile-assets.json --sweep-out ./sweep.json
drs --json identity-job run ./clean-profile-assets.json --per-asset --child-concurrency 5 --runtime-renew-interval-seconds 300 --child-timeout-seconds 1800 --child-result-dir ./child-results --max-failed-assets 1 --max-failed-assets-per-reason 2 --limit 5 --worker worker-a --job publish --asset-manifest-out ./job-profile-assets.json --release-out ./runtime-release.ndjson --append-release --runtime-risk-out ./runtime-risk.ndjson --append-runtime-risk -- python publish.py
drs --json identity-ledger compact --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --retain-recent 50 --checkpoint-out ./ledger-checkpoint.json --out ./ledger-compact.json
drs --json identity-ledger dashboard --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --checkpoint-in ./ledger-checkpoint.json --checkpoint-out ./ledger-checkpoint.json --out ./ledger-dashboard.json --html-out ./ledger-dashboard.html
drs --json identity-ledger query --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --reason rate_limited --out ./ledger-query.json
drs --json identity-ledger explain --release-ledger ./runtime-release.ndjson --runtime-risk-ledger ./runtime-risk.ndjson --window-seconds 86400 --job publish --reason rate_limited --account-id acct-a --out ./ledger-explain.json
drs --json identity-dispatch ./dispatch.ndjson --worker worker-b --limit 10 --claim-ledger ./claims.ndjson --completion-ledger ./completed.ndjson --claim-out ./claims.ndjson --append-claim
```

`identity-plan INPUT...` 会宽松读取完整 `drs --json` 响应、action queue JSON、追加式 action NDJSON、`identity-apply` report 和 `--asset-state-out --append-asset-state` 产物。输出 `summary`、`inputs[]`、`actionCodeCounts`、`priorityCounts`、`stateCounts`、`recommendations[]`、调度器 runbook、dispatch queue、归一化 `actions[]` 和 `assetPatches[]`。它会把 gate failure、隔离动作、修复动作、capacity 扩容动作、apply failed/unresolved、资产状态 patch 汇总到同一个计划里;`executionRunbook[]` 会按 `gate_review`、`quarantine`、`repair`、`capacity`、`review`、`manifest_writeback`、`resample_verify` 阶段排序,每步包含 `actionIndexes[]` / `assetPatchIndexes[]`、数量、说明和可选命令提示。`dispatchQueue.items[]` 会把 runbook 展开成可领取工作项,每项带 `kind`、`phase`、`sortRank`、`dedupeKey`、`leaseKey`、目标账号/profile 字段和可选命令提示;传 `--dispatch-out file.json` 会覆盖写完整 dispatch queue,加 `--append-dispatch` 会逐行追加 NDJSON 工作项。传 `--out` 会写机器可读 JSON report,传 `--html-out` 会写独立 HTML 审计报告。传 `--asset-manifest profile-assets.json --asset-manifest-out next-profile-assets.json` 会按 `accountId`、`profileId`、`identityId`、`label` 或 `profileDir` 匹配资产,把命中的 patch 写回下一轮 manifest:更新 `state`,当 patch `status=applied` 且带 `destinationPath` 时同步更新 `profileDir`,并追加 `lastIdentityPlanRunId`、`lastIdentityPlanActionCode`、`lastIdentityPlanUpdatedAtUnixSeconds` 等审计字段;响应里的 `assetManifestOut` 会给出命中数、未命中 patch 数和回写后的状态分布。

`identity-dispatch DISPATCH` 会读取完整 `identity-plan` 输出、`--dispatch-out` JSON report 或追加式 NDJSON 工作流,按 `sortRank`、`dispatchIndex`、`dedupeKey` 稳定排序。默认跳过 `blockedByGate=true` 的工作项,传 `--include-blocked` 可强制领取;同一个 `dedupeKey` 只会领取一次。传 `--claim-ledger claims.ndjson` 会读取历史 claim report / NDJSON claim item,按 `leaseExpiresUnixSeconds` 识别 active lease 并跳过仍未过期的 `dedupeKey`;传 `--include-leased` 才会重领这些工作项。传 `--completion-ledger completed.ndjson` 会读取历史 completion report / NDJSON completion item,默认跳过最新状态为 `succeeded`、`cancelled` 或不可重试 `failed` 的 `dedupeKey`;`retry` 或 `failed --retryable` 会保留为可重领任务,需要强制重领终态任务时才传 `--include-completed`。响应包含 `claimId`、`workerId`、`leaseExpiresUnixSeconds`、`claimedPhases[]`、`items[]`、`activeLeaseCount`、`expiredLeaseCount`、`completionLedgerCount`、`terminalCompletionCount`、`retryableCompletionCount`、`skippedBlockedCount`、`skippedLeasedCount`、`skippedCompletedCount` 和 `duplicateDedupeKeyCount`。传 `--claim-out file.json` 会覆盖写完整 claim report;加 `--append-claim` 会逐行追加 NDJSON claim item,每行包含 worker、claim、lease 和原始 dispatch payload。

`identity-dispatch-renew CLAIMS` 会读取 claim report / NDJSON claim item,为仍在执行的任务追加新的 lease 记录,适合作为 worker 心跳。默认只续 `status=leased` 且尚未过期的 item;传 `--include-expired` 才会抢救已过期租约。可用 `--worker`、`--claim-id`、重复的 `--dedupe-key` 限定续租范围;同一 `dedupeKey` 在流水里出现多次时只续最新/最长的那条。续租 item 会带 `renewalId`、`renewedAtUnixSeconds` 和 `previousLeaseExpiresUnixSeconds`;传 `--claim-out claims.ndjson --append-claim` 后,下一轮 `identity-dispatch --claim-ledger claims.ndjson` 会把续租记录当作 active lease 跳过,避免长任务被其它 worker 重领。

`identity-dispatch-complete CLAIMS` 会读取 `identity-dispatch --claim-out` 的 JSON/NDJSON claim 流水,把本轮 worker 执行结果写成 completion ledger。`--status` 支持 `succeeded|failed|retry|cancelled`;`failed` 默认是终态,需要重新派发时传 `--retryable`,或者直接用 `--status retry`。可用 `--worker`、`--claim-id`、重复的 `--dedupe-key` 精确选择要完成的 claim item;`--message` 和 `--result-json` 会写进每条 completion item,例如保存隔离后的 profile 目录、代理冷却原因或修复服务返回值。传 `--complete-out completed.json` 会覆盖写完整 completion report;加 `--append-complete` 会逐行追加 NDJSON completion item,供下一轮 `identity-dispatch --completion-ledger` 做去重、终态跳过和 retry 重派。

`identity-dispatch-reconcile ASSET_MANIFEST` 会把 claim / completion 流水对账回 profile asset manifest。传 `--claim-ledger claims.ndjson` 时,未过期租约会把匹配资产标为 `dispatchState=leased`;传 `--completion-ledger completed.ndjson` 时,最新 completion 会把资产标为 `succeeded|failed|retry|cancelled`,并写入 `lastDispatch*` 审计字段。匹配口径和 `identity-plan --asset-manifest-out` 一致:按 `accountId`、`profileId`、`identityId`、`label` 或 `profileDir` 命中资产。`succeeded` 且 dispatch / result 带 `nextState`、`state` 或 `profileDir` 时会同步更新 manifest 的 `state` / `profileDir`;传 `--asset-manifest-out runtime-profile-assets.json` 会写出下一轮调度可直接读取的运行态资产池。

`identity-assets-validate ASSET_MANIFEST` 是 profile asset manifest 的协议校验器。它只读 manifest,检查 entry 是否为对象、是否至少有一个稳定匹配键(`accountId` / `profileId` / `identityId` / `label` / `profileDir`)、关键匹配键是否重复、时间戳字段是否为 Unix seconds、`state` / `dispatchState` / `runtimeLeaseState` 是否在已知集合内,并提示缺 profileDir、过期 runtime/dispatch lease、到期 cooldown 等运行态残留。输出包含 `valid`、`errorCount`、`warningCount`、`issueCodeCounts`、`issues[]`、状态分布和可选 `manifestVersion`;传 `--validate-out asset-validate.json` 会落盘完整报告。默认即使 `valid=false` 也正常输出报告;加 `--strict` 时只要存在 error 就在打印报告后退出码 `2`,适合放在调度/发布前做硬门禁。

`identity-assets-status ASSET_MANIFEST` 是只读的账号/Profile 容量看板。它复用 `identity-assets-select` 的准入规则,默认只把 `state=active`、有 profile 目录、没有 active dispatch/runtime lease、没有 retry/cooldown/failed/cancelled 阻塞的资产计为 `runnableCount`;可用 `--allow-state repair` 或 `--include-dispatch-leased`、`--include-runtime-leased`、`--include-retry`、`--include-failed`、`--include-cancelled`、`--include-missing-profile-dir` 模拟放宽门禁。输出包含 `assetCount`、`runnableCount`、`blockedCount`、`capacityStatus`、`capacityShortageCount`、`recommendedLimit`、`blockReasonCounts`、状态分布、active/expired runtime lease、active/expired dispatch lease、active/expired cooldown、retry waiting/ready 统计和 `recommendations[]`。传 `--desired-concurrency 5` 时会直接告诉你当前账号池是否足够支撑期望并发;传 `--status-out asset-status.json` 可落盘给监控或调度器读取。

`identity-assets-forecast ASSET_MANIFEST` 是只读的账号/Profile 容量恢复预测。它复用 `identity-assets-select` 的准入规则,但会把阻塞分成“可预测恢复”和“硬阻塞”:`cooldown_active`、`runtime_lease_active`、`dispatch_lease_active`、`dispatch_retry_waiting` 且带明确到期时间时会进入 `recoveryEvents[]`,其它如 `missing_profile_dir`、`state_not_allowed:*`、`dispatch_failed` 会进入 `hardBlockedAssets[]`。输出包含 `currentRunnableCount`、`recoverableCount`、`recoverableWithinHorizonCount`、`predictedRunnableCount`、`currentShortageCount`、`predictedShortageCount`、`nextRecoveryAtUnixSeconds` 和达到 `--desired-concurrency` 的 `enoughAtUnixSeconds`;传 `--horizon-seconds 3600` 可只把一小时内恢复的资产计入预测容量。它适合在 status 报 shortage 后判断“等多久能恢复、还是需要扩容/降并发/健康治理”。

`identity-assets-gate ASSET_MANIFEST` 是业务启动前的硬门禁。它内部复用 `identity-assets-forecast`,必须传 `--desired-concurrency`;当前 runnable 已满足目标时输出 `decision=run_now` 且退出码 `0`。如果当前不足但能在 `--max-wait-seconds` 内恢复,输出 `decision=wait`、`recommendedAction=sleep_until_enough_at` 和 `enoughAtUnixSeconds`;默认这仍然退出码 `2`,避免误启动业务,只有显式传 `--allow-wait` 才把 wait 当作通过。预测窗口内仍不够时输出 `decision=insufficient`、`recommendedAction=reduce_concurrency_or_add_assets`,退出码 `2`。`--gate-out asset-gate.json` 会落盘完整门禁报告,其中嵌入 forecast 详情,适合调度器直接消费。

`identity-assets-select ASSET_MANIFEST` 是业务自动化启动前的 profile 准入器。默认只选择 `state=active` 且有 `profileDir/profilePath/userDataDir` 的资产,并自动跳过 active dispatch lease、runtime lease、retry 冷却、failed/cancelled 和显式冷却中的资产。可用 `--allow-state repair` 放宽状态,或用 `--include-dispatch-leased`、`--include-runtime-leased`、`--include-retry`、`--include-failed`、`--include-cancelled` 覆盖默认门禁。传 `--asset-manifest-out leased-profile-assets.json` 会把选中的资产写入 `runtimeLeaseState=leased`、`runtimeLeaseId`、`runtimeLeaseWorkerId`、`runtimeLeaseJobId`、`runtimeLeaseExpiresUnixSeconds`,让并发 worker 下一轮选择时避开这些 profile;传 `--selection-out selection.json` 会落盘本轮 selected/blocked 详情和 block reason 统计。

`identity-assets-release ASSET_MANIFEST` 在业务自动化结束后释放 runtime lease,必须带至少一个筛选条件: `--worker`、`--job`、`--lease-id`、`--account-id`、`--profile-id`、`--identity-id` 或 `--label`。`--status` 支持 `succeeded|failed|cancelled`;释放时会把 `runtimeLeaseId/WorkerId/JobId/Expires` 移入 `lastRuntime*` 审计字段,把 `runtimeLeaseState` 改成 `released`,并清除活跃 lease 字段。失败或需要降速时可传 `--cooldown-seconds 600`,下一轮 `identity-assets-select` 会因为 `cooldownUntilUnixSeconds` 自动跳过;也可传 `--next-state repair` 把账号资产转入修复态。`--message` / `--result-json` 会写入 `lastRuntimeMessage` / `lastRuntimeResult`;`--asset-manifest-out` 写出释放后的资产池。`--release-out release.json` 默认覆盖写完整释放报告;加 `--release-out runtime-release.ndjson --append-release` 时会把 `releasedAssets[]` 逐行追加成 NDJSON runtime release ledger,每行包含 `scope`、`assetManifest`、`generatedAtUnixSeconds`、`status`、`workerId`、`jobId`、`cooldownUntilUnixSeconds`、`nextState`、`message`、`result` 和 `item`,适合长期审计业务成功、失败、冷却和状态变更。

`identity-assets-reconcile-runtime ASSET_MANIFEST` 会把一个或多个 runtime release ledger 回放到中心 profile asset manifest。它支持读取 `identity-assets-release --release-out release.json` 的完整 JSON 报告,也支持读取 `--append-release` 追加出来的 NDJSON 流水;匹配字段沿用 `accountId` / `profileId` / `identityId` / `label` / `profileDir`,并额外支持 `runtimeLeaseId` / `lastRuntimeLeaseId`。命中资产后会写入 `runtimeLeaseState=released`、`lastRuntimeStatus`、`lastRuntimeWorkerId`、`lastRuntimeJobId`、`lastRuntimeLeaseId`、`lastRuntimeMessage`、`lastRuntimeResult`、冷却时间和 `nextState`,并清掉活跃 `runtimeLease*` 字段。输出包含 `releaseEventCount`、`updatedAssetCount`、`unmatchedEventCount`、状态分布和 `updates[]`;传 `--asset-manifest-out reconciled-profile-assets.json` 可把多 worker 的执行结果合并成下一轮准入/看板/清理可直接读取的中心资产池。

`identity-assets-health ASSET_MANIFEST` 会读取一个或多个 runtime release ledger,给中心资产池里的每个账号/Profile 计算运行健康度。它按 `accountId` / `profileId` / `identityId` / `label` / `profileDir` / `runtimeLeaseId` 匹配 release 事件,统计 `eventCount`、成功/失败/取消次数、`consecutiveUnsuccessfulCount`、`failureRate`、`healthScore`、`healthState` 和 `recommendedAction`。默认 `--repair-threshold 3`、`--quarantine-threshold 5`:连续失败达到 repair 阈值时建议 `mark_repair`,达到 quarantine 阈值时建议 `mark_quarantine`;传 `--window-seconds` 可只看最近一段时间。也可把 `windowSeconds`、`repairThreshold`、`quarantineThreshold`、`cooldownSeconds` 放进 `identity-policy.json` 的 `health` 分组,再用 `--policy` 复用;命令行显式值覆盖 policy。只传 `--health-out asset-health.json` 时它是只读报告;再传 `--asset-manifest-out health-profile-assets.json` 时会把触发阈值的资产写成 `state=repair|quarantine`,记录 `lastRuntimeHealth*`、`runtimeConsecutiveUnsuccessfulCount`、`runtimeHealthScore`,并按 `--cooldown-seconds` 或 policy cooldown 写入 cooldown。它适合把“某账号连续失败太多就降速/隔离”的经验从业务脚本里抽成中心治理策略。

`identity-assets-sweep ASSET_MANIFEST` 用来清理运行态资产池里的过期残留。它会把已过期的 `runtimeLeaseState=leased` 归档成 `runtimeLeaseState=expired`,清掉活跃 `runtimeLeaseId/WorkerId/JobId/Expires`,并写 `lastRuntimeExpiredAtUnixSeconds`;把已过期的 `dispatchState=leased` 标成 `dispatchState=expired`;把到期的 `cooldownUntilUnixSeconds` / `nextAvailableUnixSeconds` 清掉并写 `lastCooldownClearedAtUnixSeconds`。`--runtime-grace-seconds`、`--dispatch-grace-seconds`、`--cooldown-grace-seconds` 可给过期判断加宽限期。传 `--asset-manifest-out clean-profile-assets.json` 写出清理后的资产池,`--sweep-out sweep.json` 写清理动作报告。

`identity-job run ASSET_MANIFEST -- COMMAND...` 是给既有 Python/Node/shell 业务脚本用的运行时 sidecar。它会按顺序执行 `identity-assets-sweep`、`identity-assets-validate`、runtime risk 预启动门禁、`identity-assets-gate`、`identity-assets-select`,只有门禁通过并成功写入 runtime lease 后才运行 `--` 后面的业务命令;命令结束后自动 `identity-assets-release`。默认 `--limit 1`,未传 `--desired-concurrency` 时用 limit 作为启动门禁目标;未传 `--worker` 时生成 `identity-job-<pid>`,未传 `--job` 时使用 `identity-job`。传 `--job-preset publish_conservative|login_sensitive|scrape_aggressive` 可直接套用内置业务风险模型;传 `--policy identity-policy.json` 可从 `job` 分组读取 `preset` / `jobPreset`、`desiredConcurrency`、`limit`、`leaseSeconds`、`maxWaitSeconds`、`allowWait`、`perAsset`、`childConcurrency`、`runtimeRenewIntervalSeconds`、`childTimeoutSeconds`、`childResultDir`、`maxFailedAssets`、`maxFailedAssetsPerReason`、`allowState`、`failureCooldownSeconds`、`failureNextState`、`failureReasonRules`、`runtimeRiskLedgers`、`runtimeRiskWindowSeconds`、`runtimeRiskOut`、`appendRuntimeRisk`、`explainOut` 等默认值,命令行显式参数覆盖 policy。传 `--asset-manifest-out` 会把租约和释放结果写到工作 manifest;不传则就地更新输入 manifest。子进程会收到 `DRS_IDENTITY_JOB_RUN_ID`、`DRS_IDENTITY_ASSET_MANIFEST`、`DRS_IDENTITY_SELECTED_COUNT`、`DRS_IDENTITY_SELECTED_ASSETS_JSON`、`DRS_IDENTITY_WORKER`、`DRS_IDENTITY_JOB` 等环境变量,业务脚本可直接读取已领取的 profile/account。加 `--per-asset` 时,选中几个资产就为每个资产启动一次同一命令;每次只注入一个资产,并额外提供 `DRS_IDENTITY_ASSET_JSON`、`DRS_IDENTITY_LABEL`、`DRS_IDENTITY_PROFILE_DIR`、`DRS_IDENTITY_ACCOUNT_ID`、`DRS_IDENTITY_PROFILE_ID`、`DRS_IDENTITY_RUNTIME_LEASE_ID` 等单资产环境变量。默认一次跑一个子进程,加 `--child-concurrency N` 或 policy `job.childConcurrency` 后会按批次并发拉起 N 个旧脚本实例,但 release 仍按账号逐条写回。长任务可加 `--runtime-renew-interval-seconds N` 或 policy `job.runtimeRenewIntervalSeconds`:只要子进程仍在运行,drission-rs 会周期性刷新本轮 selected lease 的 `runtimeLeaseExpiresUnixSeconds`,并在 job report 的 `leaseRenewal` 里记录 tick、续租数量和错误。卡死任务可加 `--child-timeout-seconds N` 或 policy `job.childTimeoutSeconds`:超时后 child report 会记录 `timedOut=true`,本轮 release 按失败处理,可继续配合 `--failure-cooldown-seconds` / `--failure-next-state repair` 做账号冷却和修复态。需要业务脚本细分判责时加 `--child-result-dir DIR` 或 policy `job.childResultDir`:drission-rs 会给子进程注入 `DRS_IDENTITY_RESULT_OUT` / `DRS_IDENTITY_CHILD_RESULT_OUT`,普通模式文件名为 `child-result.json`,per-asset 模式为 `child-<index>.json`;脚本可写 `{"status":"failed","message":"rate limited","reason":"rate_limited"}`,其中 `status` 支持 `succeeded|failed|cancelled`,`reason` 或 `result.reason` 会进入 `failureReasonCounts`;若脚本显式写 `cooldownSeconds` / `nextState` 会覆盖 release 决策,否则 sidecar 会按 policy `job.failureReasonRules.<reason>.cooldownSeconds/nextState` 自动补冷却和状态迁移,并写进 `lastRuntimeResult.failureReasonRuleApplied`;同一规则还可写 `recommendedAction`、`runtimeRiskSeverity`、`nextSuggestedLimit`、`nextSuggestedDesiredConcurrency`、`runtimeRiskMessage`、`runtimeRiskCooldownSeconds`,让单个业务原因直接覆盖本轮 `runtimeRisk` 建议、生成 `suppressUntilUnixSeconds`,并进入后续 `runtime-risk-out` / `runtime-risk-ledger` 闭环。一个账号失败只会把自己的资产写成 `failed` / 冷却 / repair;使用 `--max-failed-assets N` 或 policy `job.maxFailedAssets` 时,per-asset 模式会在失败数达到阈值后停止启动剩余账号;使用 `--max-failed-assets-per-reason N` 或 policy `job.maxFailedAssetsPerReason` 时,同一失败原因达到阈值后也会停掉剩余账号。两种熔断都会把已领取但未执行的资产 release 为 `cancelled`,并在 child/release report 中记录 `skippedCount`、`cancelledCount`、`failureReasonCounts` 和 `circuitBreaker`。job report 顶层 `runtimeRisk` 是给调度器直接消费的下一轮建议:字段包括 `severity`、`recommendedAction`、`nextSuggestedLimit`、`nextSuggestedDesiredConcurrency`、`failureRatePermille`、`failureReasonCounts`、`dominantFailureReason` 和 `circuitBreaker*`;`recommendedAction` 可能是 `continue_current`、`reduce_concurrency`、`pause_pool`、`pause_failure_reason` 或前置阶段的修复/等待建议。加 `--runtime-risk-out runtime-risk.ndjson --append-runtime-risk` 或 policy `job.runtimeRiskOut/job.appendRuntimeRisk` 后,同一建议会写成顶层可查询的 `identity_job_runtime_risk_event` NDJSON 事件,方便多 worker 调度器跨轮聚合和回放。下一轮加 `--runtime-risk-ledger runtime-risk.ndjson --runtime-risk-window-seconds 900` 或 policy `job.runtimeRiskLedgers/job.runtimeRiskWindowSeconds` 后,sidecar 会在领取 profile 前读取最近同 job 风险事件:带 `suppressUntilUnixSeconds` 的事件按精确截止时间生效,最新建议是 `pause_pool` / `pause_failure_reason` 时直接以 `phase=runtime_risk_gate`、退出码 `2` 停止,不会写 runtime lease;最新建议是 `reduce_concurrency` 时会把本轮 `limit` / `desiredConcurrency` 降到 `nextSuggestedLimit` / `nextSuggestedDesiredConcurrency` 后再 gate/select。每次 job report 都带 `explain`:其中 `stageDecisions[]` 解释 sweep/validate/runtime-risk-gate/gate/select/child/release/runtimeRisk 每一阶段为何通过、阻断或调整,`assetDecisions[]` 解释每个账号/Profile 为何 selected、blocked、child failed/succeeded 或 released;加 `--explain-out job-explain.json` 或 policy `job.explainOut` 可把这份解释单独落盘给调度器和人复盘。使用 `--release-out` 时需配合 `--append-release` 形成多行 runtime release ledger。子进程退出 0 会释放为 `succeeded`;非 0、超时或启动失败会释放为 `failed`,并可用 `--failure-cooldown-seconds 600 --failure-next-state repair` 自动把失败资产冷却或转修复态。`--release-out runtime-release.ndjson --append-release` 会保留 runtime release ledger,`--runtime-risk-out runtime-risk.ndjson --append-runtime-risk` 会保留 runtime risk ledger,`--job-out job.json` 会落盘完整 wrapper 报告,CLI 自身退出码跟随门禁/选择/子进程结果。

Python 业务脚本可用仓库内的零依赖 helper [`python/drission_sidecar`](../python/README.md) 接入结果协议:在启动命令前设置 `PYTHONPATH=/path/to/drission-rs/python`,脚本里读取 `asset()` / `profile_dir()`,成功时调用 `succeeded("published", result={...})`,限流或风控时调用 `failed("rate_limited", cooldown_seconds=900, next_state="repair")`。helper 只写 `DRS_IDENTITY_RESULT_OUT`,不会把浏览器自动化 API 搬回 Python,因此治理决策仍由 Rust sidecar 和 policy 统一执行。

`identity-ledger query` 用于运行后复盘和下一轮调度决策。它可同时读取 `identity-job run` / `identity-assets-release` 追加出的 runtime release ledger 和 runtime risk ledger,并按 `--window-seconds`、`--job`、`--worker`、`--reason` 过滤。输出包含 `releaseStatusCounts`、`failureReasonCounts`、`topAssets`、`runtimeRiskActionCounts`、`runtimeRiskSeverityCounts`、`activeSuppressions` 和 `recommendations[]`;带 `suppressUntilUnixSeconds` 且尚未到期的 risk event 即使超出普通窗口也会保留,方便回答"为什么下一轮还不能跑"。

`identity-ledger explain` 用于回答单个账号/Profile/失败原因为什么现在不能跑。它复用同一组 release/risk ledger,额外支持 `--account-id`、`--profile-id`、`--identity-id`、`--label`、`--profile-dir`、`--lease-id` 和 `--evidence-limit`。输出包含 `decision`、`blockingReasons[]`、`blockedByActiveSuppression`、`blockedByCooldown`、`nextRunnableUnixSeconds`、`activeSuppressions`、`activeCooldowns`、`releaseEvidence[]`、`runtimeRiskEvidence[]` 和 `recommendations[]`;调度器可以直接把 `decision=blocked_by_runtime_risk_suppression|blocked_by_asset_cooldown` 当作下一轮阻断依据。

`identity-ledger compact` 用于把长期 NDJSON 压成一个调度器可快速读取的 JSON 摘要。它支持 `--window-seconds`、`--job`、`--worker`、`--reason`、`--retain-recent` 和 `--top`;输出包含 `compactedThroughUnixSeconds`、`sourceEventCount`、`compactedEventCount`、`assetSummaries[]`、`activeSuppressions`、`nextSuppressionUntilUnixSeconds`、`retainedReleaseEvidence[]`、`retainedRuntimeRiskEvidence[]` 和 `recommendations[]`。有 `suppressUntilUnixSeconds` 且仍生效的 risk event 会被保留,即使它早于普通窗口;旧原始账本可以在确认 compact 文件进入调度链路后再冷归档。加 `--checkpoint-out ledger-checkpoint.json` 会写出每个源 ledger 的 byte offset 和 active suppression;下一轮传 `--checkpoint-in ledger-checkpoint.json` 时只读取新增 tail,如果源文件被截断/轮转则自动从 0 重读并在 `sourceCheckpoints[].reset=true` 标记。

`identity-ledger dashboard` 是 compact 的人读版本:同样读取 release/risk ledger,先生成 `identity_ledger_compact`,再输出 `identity_ledger_dashboard` JSON;加 `--html-out ledger-dashboard.html` 会写单文件 HTML。dashboard 顶层 `summary` 包含 `status`、`recommendedAction`、`failureRatePermille`、`activeSuppressionCount`、`topFailureReason`、`topRuntimeRiskAction` 和 `compactedThroughUnixSeconds`;HTML 会展示 active suppression、top assets、失败原因排行、risk action 和最近 release/risk 证据。它也支持 `--checkpoint-in/--checkpoint-out`,适合监控页定时刷新时只读新增 ledger tail。

`profile-map.json` 可以是简单对象:

```json
{
  "acct-a": "/data/profiles/acct-a",
  "fp_1234": "relative-profile-dir"
}
```

也可以是数组:

```json
[
  { "accountId": "acct-a", "identityId": "fp_1234", "profileDir": "/data/profiles/acct-a" }
]
```

## MCP 模式

启动 stdio MCP server:

```bash
drs mcp --backend cdp --headless
```

默认行为:MCP 启动时会 `ensure-serve` 拉起(或复用)常驻 daemon,并把所有 `browser_*` / `network_*` 工具调用转发到该 daemon。浏览器活在长命的 `drs serve` 进程里,因此:

- CLI 里 `drs open` / 登录后的标签,AI 通过 MCP 能直接接着用;反之亦然。
- Cursor 重启 MCP server(改配置、重开窗口、会话结束)不会杀掉浏览器,登录态和已开标签仍在。
- daemon 用固定 profile,即使 daemon 本身重启,Chrome 复用同一 profile,cookie/登录不丢。

想让 MCP 在自己进程内单独持有浏览器(一次性、不常驻场景),加 `--standalone`:

```bash
drs mcp --headless --standalone
```

`identity_assets_*` 等纯文件治理工具不需要浏览器,始终在 MCP 进程内直接执行,与是否 attach daemon 无关。

稳定工具名如下(客户端不要依赖 `tools/list` 的返回顺序):

- `browser_open`
- `browser_tabs`
- `browser_use_tab`
- `browser_close`
- `browser_ax`
- `browser_html`
- `browser_title`
- `browser_url`
- `browser_extract`
- `browser_text`
- `browser_eval`
- `browser_click`
- `browser_type`
- `browser_wait`
- `browser_screenshot`
- `network_listen_start`
- `network_listen_wait`
- `network_listen_stop`
- `browser_identity`
- `browser_identity_pool`
- `browser_pass_cf`
- `identity_assets_validate`
- `identity_assets_status`
- `identity_assets_forecast`
- `identity_assets_gate`
- `identity_assets_select`
- `identity_assets_release`
- `identity_assets_reconcile_runtime`
- `identity_assets_health`
- `identity_assets_sweep`

`browser_screenshot` 默认保存 PNG 并返回路径;传 `inline=true` 时同时返回 base64 与 MCP image content。
`browser_extract` 打开 URL(或复用当前标签)并返回 `title` / `url` / `text` / `outline`;可选 `include_html`、`include_ax_json`、`pass_cf`、`wait_selector`、`screenshot_out`。
`browser_identity` / `browser_identity_pool` 接受可选 `gate_preset`、`min_score`、`max_linkability`、`max_concentration_ratio`、`max_concentrated_signals`、`fail_on_high_risk`、`fail_on_risky_pairs`,并在结构化结果里返回同样的 `gate` 对象。
`identity_assets_*` 工具对齐同名 CLI 命令的 kebab-case 版本,但 MCP 参数使用 snake_case,例如 `identity_assets_gate` 传 `asset_manifest`、`desired_concurrency`、`max_wait_seconds`;`identity_assets_select` 可写 `asset_manifest_out` 来预留 `runtimeLease*`;`identity_assets_release` 的 `result_json` 可直接传 JSON 值。它们让 Agent 在启动业务自动化前完成容量门禁、恢复预测、profile 领取,并在业务结束后回写 release ledger、健康分和过期残留清理。

## 一键接入 Cursor / Codex(`drs setup`)

`drs setup` 自动把 `drs` 写成 Cursor 和 Codex 的 MCP server,免得手写 JSON/TOML:

```bash
drs setup                       # 默认两家都配:Cursor 项目 .cursor/mcp.json + Codex ~/.codex/config.toml
drs setup --target cursor       # 只配 Cursor
drs setup --target codex        # 只配 Codex
drs setup --scope global        # Cursor 写全局 ~/.cursor/mcp.json(默认 project)
drs setup --dir /path/to/proj   # 指定 Cursor 项目根(默认当前目录)
drs setup --no-headless         # 生成的 server 命令去掉 --headless(想看有头浏览器)
drs --json setup --dry-run      # 只打印计划、不落盘
```

行为要点:

- `command` 写当前 `drs` 可执行文件的**绝对路径**(`std::env::current_exe()`),避免编辑器起 MCP 时 `PATH` 找不到。
- Cursor 写 `.cursor/mcp.json`,**合并**已有 `mcpServers`(不覆盖别的 server);Codex 写 `[mcp_servers.drs]` 表,保留 `config.toml` 里其它表,重复运行是幂等 upsert。
- 生成的 server 命令是 `drs mcp --backend cdp --headless`,即默认 attach 常驻持久浏览器(见 [`mcp-持久浏览器.md`](mcp-持久浏览器.md))。
- `--name` 可改 server 名(默认 `drs`)。

配好后重启 Cursor / Codex 即生效。给 AI 的话术:先 `curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh` 装 CLI,再 `drs setup`,之后抓「难获取」的页面(登录态/反爬/Cloudflare/动态渲染)一律走 `drs` 的 `browser_*` / `network_*` 工具。

## OCR

开启 `ocr` feature 后可使用纯图片点选求解:

```bash
drs --json ocr clickword ./captcha.png 税实企
```

输出包含按目标顺序的 `points` 和每个命中的 `bbox` / `affinity` / `template`。

## 常见错误

| 错误码 | 含义 | 处理 |
|---|---|---|
| `daemon_not_running` | 没找到可用 daemon | 先运行 `drs serve --headless` |
| `daemon_unreachable` | state 文件存在但端口连不上 | 重启 `drs serve`;CLI 会移除 stale state |
| `unauthorized` | token 不匹配 | 删除缓存中的 `drs-server.json` 或重启 daemon |
| `command_failed` | 浏览器动作失败 | 查看 `message`;常见是 selector 未命中或页面超时 |
| `Session with given id not found` | active tab 对应的浏览器 target 已关闭,常见于打开会触发下载的 URL | 用 `drs open` 打开 HTML 页面;下载型资源用网络监听或 HTTP 客户端处理 |

## 设计边界

MCP 聚焦 AI 可调用的浏览器运行时和账号/Profile 治理控制面,不直接承诺完整批量爬虫 UI、HAR 回放、recorder、dump_env、代理池 UI、滑块全流程命令。这些能力仍可通过 Rust API 或 CLI 使用,后续可逐步接入 MCP。
