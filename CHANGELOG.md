# Changelog

## [未发布]

### Changed（评审修复第二批，含 API 变更）

- 解析器在构造期扣预算：右结合蕴含链改为迭代收集再右折，扁平二元链按实际 AST 深度拒绝；
  新增输入字节上限（1 MiB）与节点预算错误；`Parser::parse_with_limits` 可指定预算。
- `Evaluator::assignment_count` / `chunk_planes` / `evaluate` 改为可失败（`Result`），
  `3^n` 用 checked 计算，新增 `Limits::max_rows` 与 `evaluate_within`。
- `trit_eval::{truth_table, equivalent}`、`trit_belnap::{table, equivalent}`、
  `Circuit::combination_count` 改为返回 `Result`；`analyze` 在 `3^n` 循环前拒绝 >16 个主输入。
- `TriVec` 的 `PartialEq/Eq` 改为按值比较（全零 unknown 平面等同 `None`）；写入清除最后一个
  `Unknown` 时规范化平面；新增 `try_from_binary` / `try_get` / `try_set` / `try_word`。
- `BitPlane` 的 `with_bit` / `bit` 明确为环绕语义，新增 `checked_with_bit` / `checked_bit` /
  `TriBit::checked_get`。
- `Ternary::{is_false,is_true}` 文档改为精确匹配（`X` 两者皆假）；新增 `Expr::try_var` 拒绝
  保留常量 token/非法标识符，`Display` 不再暗示任意变量可无损往返。
- 赋值编号：新增 `try_assignment_index` / `try_decode_assignment`（重复变量、超出 `3^n` 域、
  u64 溢出可区分）；编码不再因末位多余的 `3^n` 计算而过度拒绝。
- 策略：展开时按节点/深度预算（`ExpandBudget` / `DefinitionDepth`），完整定义图（含未引用环）
  先预检；`binary` 只允许原始变量，定义名拒绝保留 token，重复 binary 报错；
  上层错误携带 `LimitError` / `SymbolicError` / `BddError`，不再压成字符串。
- BDD 预算明确为软限额（每次运算返回后检查，峰值可超）；新增 `Quantified::try_is_valid` /
  `try_counterexample`；README 说明硬隔离需进程/worker。
- 策略反例文档改为“完整赋值模型，不做最小化”。

### Fixed

- 评审修复（第一批）：Verilog 括号倒置不再 panic；多输入 NAND/NOR 改为“先合并再反相”；
  XNOR 不再双驱动；临时信号名全模块唯一；强加 `Xor` 导出 Verilog 明确拒绝；
  `check` 无法执行覆盖率检查且用户指定阈值时 FAIL；单元库键规范化并拒绝重复；
  时序/映射累计改用 u64 检查，溢出返回错误；ATPG X 位改为联合填充验证；
  `minimum_cover` 预算耗尽不再宣称最优；`decode_binary` 超过 64 输入返回错误。

### Added

- ATPG X 合并压缩：两个向量若存在一种填充能覆盖“删掉它们后会失去”的唯一故障，就合成一个；
  `inputs` 仍给出具体 0/1 填充，CSV 导出即用该填充。
- `AtpgReport::x_coverage`（含 X 向量覆盖的可测故障比例）与 `merged`（压缩省下的向量数）；
  表格、JSON、WASM 同步输出。
- 复审测试缺口（§3.3）：`trit-core/tests/structure.rs` 用 proptest 生成不同形状的 AST，
  验证 `to_infix → parse` 的结构/深度/节点数守恒、扁平链与右结合蕴含链的深度边界、
  任意文本不 panic；`trit-policy` 新增展开预算精确性/深度单测与稳定 smoke。
- fuzz 目标补到 8 个：新增 `expr`（解析往返）、`policy`（小策略验证）、`netlist_json`
  （JSON 往返 + 坏电路导出）；`fuzz/Cargo.toml` 的 `trit-circuit` 依赖路径修正为
  `../crates/trit-circuit`。
- ATPG 端到端回归：用 `generate_tests` 实际产出的覆盖向量验证 don't-care 的每个联合填充
  仍检出故障，补上报告 §3.1 指出的“只证明 helper”缺口。
