<!-- 感谢贡献!提交前请确认下面的检查项。 -->

## 动机 / 改动

<!-- 这个 PR 解决什么问题、做了什么。涉及行为变化请说明。 -->

## 关联 issue

<!-- 如 Closes #123 -->

## 自检清单

- [ ] `cargo fmt --all` 已格式化
- [ ] `cargo clippy --all-targets --features slider,cdp -- -D warnings` 零警告
- [ ] `cargo test --features slider,cdp --lib --tests` 通过
- [ ] 非测试代码无 `unwrap/expect/panic`,错误走 `Result`
- [ ] 新增对外类型已在 `prelude` 导出(如适用)
- [ ] 已更新 `CHANGELOG.md` 的 `[Unreleased]`(如有行为变化)
- [ ] 涉及新能力时,补了**离线** example 或测试

## 验证方式

<!-- 如何验证这次改动?example 名 / 测试名 / 实机结果。 -->
