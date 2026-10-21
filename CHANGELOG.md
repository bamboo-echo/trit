# Changelog

## [0.3.0] - 2026-10-22

### Added

- `trit-core`：`TriVec` 位切片三值向量与 `TriBit::from_binary` / `as_binary`；二进制是
  `unknowns` 缺失的零拷贝视图（1M 元素打包 0.65 ms、K3 AND 0.033 ms）。
- `trit-sat` 二值快速路径：`--binary` / `--binary-all`，纯二值子图走经典 CNF
  （100 变量链子句 3866→598，0.028→0.0147 s）；`--stats` 打印编码规模。
- 充分性反例最小化：`minimal_counterexample` / `--minimize`，对偶 `a==b` 检查保证未列出的变量任取仍区分。
- `trit-bdd`：canonical BDD 等价引擎与 CLI `--bdd`；`--stats` 打印差集节点数。
- `trit-policy` + `trit verify`：JSON 决策表、定义展开、全合法赋值断言检查；失败退出码 1 并给最小反例。
- `action/action.yml`：可复用的 `trit verify` GitHub Action。

### Changed

- 仓库由 `ternary-logic-analyzer` 更名 `trit`，拆成 6 个 crate 的 workspace。
- `EquivalenceResult` 新增 `counterexample_assignment`；编号超出 `u64` 时 `counterexample=None`
  （100 变量不再溢出）。

### Fixed

- Release 工作流打包的二进制名由 `tla` 修正为 `trit`。