- AIGER ASCII 子集读写：`parse_aiger` / `to_aiger`（`L = 0` 组合逻辑，AND + 取反编码）；
  CLI `netlist --from/--to aiger`；0/1 激励上的往返等价。
- BLIF 子集读写：`parse_blif` / `to_blif`（`.model/.inputs/.outputs/.names`，拒绝 `.latch`）；
  CLI `netlist --from blif` / `--to blif`；往返等价在 0/1 激励上验证。
- Verilog `assign` 支持 `^` / `~^` / `^~`、括号与常量字面量，按 Verilog 优先级解析；
  `^` 映射为经典异或（`XorClassic`），不是强加三值 `Xor`。
- 负载感知时序：`TimingModel.load_delay`（每个扇出追加延迟）进入 `analyze_timing` / `critical_paths`；
  `to_cells_with_load` 在选实现时按扇出计负载；CLI `timing` / `check` / `map --load-delay`。
- fuzz：`fuzz/` 下 cargo-fuzz 目标（netlist / Verilog / ATPG）+ CI 可跑的稳定 smoke；
  `scripts/bench.sh` 与 `trit-circuit` 的 `core` bench（解析/时序/映射/ATPG）。

## [0.8.0] - 2026-10-22

芯片侧能力的一批新入口：结构化 Verilog、STA-lite、技术映射（NAND-only 与时序驱动）、
设计检查流水线、X-aware ATPG；三值语义不变。

### Added

- 时序驱动技术映射：`CellLibrary` + `to_cells`，每门在直接单元与 NAND 展开间按到达时间选实现
  （最长路径 DP，关键路径最优）；`MapStrategy::Area` 优先单元数。CLI `map --cells` / `--area-first`。
- `trit atpg --format csv`：`pattern,<输入…>,covered`，便于导出给外部仿真器。
- `trit_circuit::netlist`：`parse_netlist` / `to_netlist` 从 CLI 下沉到库（可往返）。
- WASM `atpg_js(netlist, algebra)`：浏览器里直接跑 ATPG，返回 JSON。
- `satisfiable_bdd_with_order` / `premise_counterexample_bdd_with_order`，策略验证 `--bdd-order`。
- 网表 JSON 互转：`trit_circuit::netlist::{to_json, from_json}` 与 CLI `trit netlist`。
- `analyze_faults`：ATPG 不再做 `3^n` 输出域枚举；全向量搜索收紧到 ≤8 个主输入。
- 索引化仿真器（`Simulator`）：故障分析/覆盖矩阵不再逐向量建 `BTreeMap`。
- 结构化 Verilog 子集：`parse_verilog` / `to_verilog`，CLI `netlist --from verilog --to verilog`。
- 新增 `GateKind::XorClassic`（真实布尔异或，X 传播）：网表 `XOR2`，Verilog `xor` 原语，NAND 四门映射。
- `trit map`：降到 NAND + 常量；三值强加 `Xor` 因缺 X 检测单元明确拒绝。
- Verilog `assign` 里的 `^` 仍拒绝（请用 `xor` 原语）。
- `trit check`：结构 + 时序 + X 传播 + ATPG 覆盖率，`--clock`/`--min-coverage` 失败即退出 1。
- ATPG X don't-care：每个向量标出可任意填 0/1 的输入位（`dont_care` / `dont_care_bits`）。
- STA-lite：`analyze_timing` / `critical_paths` 与 `trit timing`（required/slack、违例、top-N 路径）；
  `TimingModel` 支持按门类型延迟与 JSON 库。
- `circuit` / `atpg` 支持 `--from text|json|verilog`。
- `BddRequest`（顺序 + 预算）与 `*_with` 系列 API；CLI `equiv/quantify/verify --bdd-max-nodes`。
- `verify --bdd-order best`：四种排序取断言 BDD 节点总数最少者。
- ATPG 随机电路属性测试：每个向量必须真的检出它声称覆盖的故障。

## [0.7.0] - 2026-10-22

ATPG 与 BDD 的一批新能力；`Fault` 结构体有破坏性变更。

