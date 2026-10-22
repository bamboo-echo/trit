# 治理（Governance）

## 模型：BDFL

本项目采用 **BDFL（Benevolent Dictator For Life）** 模式：技术上锁死方向，制度上保留最终拍板权。

- **BDFL**：[@bfxh](https://github.com/bfxh)
- 最终技术方向、架构变更、版本发布、代码审查权限归 BDFL。
- 日常小修小补（文档、测试、明确的 bug 修复）可以放权给贡献者；合并前仍需 CI 通过，
  关键路径仍需 CODEOWNERS 指定的审查。
- 架构变更（公开 API、模块边界、依赖、发布流程、治理文件本身）必须先经 BDFL 在 issue/PR 中
  书面明确批准，不能以“CI 绿了”代替设计批准。
- 版本发布、撤回、yank 只能由 BDFL 批准并执行；任何人不得自行打 tag 或发布 crate/二进制。
- 安全问题的处理与披露方式由 BDFL 决定。

## 决策与异议

1. **小改动**：issue/PR 说明动机与影响，CI 绿 + 审查通过即可合并。
2. **架构/契约变更**：先写提案（问题、方案、替代方案、迁移与回滚），BDFL 明确同意后再实现。
3. **分歧**：BDFL 保留最终拍板权；技术理由必须记录在 PR/issue 里，便于后人复盘。
4. 本文件的修改需 BDFL 批准。

## 版本与发布

- 使用 `major.minor.patch` 形式的版本号；当前处于 **0.x** 阶段，minor 变更可能包含破坏性调整。
- 主版本号（1.0 及以后）的启用时间由 BDFL 在项目足够成熟后决定；0.x 阶段不急于进入 1.0。
- 发布流程：更新 workspace 版本与 CHANGELOG → 全部门禁（fmt/clippy/test/rustdoc/wasm/变异门）→
  打 tag → 触发 Release → 需要时发布 crates.io。
- 版本发布必须由 BDFL 执行或书面授权；回滚与撤回同样由 BDFL 决定。

## 审查与 CODEOWNERS

- `.github/CODEOWNERS` 把源码、构建、CI、脚本、fuzz、Action 与治理文件指定给 BDFL。
- 关键路径的修改必须有 BDFL 的 approving review 才能合入；是否在分支保护里强制
  “require code owner reviews” 由 BDFL 在仓库设置中决定。
- 非关键路径（示例、普通文档）可由贡献者合并，但仍需 CI。

## 继承

BDFL 可书面指定继任者或临时代理人；在此之前，最终权限只属于 BDFL。