### Added

- `trit atpg <netlist>` 与 `trit_circuit::generate_tests`：门引脚 stuck-at 的测试向量生成
  （贪心集合覆盖）+ 覆盖率报告（table / JSON）。
- `AtpgReport::by_gate_kind`：覆盖率按门类型拆分；`exhaustive` 标记是否做了全 `2^n` 搜索；
  `optimal` 标记是否为精确最小覆盖（故障 ≤ 64、向量 ≤ 256 时用 DFS + 下界剪枝）。
- `Fault::witness`（首个二进制区分向量）、`Fault::reason`（`redundant` / `unobserved` / `masked`）
  与 `Circuit::decode_binary`；`trit circuit` 同步输出原因。
- `VarOrder`（appearance / reverse / frequency / frequency-asc）、`order_variables`、
  `equivalent_bdd_with_order`、`equivalent_bdd_best`、`quantify_bdd_with_order`、`quantify_bdd_best`；
  CLI `equiv --bdd-order` 与 `quantify --bdd-order`。
- `BddLimits`（默认 100 万节点/单个 BDD）与 `equivalent_bdd_with_limits`；超限返回
  `BddError::BudgetExceeded`，所有 BDD 构造点都做 O(1) 检查。

### Changed（破坏性）

- `Fault` 新增 `witness` 与 `reason` 字段：用结构体字面量构造 `Fault` 的代码需要补字段。
- stuck-at 可测性判定改用 `2^n` 二进制测试向量；`X` 输入不是物理测试向量，`analyze` 的
  `observable` 与 ATPG 口径一致。
- 主输入超过 16 时故障分析返回 `CircuitError::TooManyInputs`。

## [0.6.0] - 2026-10-22

### Added

- `trit-core::Limits` + `check_expr` / `check_vars`：库级统一预算（深度 / 节点 / 枚举变量数）。
- `trit-eval`：`eval_within` / `truth_table_within` / `equivalent_within`。
- SAT、BDD、策略入口先做深度/节点检查，超限返回 `Limit` 错误；CLI 与 WASM 共用同一套预算。
- `scripts/mutation_check.py`：12 条关键规则的变异门（12/12 被抓）；新增每周 CI。

### Fixed

- 变异门发现 `strong_and` / `strong_add` 的 K3 断言过弱（只测了 `X⊗X` 与 `1⊗1`），
  补上 `1⊗0 = 0`、`0⊗1 = 0` 的固定用例。

## [0.5.0] - 2026-10-22

### Changed（破坏性）

- `CircuitReport::untestable_faults` 替换为 `faults: Vec<Fault>`：逐**门引脚**、逐极性（SA-0/SA-1）。
  `trit circuit` 的 JSON 字段同步为 `faults`（含 `input_index` / `stuck_at` / `observable`）。

### Added

- `trit-core`：解析嵌套上限 256（`ParseError::TooDeep`）。
- CLI 入口规模护栏：`table` / REPL 最多 10 个变量，网表分析最多 10 个主输入。

### Fixed

- 故障注入从整条 net 改为 gate-pin：多扇出不再串扰，SA-0 / SA-1 分开报告。
- `TriVec::from_ternary` / `and_assign` / `or_assign` 的纯二值路径不再分配 Unknown 临时平面。
- GitHub Action 全部固定到 commit SHA（checkout / rust-toolchain / rust-cache / artifact / gh-release），
  MSRV 1.88 同样固定。
- 文档：DESIGN 路线补 v0.5；CONTRIBUTING 的 `tla-core` 路径改为 `trit-core` / `trit-eval`；
  `bulk_bench` 把缓冲区分配移出 move-only 计时；README 明确双导轨只是进程内表示，不是字节协议。

## [0.4.2] - 2026-10-22

### Fixed

- `trit-circuit`：`analyze` / `propagate` / 故障模拟改用真正的拓扑序（此前把全部门输出预置为已解析，
  等于按声明顺序求值）；主输出必须有驱动。
- `trit-eval`：修正三条不保真化简（`a | (a | b)`、K3 `a ⊕ a`、K3/B3 `a ≡ a`）；
  `semantically_equal` 改用真实 `3^n` 等价，不再比较变量顺序敏感的 64 位指纹。
- `trit-bdd`：`is_valid` 相对合法自由变量域判定；`satisfiable_bdd` 与合法域相交，不再把非法导轨 (1,1)
  当成见证；新增 `premise_counterexample_bdd`。
- `trit-policy`：`assumptions` 按“前提确定为 True”过滤（SAT/BDD 新增 `premise_counterexample*`），
  不再用三值 `premise ⇒ assertion` 恒真替代；schema 拒绝未知字段；空断言不再静默通过。
- `trit-core`：`TriVec::from_binary` 规范化多余字与尾部 padding；`TriBit` 的 B3 或不再产生禁止的 `(1,1)`。
- `trit-cli`：CSV/Markdown 每个变量独立成列（此前整行拼成一格、表头拆两行）；REPL `:algebra` 真正切换语义；
  `simplify` 若结果不等价则非零退出，不再 fail-open。
- `action`：`policy` / `engine` 经 env 传入并校验 engine 白名单，不再把输入拼进 shell 脚本。

## [0.4.1] - 2026-10-22

### Fixed

- `trit-sat` / `trit-bdd` 的二值快速路径把 `⊕` 编成经典异或，与解释器不一致：K3/B3 的 `⊕` 在 `0/1` 上是 `∧`，
  Ł3 上是 `∨`。常量（`1 ^ 1`）与 `--binary` 变量都会触发，例如 `trit equiv --symbolic "1 ^ 1" "1"` 误报不等价。
  SAT 新增 `binary_strong_add`，BDD 的二值分支按代数选 `∧`/`∨`，并加回归测试。
- 新增 `trit-sat` 属性测试：随机公式上 SAT 与 `3^n` 穷举逐例一致，且 SAT 给出的反例必须在解释器下区分两边。

## [0.4.0] - 2026-10-22

### Added

- `trit-wasm` + `playground/`：浏览器里跑求值、三值真值表、等价与 Belnap 四值（wasm-bindgen，`--target web`）。
- `trit-belnap`：Belnap 四值 `T/F/B/N` 的 opt-in 语义（双轨 `(t,f)`、`∧/∨/¬/⇒/⇔`、
  4^n 真值表与逐点等价）；`trit belnap eval|table|equiv`；`sansheng` 再导出。
- 网表指令 `const <0|1|X> <out>` 与 `buf <in> <out>`；`examples/x_mask.net` 演示 `X & 0` / `X | 1` 遮蔽。
- 网表校验：同一信号多驱动（含“既是主输入又被门驱动”）与重复主输入分别报
  `MultipleDrivers` / `DuplicateInput`。
- `trit_circuit::propagate` 与 `trit circuit --assign`：给定一条含 `X` 的主输入向量，单遍拓扑传播，
  报告每根信号取值、主输出上的 `X` 来源，以及被遮蔽（输入含 `X`、输出确定）的门；不枚举 `3^n`。
- `trit_bdd::quantify_bdd` 与 `trit quantify`：三值存在/全称量化（`exists` / `forall`），
  量化域是 trit（`{0,1,X}`），可用 `--binary` 收窄到 `{0,1}`；提供自由变量上的见证/反例与 `--json`。
- `sansheng` 再导出 `Quantifier` / `Quantified` / `quantify_bdd`。

### Fixed

- `Gate::constant` 不再把自己的输出当输入，避免 Const 门被判成组合环。

## [0.3.2] - 2026-10-22

### Added

- `trit verify` 策略支持 `assumptions`：先检查前提可满足，再检查 `assumptions ⇒ assertion` 恒真；
  前提不一致返回 `InconsistentAssumptions`。
- `trit_sat::satisfiable` / `trit_bdd::satisfiable_bdd`：返回可满足见证。

## [0.3.1] - 2026-10-22

### Changed

- Release 产物扩到三平台：`trit-linux-x86_64.tar.gz`、`trit-windows-x86_64.zip`、
  `trit-macos-aarch64.tar.gz`；发布改为 build 矩阵 + 单次 publish，避免并发建 release。

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
