# Changelog

## [Unreleased]

### Changed（精确传播：索引化内循环 + 有上限的并行）

- `propagate_exact` 的枚举不再对每个完成调用 `propagate`（那会为**每个信号**建
  `BTreeMap<String, _>`）；改成复用 `Simulator` 的索引化求值，只分配一份向量。
  同机同电路实测：k=8 **74×**、k=10 **209×**、k=12 **334×**（14.36 ms → 194 µs / 57.8 ms →
  277 µs / 239 ms → 716 µs），`k=8` 的永久基线读数见 `benches/core.rs`。
- 新增 `propagate_exact_with_jobs(..., jobs)`：完成之间独立，按块分给 `jobs` 个线程，
  再按「值不同记 `X`」合并；结果与并行度无关（`sim_tests` 在真分块的 k=12 上对拍串行）。
- **并行度按工作量收敛**：每线程至少 1024 个完成（`MIN_COMPLETIONS_PER_JOB`），
  `jobs = 0` 表示自动。实测 k ≤ 10 串行更快，k=12 四线程 1.6×，k=14 2.2×，k=16 5.4×；
  不设上限时 `available_parallelism` 会在亚毫秒任务上炸出几十个线程，反而更慢（k=12 auto 0.92 ms
  vs 四线程 0.46 ms）。`ExactPropagation` 新增 `jobs` 字段报告**实际**并行度。
- CLI：`trit circuit FILE --assign ... --exact [--jobs N]`，表格输出枚举完成数与实际并行度，
  JSON 增加 `exact` 对象（`exact` / `completions` / `budget` / `jobs` / `unknown_inputs`）。
- 测试：新增 `sim_tests`（串并行等价、**真分块**断言 `jobs == 4`、完成数/预算上报、超预算回退）
  + 3 条 CLI 测试；变异门 139 → **141**（新增 3 条并逐条实跑判红，删 1 条被重写覆盖的旧锚点）。

### Changed（三值拓扑：两趟 BFS + 三值验证证书）

- `TernaryTopology::reachability` 从「逐边松弛到不动点」改为**两趟 BFS**：先沿 `True` 边求
  `True` 集，再从 `True` 集出发沿非 `False` 边求 `Unknown` 集。结果与旧最小不动点逐点一致
  （200 个随机图上与参考实现逐点对拍），但**不再依赖边的插入顺序**。
- 实测（研究探针，未入库）：稀疏随机图 1.7×；长链且边按拓扑序 0.3×（旧实现一轮扫完）；
  同一条链边逆序时旧实现 12,500 轮 / 20.8 ms，新实现 0.10 ms（**199×**）；稠密图 1.1×。
- 邻接表随 `add_edge` 持久维护：第一版每次调用重建邻接表，实测反而慢 3×，已改。

### Added（三值自己的验证结论）

- `TernaryTopology::explain(origin, target) -> ReachEvidence`：三值结论
  `Verdict::{Proved, Undetermined, Refuted}` + `value` + `path` + `unknown_edges`。
  `Proved` 给纯 `True` 路径；`Undetermined` 给一条路径与路径上需要补全的 `Unknown` 边；
  `Refuted` 表示任何补全都不可达。结论是**通过 / 未定 / 否定**，不是二值的「可能」。
- 新增 `trit topology FILE [--origin N] [--to M] [-f json]`：JSON 输入
  （`nodes` / `origin` / `edges`，边值 `0`/`1`/`X`）；不给 `--to` 输出整张传播表，
  给了则输出单点判定与证书。`TernaryTopology` 此前没有任何消费方，现在有了对外入口。
- 测试：9 条库测试（200 组随机图与旧不动点对拍、100 组随机图 explain 与 reachability 一致）
  + 4 条 CLI 集成测试。
- 变异门 136 → **139**：删 1 条已失效的（拆模块后锚点落进测试代码），新增 4 条并逐条实跑判红。
- 变异门 `preflight` 增加规则：锚点落在 `#[cfg(test)] mod ...` 之后直接报错——
  改测试代码不改变产品行为，不该算覆盖。

### Added（对比基线规则 + 高压测试门）

- `docs/TERNARY-BOUNDARIES.md` 新增两条硬规则：
  - **§6 任何对比都必须同时给二进制基线**：性能要同机 `f32`/`f64`（或 1-bit 二值）读数，
    精度要同比特预算的二进制上界；**没有基线不算结论**，且必须写明省的是**带宽/存储**还是**指令数**。
  - **§7 高压测试**：位运算 / 编解码 / 浮点边界必须有跨字边界、极端向量
    （全 `+1` / 全 `-1` / 单点 / 交替）与大规模随机的不变量测试；只测小样例不算验证。
- 新增 `tests/stress.rs`：
  - `trit-dot`：极端向量 × 长度 `1/31/32/33/63/64/65/1000`、bipolar 尾部补位、
    3 个种子 × 1M 随机、块级稀疏，以及**二进制基线自检**（朴素 f32 与四累加器互相一致）。
  - `trit-codec`：0..=256 全长往返、10 万 trit 大载荷、256 个字节值分类、2000 个随机 `i64`、
    200 组随机消息 × 4 位置 × 2 幅度的单符号纠错。
  - `trit-float`：6 个精度 × 1000 个值（指数 -40..40）不越 `3^{1-p}`；乘积误差 ≤ 2.5× 上界。
- CI 新增 `stress (high pressure invariants)` job（release 模式跑上述套件）。

### Added（GPU 后端 `trit-gpu`：WGSL `countOneBits`）

- 新 crate `crates/trit-gpu`（`publish = false`）。**GPU 代码在 `gpu` feature 后面**，默认构建不拉
  `wgpu`，CI 的三平台 / MSRV job 不受影响；本地用 `--features gpu` 构建。
- `src/ternary.wgsl`：每 32 个 trit 打成 `u32`（双导轨），`countOneBits` 四组计数累加；
  `src/float.wgsl`：**同一个设备**上的 f32 点积（workgroup 归约），作为公平对照。
- CPU 位并行 `dot_words` 与 GPU 内核逐位一致；测试覆盖 1/31/32/33/100 跨字边界、四种符号组合、
  正负导轨不对称；GPU 测试在没有 adapter 时自动跳过。
- 实测（RTX 4060 Ti / Vulkan，5 批取中位数）：

  | 规模 | GPU 三值 | 同卡 f32 | CPU 位并行 | 加速 |
  |---|---|---|---|---|
  | 16M trit | **2.68 ps/trit** | 35.6 ps/元素 | 50.7 ps/trit | **13.3×** |
  | 1M trit | 40.6 ps/trit | 58.0 ps/元素 | 54.6 ps/trit | 1.4× |

  与内存带宽一致：三值 0.5 B/trit（4 个位平面 / 32 trit）vs f32 8 B/元素 = **16× 字节比**；
  1M 时数据进缓存、dispatch 延迟占主导，GPU 优势消失。
- 测试 4 条 + 1 条 GPU 测试；变异门 163 → **166**（少减一项、`+1` 写错导轨、标量参考改加法）。
- 边界：只测这一个 kernel，不是完整推理；没有与 int8/triton 内核对比；GPU 计时抖动大（已取中位数）。

### Added（发布图门 `scripts/publish_graph.py`）

- 新门：**任何 crate 都不许依赖未发布（`publish = false`）的 crate**。
  - 可发布 → 实验层：发布链断（红）；
  - 实验层 → 实验层：`cargo package --workspace` 会把实验层也一起打包，路径依赖没有版本 ⇒ 同样失败（红）；
  - 实验层 → 可发布：允许（绿）。
- 推论：**实验层必须自包含**；共享代码只能下沉到可发布 crate。`trit-net::format` 里 5-trit 打包
  之所以内联而不是复用 `trit-codec`，就是这条约束。
- 顺带检查可发布 crate 的 crates.io 必填字段（`description` / `license` / `repository`），
  并按依赖打印**发布顺序**（当前：17 个 crate / 11 个可发布）。
- `--self-test` 5 条断言：2 正例（可发布→可发布、实验层→可发布）+ 3 反证（可发布→实验层、
  实验层→实验层、缺 `license`），夹具是真实的两 crate workspace，跑的是真 `cargo metadata`。
- 接入 CI：新 job `publish graph (no dep on unpublished crates)`。
- 由来：本会话真实踩过两次（`trit-net → trit-codec` 让 `cargo package` 失败），现在由门强制。

### Added（三值点积的位并行实现 `trit-dot`）

- 新 crate `crates/trit-dot`（`publish = false`，不属于稳定 API）：
  - **二值（±1）点积**：`dot = n - 2·popcount(a XOR b)`——这才是「XNOR + POPC」能算的东西，
    每个 64 位块 **2 条指令**。
  - **三值点积**：`{-1,0,+1}` 一个 bit 装不下，必须双导轨（2 bit/trit）：
    `dot = Σa⁺b⁺ - Σa⁺b⁻ - Σa⁻b⁺ + Σa⁻b⁻`，每块 **4 AND + 4 POPC = 8 条指令**。
  - **块级稀疏跳过**：整块为 0 才跳过；**元素级稀疏救不了**——64 位块里几乎每块都有非零。
- 实测（1M 元素、同机、20 次平均）：

  | 方式 | 时间 | ns/元素 |
  |---|---|---|
  | 三值位并行 | 22.8 µs | 0.0217 |
  | 二值 XNOR+POPC | 6.4 µs | 0.0061 |
  | f32 朴素（单累加器） | 451 µs | 0.430 |
  | f32 四累加器（公平对照） | 113 µs | 0.108 |
  | 三值 + 块级 90% 稀疏 | 13.8 µs | 0.0132 |
  | 三值 + 元素级 90% 稀疏 | 43.4 µs | 0.0414（**更慢**） |

  三值位并行相对**公平**的 f32 约 **5×**（相对朴素 f32 是 19.8×；「16×」只在拿朴素基线时成立）。
- 结论：**「XNOR+POPC 算三值点积」是误传**。准确说法是「二值点积 2 条指令/64 元素，
  三值 8 条」；GPU 晶体管不是三值的，三值始终是 2 bit 模拟（B 类）。
- 测试 6 条库 + 3 条 CLI；变异门 160 → **163**（二值公式丢系数、三值少减一项、稀疏漏负负项）。

### Added（三值浮点实验层 `trit-float`）

- 新 crate `crates/trit-float`（`publish = false`，不属于稳定 API）：`value = mantissa · 3^exponent`，
  尾数是平衡三进制整数、规格化后最高位 `±1`、按 `precision` 位舍入；实现 `+ - * neg` 与 `f64` 互转。
- 舍入：丢掉最低位时余数只有 `±1`（不到半个 ulp），所以平衡三进制的**最近舍入就是截断**。
- 实测（`trit-float stats`，2000 次随机）：位数 6/12/18/24 的最大相对误差都低于理论上界 `3^{1-p}`；
  **同比特预算下三值的误差上界恒为二进制的 3 倍**（`3^{1-p}` vs `2^{-p·log2 3}`）。
  即三值浮点**没有精度红利**，相同存储白丢约 **1.58 bit**。
- 要匹配 `f64` 的 53 bit 精度，三值需要 **35 位 ≈ 55.47 bit（多 2.47 bit）**。
- 测试 7 条库 + 3 条 CLI；变异门 157 → **160**（舍入改截断、加法恒丢弃小项、乘法指数相减）。
- 边界：浮点精度是**基数无关的 D 类**问题；三值的优势只在整数的基数经济性（`3/ln3 ≈ 2.73` 最小），
  不在浮点精度。未实现次正规数、溢出标志、NaN / ±Inf 的三值表示。

### Added（三值注意力 / 快权重记忆实验层 `trit-attn`）

- 新 crate `crates/trit-attn`（`publish = false`，不属于稳定 API）：
  - **注意力 = 快权重 FFN**：`attention` 与 `ffn_view` 是同一计算的两个写法——第一层权重是 `K`、
    第二层是 `V`，都由输入现场生成，激活是 `exp`，**隐层宽度 = 序列长度**。
    20 组随机用例上逐元素差 `<1e-6`，本机实测最大差 **0.0**。
  - **三值关联记忆（编码器 / 解码器）**：`TernaryMemory` 把「键 → 值」写成整数矩阵 `W += v kᵀ`，
    解码 `W q` 取符号，编码与解码全程没有浮点。
  - `recall_experiment`：32 条键值对、维度 16，**逐分量**召回率
    **三值整数记忆 0.760 / softmax 注意力 0.963 / f32 对照 0.725**。
- CLI：`trit-attn demo [--pairs N] [--dim D] [--seed N]`。
- 测试 7 条库 + 3 条 CLI；变异门 154 → **157**（softmax 不归一化、记忆编码符号写反、三值化不夹边界）。
- 边界：f32 对照**未做能量对齐**（三值键 `|k|²≈2d/3`，f32 均匀键只有 `d/3`），不能据此说三值更好；
  记忆容量随条目数线性退化，没有做归一化/正交化；这是离散玩具模型，不是 Transformer 实现。

### Added（`trit-net`：二进制模型格式 + STE 反向传播）

- **模型格式 `TRN1`**：`magic / version / 层数` + 每层维度 + 权重与偏置按 **5 trits/byte** 打包
  （`{-1,0,+1} ↔ F/X/T` 只是存储映射，不代表 0 是逻辑未知）。113 参数的模型 **26 B**，
  同模型 JSON 是几百字节；非法魔数 / 版本 / 截断 / 尾部多余 / `243..=255` 非法字节全部显式报错。
- **反向传播（直通估计器 STE）**：保留 f32 影子权重，前向用 `clamp(round(w), -1, 1)`，
  梯度直通写回影子权重，训练结束量化回 `{-1,0,+1}`。只支持 1 个隐层的 tanh MLP、`±1` 输入输出。
- CLI：`trit-net train [--epochs N] [--hidden N] [--lr F]`、`trit-net format encode|decode`。
- 读数：majority-of-5（32 样本、6 隐元）300 轮，损失 **0.5 → 0.0027**，
  **三值网络精度 1.00**（影子权重同样 1.00），导出模型 26 B。
- 测试：库 16（格式往返 / 坏输入 / 训练收敛 / 标签与层数校验）+ CLI 7；
  变异门 151 → **154**（不校验魔数、打包长度向下取整、梯度上升）。
- 边界：STE 是近似梯度；只支持单隐层；合成任务上的精度不代表真实数据集。

### Added（三值编解码实验层 `trit-codec`）

- 新 crate `crates/trit-codec`（`publish = false`，不属于稳定 API）：
  - **`pack`：5 trits/byte**。`3^5 = 243 ≤ 256`，密度 **1.6 bit/trit**；1000 trit 实测 200 B
    （双导轨 250 B，省 20%；文本 `0/1/X` 1000 B，省 80%）。`243..=255` 是非法字节，解码显式报错；
    尾部不足 5 个时按长度截断。
  - **`balanced`：平衡三进制整数**（`+0-`）。i64 全域需要 **41 位**
    （`(3^40-1)/2 < i64::MAX < (3^41-1)/2`）；往返覆盖 `i64::MIN/MAX`；空串 / 非法字符 / 溢出分别报错。
  - **`hamming`：GF(3) 上的 `[4,2,3]` 纠错码**。2 个消息 trit → 4 个码字 trit，
    校验位 `p = a + b`、`q = a - b`；穷举 9×4×2 = **72 种单符号错误全部纠正**，最小距离实测 3。
    2 个错误不保证（距离 3 的固有上限），文档写明。
- CLI：`trit-codec pack|unpack|balanced|hamming|stats`。
- 测试 13 条库 + 5 条 CLI；变异门 147 → **151**（打包丢位权、非法字节边界写成 `>`、
  第 4 列误差幅度取错、平衡三进制负号解成 +1）。
- 边界：打包**不是压缩**（固定 1.6 bit/trit，不随分布变化）；Hamming 只保证纠 1 个符号；
  平衡三进制只做整数，不含分数。

### Added（三值小部件实验层 `trit-machines`）

- 新 crate `crates/trit-machines`（`publish = false`，不属于稳定 API），三个独立小机器：
  - **`vm` 三值栈式解释器**：`Jz` 的条件是 `X` 时不猜，两条路都走；结论分成
    「所有路径同值 / 多个可能值 / 截断」，并给出每个输出的路径数。
  - **`ca` 三值一维元胞自动机**（半径 1、总istic）：`3^7 = 2187` 条规则可整片枚举。
  - **`snn` 三值权重脉冲神经元**：`{-1,0,+1}` 突触、整数膜电位、不应期；统计脉冲率与活跃比例。
- CLI：`trit-machines vm|ca|snn`，JSON 输出。
- 研究读数（本机）：CA 在 64 格 × 200 步下 2187 条规则 = 死 274 / 不动点 351 / 周期 690 /
  复杂 872，最大熵规则 #1031（1.5846 bit ≈ log2 3）；SNN 100 神经元时**全局同步驱动会让网络
  退化成一个神经元**（脉冲率只落在离散档位），无驱动时一次 kick 后即静默——要看到传播必须异质驱动。
- 测试 16 条库 + 4 条 CLI 集成；变异门 144 → **147**，三条新变异逐条实跑判红。
- 边界：VM 不做相关性分析（`X & ~X` 仍是 `X`，那是 #90 分析器的活）；CA 只做总istic；
  SNN 是离散时间玩具模型，不能当神经形态硬件的证据。

### Added（三值网络实验层 `trit-net`）

- 新 crate `crates/trit-net`（`publish = false`，**不属于稳定 API**）：三值权重 `{-1,0,+1}`
  的全连接网络，信号是逻辑三值 `{F,X,T}`。`analyze` 枚举 `X` 的所有补全，把每个输出判成
  **确定 / 未定（带两组见证）/ 超预算**，同时给出逐层区间（保守）结论用于对照。
- CLI：`trit-net infer MODEL.json --inputs 1,X,0 [--budget N] [--json]`。
- 研究读数（本机原型，12→8→1，单种子合成任务）：f32 94.75% → 三值权重 84.75%
  （非零输出 85.45%），参数 452 B → 28 B；输入含 `X` 时精确解析 k=1 覆盖 70.3%/确定时精度 90.7%、
  k=4 19.8%/94.9%、k=8 0.7%/100%；而逐层区间只敢确定 0–3%，**假 X 占 95.7–100%**
  （远高于电路的 28.7%，因为区间误差逐层放大）。
- 测试 8 条（取消相关性、见证可复算、预算边界、超预算不猜）+ 4 条 CLI 测试；
  变异门 141 → **144**，三条新变异逐条实跑判红。
- 边界：只做前向解析、不含训练；精确解析是 `2^k` 枚举（k > 12 直接报超预算）；
  权重三值是**量化**（B 类税），只有 `X` 语义是本仓的 A 类能力。

### Added（X-Pessimism 分析）

- `trit_circuit::circuit::analyze_x_pessimism`：把单遍传播给出的 `X` 分成**真 X**与
  **悲观 X**。真 X 给出一对见证赋值（一个补全得 0、一个得 1）；悲观 X 给出可解析的确定值
  （不可满足的那一边就是证明）。
- 用 SAT（`varisat`）查询「输出能否为 0 / 能否为 1」，把 `X` 主输入当自由变量；
  不再需要 `2^k` 次完整传播，`propagate_exact` 的 4096 预算不再限制验证规模。
- `support` = 结构扇入锥里的 `X` 主输入（保守可达集，不是最小反例）。
- `Const X` 无法用二值编码表达，返回 `CircuitError::NonBinaryConstant`。
- CLI：`trit circuit FILE --assign a=X,b=0 --pessimism [-f json]`；JSON 含 `kind`、
  `resolved`、两个 `*_witness`、`support` 与 `sat_calls`。
- 测试：7 条库测试（重汇聚互补、mux 数据相同、见证代入复算、锥隔离、已知输出不发 SAT、
  `Const X` 拒绝、60 组随机电路与 `propagate_exact` 逐输出对拍）+ 3 条 CLI 集成测试。
- 变异门 133 → **136**（单边可满足判成 1、跳过 X 判定、支持集不回溯扇入），三条逐条实跑判红。
- **修变异门的跨平台缺陷**：18 条锚点把 `file` 写成反斜杠路径，Linux 上（周常全量门）
  直接 `FileNotFoundError`。统一为正斜杠，并在跑测试前加 `preflight`：文件缺失、锚点出现
  次数 != 1、路径含反斜杠一次性报全并退出，不再跑一半才崩。
- 实测（研究用探针，未入库）：200 个随机电路 / 800 个输出里，单遍的 383 个 `X` 有
  **110 个（28.7%）是悲观 X**。

### Changed（去上帝模块 + 门禁加强）

- **`trit-schedule` 去上帝模块**：`checkpoint.rs`（1095 行 / 82 个函数）拆成
  `checkpoint/{mod,file,memory,index,lock}.rs` + `tests.rs`；`runtime.rs`（988 行）拆成
  `runtime/{mod,types,execute}.rs` + `tests.rs`。公开路径不变（`trit_schedule::checkpoint::*`、
  `trit_schedule::runtime::*` 与 crate 根重导出均保持），无新增公开 API。
- **新门：模块预算**（`scripts/module_budget.py`）：单文件 800 行预算；超预算必须登记在
  `scripts/module_budget_baseline.json`，且台账**只允许变短**——变胖、变瘦却没收紧、
  文件已降到预算内却没移除、台账里的文件已删除，四种都判红；`--update` 拒绝任何增长。
  当前登记 5 件：`map.rs` 842、`verilog/parse.rs` 861、`verilog/tests.rs` 1094、
  `cli/tests.rs` 1100、`bits.rs` 801。
- **门的自证**：`--self-test` 用脚本自己的 `LIMIT` 生成夹具，10 条断言（正例 5 / 反证 5），
  逐条覆盖上面四种红法与 `--update` 的两条路径。
- **CI**：新增 `module budget (god objects)` job（先跑自测再查台账）；`mutation` 新增 PR 触发，
  只跑改动过的 crate（`mutation_check.py --pkg`），周常全量保留。
- 变异门仍为 133 条：拆分只改路径不改语义，锚点逐条重定位并验证唯一命中。

### Added（0.12 静态三值调度）

- 新增 `trit-schedule`：静态 DAG 调度，支持 `True` / `False` / `Unknown` 依赖；
  `Unknown` 直接延迟并沿必需依赖传播；输出 worker、start/finish、makespan、关键路径。
- 多 worker 事件驱动列表调度；高优先级先排，确定性 tie-break。
- CLI `trit schedule tasks.json --workers N`（table/json/csv/markdown）；`sansheng` 重新导出。
- 测试：链、并行、优先级、未知依赖传播、可选依赖、环/重复/未知引用拒绝、JSON 输入。
- 变异门 96 → 100（未知依赖、延迟传播、关键路径、优先级排序）。
- 这是静态调度 MVP；runtime/抢占/取消/故障恢复不在本轮。

### Added（0.12 runtime）

- `trit_schedule::run_runtime`：依赖驱动的 worker runtime；`RuntimeTask` 闭包、
  `CancelToken` 协作式取消、失败/取消后停止派发、`completed/failed/cancelled/deferred` 报告。
- 不强制抢占运行中的任务；任务需自行轮询 token。
- 测试：依赖顺序、双 worker 重叠、join 等待全部前置、失败后不派发新任务、取消未启动任务、
  未知依赖、panic 转失败、环/零 worker 拒绝。
- 变异门 100 → 103（取消、依赖传播、失败停止）。

### Added（0.12 资源配额）

- `Resources { cpu, memory }` + `RuntimeConfig::with_capacity`；任务 `with_requirements`。
- 严格优先级准入：队首放不下时等待；运行中任务不抢占。
- 超过容量的单任务直接 `Unschedulable`；报告新增 `peak_usage`。
- 测试：6+6 超过 10 时串行、超容量拒绝、配额下高优先级先跑。
- 变异门 103 → 106（容量、占用累计、超容量拒绝）。

### Added（0.12 可恢复任务协议）

- 新增 `ResumableTask::step`：步骤边界 `Yield` + 任务对象内检查点状态。
- `run_resumable` 单线程步进式参考执行器：高优先任务在步骤之间抢占低优先任务，
  低优先任务放回 ready 但状态保留，之后继续；资源配额同样生效。
- 报告 `preemptions`、`steps`、`peak_usage`；取消/失败后不执行新 step。
- 测试：抢占后从原步骤继续、优先级顺序、取消、失败/未知依赖、环/超容量。
- 变异门 106 → 109（抢占状态、优先级过滤、失败清理）。

### Added（0.12 threaded resumable + metrics）

- `run_resumable_threaded`：worker 在 `Yield` 边界抢占，任务对象保留；资源/优先级/取消沿用协议。
- `RuntimeMetrics` 统一指标：`started`（按任务去重）/`completed`/`failed`/`cancelled`/
  `deferred`/`preemptions`/`steps`/`peak_usage`；闭包 runtime 与 resumable runtime 都有。
- 测试：线程抢占后状态续跑、双 worker 重叠、取消/指标、单线程参考仍通过。
- 变异门 109 → 112（线程抢占状态、优先级过滤、started 去重）。

### Added（0.12 metrics export）

- `RuntimeMetrics::to_prometheus()` / `to_prometheus_with(prefix)`：Prometheus/OpenMetrics
  文本导出；报告提供 `metrics_prometheus()` 便捷方法；空前缀回退到 `trit_schedule`。
- 导出字段与 `RuntimeMetrics` 一致；峰值拆为 `peak_cpu` / `peak_memory`。
- 测试 `prometheus_exports_all_fields`；变异门 112 → 114。

### Added（0.12 checkpoint protocol）

- `CheckpointTask::save_state` + `TaskRestorer::restore`：可序列化状态协议，编码格式由任务决定。
- `Checkpoint` 带 `version`，`CheckpointStore` 按任务 id 覆盖保存/读取/删除/恢复。
- `CheckpointedTask` 包装器：每次 `Yield`/`Done` 后写入共享存储，可直接用于现有
  单线程/线程 runtime。
- 测试：检查点往返、存储覆盖/删除、版本拒绝、runtime 每步落盘并恢复。
- 变异门 114 → 117（版本校验、空状态、恢复校验）。

### Added（0.12 checkpoint files）

- `FileCheckpointStore`：版本化目录 `<root>/v1/<hex(task_id)>/<generation>.json`；
  临时文件 + rename 到新代文件（不覆盖），加载取最新代；`prune`/`remove`/`len`。
- 任务 id 十六进制转义，路径分隔符/unicode 不落到文件名；临时文件不参与加载。
- 测试：往返/代号递增、id 转义、prune/remove、版本拒绝；crate 33 → 36。
- 变异门 117 → 120（保存版本校验、加载最新代、prune 保留）。

### Added（0.12 checkpoint index）

- `CheckpointIndex` / `CheckpointEntry`：扫描 `v1/<hex>/`，列出 task id、最新代、状态字节数；
  按 task id 排序，只读元数据，不加载状态。
- `FileCheckpointStore::index()` / `task_ids()` / `index_json()`；`CheckpointIndex::get()`。
- 测试：多任务排序、unicode/slash id、最新代、空索引；crate 36 → 37。
- 变异门 120 → 123（id 解码、排序、state_bytes）。

### Added（0.12 checkpoint lock）

- `CheckpointLock`：`try_lock()` 原子创建 `v1/.lock` 目录，`Drop` 自动释放；
  `lock_timeout()` / `lock()`（默认 5 秒）带超时，占用时返回 `LockBusy`。
- 不自动包裹 `save`/`load`；调用方在临界区显式持有，避免改变现有单写者调用路径。
- 测试：排他、超时、释放后可重新获取；crate 37 → 38。
- 变异门 123 → 125（已存在分支、Drop 释放）。

### Added（0.12 checkpoint resume）

- `CheckpointSource`（内存/文件存储实现）+ `TaskFactory` + `ResumableSpecDef`。
- `restore_specs(defs, source, restorer, factory)`：有检查点则恢复，没有则新建；
  返回值可直接交给 `run_resumable` / `run_resumable_threaded`。
- 测试：内存源从第 3 步恢复只再走 7 步、文件源恢复、无检查点时用 factory。
- 变异门 125 → 127（忽略检查点、无检查点不建任务）。

### Added（0.12 exact propagation）

- `propagate_exact`：相关性感知的精确 X 传播。把 `X` 当未知 0/1，枚举完成并合并可能值；
  `a & ~a`（`a=X`）得到 `0`，修正单遍 Kleene 的假 X。
- 支持部分赋值（缺省主输入按 `X`）；未知输入 `k` 个需要 `2^k` 次完成。
- 超预算时回退到单遍保守结果，`exact=false`、`completions=0`；`DEFAULT_EXACT_BUDGET=4096`。
- 测试：重汇聚相关性、部分赋值、遮蔽、超预算、全已知输入；circuit 166 → 171。
- 变异门 127 → 130（冲突合并、预算、缺省 X）。

### Added（0.12 tri-state / ternary topology）

- `TriState`（0/1/Z）与 `TriStateConflict`：`Z` 是总线单位元，0/1 同时驱动报冲突；
  `TriStateBus` 逐位解析并校位宽；`to_ternary()` 把 `Z -> Unknown`。
- `TernaryTopology`：边带 True/Unknown/False 的有向图三值可达性；路径取 min、节点取 max，
  False 边断开；固定点迭代处理环。
- 测试：解析表/冲突、字符与 Ternary 转换、总线逐位/位宽、True 链、Unknown 传播、False/环；
  `trit-core` 67 → 73。
- 变异门 130 → 133（Z 单位元、总线宽度、拓扑 min/max）。

## [0.11.0] - 2026-10-23

0.10 与 0.11 合并发布：并行/共享 CNF、SDF per-instance、Verilog 参数化、CLI 硬超时，
以及 0.10 的桥接 SAT、映射面积、STA I/O 约束、层级/向量 Verilog。

### Changed（发布工程）

- workspace 版本 0.9.0 → **0.11.0**；新增每周 `quality` 工作流：`cargo audit` +
  8 个 fuzz 目标各 500 次 smoke。

### Added（SDF per-instance）

- **门实例元数据**：`Gate.instance`（`u1` / `u1.u2`），层级展开时写入；JSON 可选字段，
  顶层门为 `None`。
- **SDF 实例对齐**：`parse_sdf_instances` / `timing_model_from_sdf_for`；按 `INSTANCE` 匹配
  `Gate::instance`（容忍 `top.u1` 前缀），实例最坏 IOPATH 优先于 celltype；匹配不到回退 celltype。
- **STA**：`TimingModel::instance_delays` + `arc_delay_for_gate` / `output_arrival_for_gate`；
  同一实例的 rise/fall/条件弧仍合并为一个最坏值，不做引脚级差异。
- 测试：两实例 2/5 周期分别到达 y1/y2；SDF 前缀匹配；CLI `timing --sdf`。
- 变异门 94 → 96（不按实例覆盖、实例路径只做全等）。

### Added（0.11 硬超时）

- **全局 `--timeout-ms N`**：CLI 重新执行自身为子进程，超时 `kill` 并退出 **124**；
  子进程通过 `TRIT_TIMEOUT_CHILD=1` 跳过包装，避免递归。可兜住 ATPG/验证/REPL 等任意子命令。
- 这是进程级硬超时，不是 varisat 单故障取消；库 API 仍无中断句柄。
- 测试：`repl` 挂起时 200 ms 超时，断言退出码 124 且 5 s 内结束。

### Added（0.11 Verilog 参数）

- **参数化模块**：模块头 `#(parameter W = 8, ...)`、实例覆盖 `#(.W(4))` / `#(4)`；
  参数表达式支持 `+ - * / %`、括号与前置负号，用于位宽/下标/实例连接。
- 解析前做源码特化：每个参数组合一个唯一模块副本（上限 256），再由现有解析/层级展开处理；
  根模块保留原名，子模块名带 `__pN`。
- 明确限制：不支持模块体 `parameter`/`localparam`、`signed`/实数/字符串参数、参数化端口类型。
- 测试：默认值、命名/位置覆盖、父参数下传、同模块多参数实例、算术表达式、未知覆盖/体参数拒绝。
- 变异门 90 → 94（忽略覆盖、不替换、不归一化位宽、位置覆盖错位）。

### Added（0.11 共享 CNF）

- **`atpg --sat --shared-cnf`**：每个 worker 只建一份 good/faulty CNF，每个故障一个 selector；
  mux 链注入引脚常量，顺序 at-most-one 保证一次只激活一个故障。按 512 个故障切块，
  避免单 solver selector 过多导致的病态慢例。
- 默认逐故障路径不变（见证稳定）；共享路径覆盖/可测性一致，但见证向量可能不同。
- 10k 门 / 40k 故障、8 线程：**2.6 s**；3000 门 0.26 s；800 门 0.08 s（本机 24 线程）。
- 测试：共享路径与逐故障在随机小电路上对拍覆盖率/可测性/可观测性，并用 `verify_sat_patterns`
  验证测试向量真正检出故障；CLI `--shared-cnf` 覆盖率对拍。
- 变异门 87 → 90（候选不注入、mux 选反、不假设 selector）。

### Added（0.11 SDF-lite 互操作）

- **SDF 读取**：`trit_circuit::sdf::{parse_sdf, timing_model_from_sdf}`；`trit timing` /
  `trit check` 增加 `--sdf FILE`（与 `--library` 互斥）。只取 `CELL/CELLTYPE/DELAY/ABSOLUTE/
  IOPATH`，rise/fall 与 `min:typ:max` 取最坏值、`COND` 合并、向上取整；按 celltype 聚合，
  `TIMESCALE` 不换算，未知 celltype 忽略，无可识别延迟时报错。
- 测试：语法/缺延迟/负值拒绝、跨实例与条件弧最坏值、`min:typ:max`、CLI `timing`/`check`。
- 变异门 83 → 87（新增：取最小值、floor、跨实例取最小、放行负值）。

### Added（0.11 并行 SAT）

- **并行 ATPG / 桥接 SAT**：`generate_tests_sat_with_jobs` 与
  `generate_bridge_tests_sat_with_jobs`；CLI `atpg --sat --jobs N`、`bridge --sat --jobs N`
  （`0` = 自动，`1` = 串行）。按故障/桥接对分块，`std::thread::scope` 独立 CNF/solver，
  按原顺序合并，输出与串行逐字段一致；wasm32 强制串行。
- **策略断言并行**：`trit-core::parallel` 下沉为公共并发原语；`Policy::verify_with_bdd_config_and_jobs`、
  CLI `verify --jobs N`。前提只检查一次，断言顺序不变。
- **减少重复工作**：SAT CNF 复用调用方算好的拓扑序，`Cnf::new_with_order`；不再每个故障重跑
  `validate` / `topological_order`。
- 测速（本机 24 线程）：800 门串行 6.6 s → 2.5 s（拓扑序复用），`jobs=8` 0.37 s（并行 6.97×）；
  3000 门 34.4 s → 4.68 s（7.36×）；2000 条策略断言 0.19 s → 0.07 s。
- 测试：并行/串行逐字段对拍（ATPG、桥接、策略、CLI JSON）、顺序与错误传播。

### Added（0.10 STA I/O 约束）

- **时序 I/O 约束**：`TimingConstraints { input_arrival, output_required }`（JSON，拒绝未知字段/名字）；
  `analyze_timing_with_constraints` 与 `critical_paths_with_constraints`；`trit timing` / `trit check`
  增加 `--constraints FILE`，`--paths` 从输入到达时间起算。未列出的输入按 0、输出按 clock
  （省略 clock 时按关键路径），保持旧行为。
- 测试：到达/required/slack、逐输出 required、未知名字拒绝、JSON 往返、CLI `timing`/`check`。
- 变异门 80 → 83（新增：忽略输入到达、忽略逐输出 required、`--paths` 忽略输入到达）。

### Added（0.10 桥接 SAT + 映射面积）

- **桥接故障 SAT**：`trit bridge --sat` 对每个桥接构造 good / faulty CNF，要求两端在 good 下
  取值不同且至少一个主输出不同；SAT 给见证，UNSAT 证明不可测。支持 **≤64 主输入**；
  默认枚举路径仍为 ≤10。`BridgeReport` 新增公开字段 `sat`；不做 X 压缩。
- **映射面积**：`CellTiming` 支持 `{delay, area}` 与 `{delay, arcs, area}`；
  `CellLibrary::area`；`--area-first` 改为按 area 选择（省略按 1，旧单元库行为等价于单元数最少）。
- 测试：桥接 SAT 与枚举逐桥接对拍、20 输入、CLI `--sat`；映射 area 选择与 JSON 往返。
- 变异门 76 → 80（SAT 锚点迁移到共享 CNF，新增桥接 3 条 + 映射 1 条）。

### Added（0.10 SAT ATPG）

- **SAT-based ATPG**：`trit atpg --sat` 对每个门引脚故障构造 good / faulty CNF，
  要求至少一个主输出不同；SAT 给出测试向量，UNSAT 证明不可测（`UntestableReason::Proven`）。
  上限 64 个主输入（`Fault.witness` 仍为 `u64` 掩码）；`GateKind::Xor` 按代数编码
  （Ł3→OR，K3/B3→AND）。不做 X don't-care 压缩，也不证明测试集最小；未设单故障求解超时。
- `AtpgReport` 新增公开字段 `sat`（外部结构体字面量需补 `..Default::default()` 或改用构造器）；
  `CircuitError` 新增 `Sat` / `NonBinaryConstant`；`trit-circuit` 增加 `varisat` 依赖。
- 测试覆盖：随机小电路与枚举逐故障对拍（三种代数）、20 输入、恒 1 门的 UNSAT、
  常量 X 拒绝、65 输入上限、CLI `--sat`。
- 变异门 72 → 76（新增 4 条 SAT 规则）。

### Added（0.10 层级）

- **Verilog 多模块实例**：`parse_verilog` 现在解析一个文件里的多个 `module` 并做层级展开。
  连接支持 `.port(signal)` 与按端口顺序的位置连接；端口必须全部连接，重复连接报错；
  未知模块、递归、多顶层、重复实例名都返回错误。
- **层级模式支持 `[N:0]` 向量端口**：整条 bus 按位展开；同方向时子模块 `p[i]` 对应父模块 `a[i]`，
  跨方向按位序映射；宽度不匹配、把向量端口接到单比特、把标量端口接到整条 bus 都明确报错。
  单模块且无实例时保留原有完整向量行为。
- **`[0:N]` 升序向量范围**：解析、导出与 JSON 都保留方向；Verilog 位序（左界=MSB）贯通
  `assign`、拼接与层级跨方向连接。其它范围（非零下界）明确拒绝。
  `Bus` 新增公开字段 `ascending`（外部结构体字面量需补 `..Default::default()` 或改用构造器）；
  网表 JSON `format` 1 → 2，旧 `format: 1` 仍接受并按降序解释，更高版本拒绝。
  内部 `wire [N:0]` / `[0:N]` 声明也写入 `Circuit.buses`（此前只记录端口），导出按声明宽度与方向重组。
- 展开后的内部信号名加 `实例名__` 前缀；与用户信号冲突时显式报错，不静默复用。
  门数上限 10 万、嵌套 256 层，均在展开过程中检查。
- `hierarchy` 模块新增 17 条回归，另加 6 条 `[0:N]` 升序回归（解析/导出/JSON、位序、
  跨方向赋值、字面量、内部 wire、层级跨方向）。
- 变异门 62 → 72（新增 10 条：hierarchy 6 条 + 升序位序 4 条）。

### Docs

- **三进制四类边界**：新增 [docs/TERNARY-BOUNDARIES.md](docs/TERNARY-BOUNDARIES.md)，
  把原生三进制红利（A）/ 二进制模拟税（B）/ 生态路径依赖（C）/ 基数无关复杂度（D）
  分开；README「范围」与 DESIGN「非目标」已引用。后续结论、评审与路线图必须先归类。

## [0.9.0] - 2026-10-23

结构模型与 Verilog 前端增量：向量元数据、尺寸常量、JSON 版本；含 `Circuit` 新字段
（minor 允许破坏性调整）。

### Added（0.9 结构模型）

- **向量元数据**：`Circuit.buses` + `Bus { name, width }` 保留 `[width-1:0]` 声明；
  Verilog 解析填充，导出优先按声明宽度重组（允许只建模部分位），JSON 往返保留；
  空映射不序列化，旧 JSON 可直接读取。`CircuitError` 新增 `InvalidBus` / `BusNameConflict`。
  注：`Circuit` 新增公开字段，外部结构体字面量需补 `..Default::default()`。
- **Verilog 尺寸常量**：`<width>'<b|o|d|h><digits>`，支持 `_` 分隔与 b/o/h 的 `x`/`z`，
  不足高位补 0、超出截断高位；十进制常量限 128 位。尺寸常量可参与拼接与按位运算。
  变异门 59 → 62。
- **网表 JSON 版本字段**：`to_json` 写入 `format: 1`；旧 JSON（无 `format`）按 0 接受，
  更高版本明确拒绝。

## [0.8.2] - 2026-10-23

硬化补丁：修掉 Circuit 的静默常量默认，补齐结构校验，收紧 CI 与模块边界；无新增功能。

### Changed（0.8.2 硬化）

- **Circuit 结构不变量**：`validate` 拒绝常量门缺 `const_value`、非常量门带 `const_value`、
  空信号名与重复主输出；`Circuit::validate` 公开。`CircuitError` 新增四个变体
  （`ConstValueMissing` / `ConstValueUnexpected` / `EmptySignalName` / `DuplicateOutput`）。
- **`trit-bdd` 去上帝模块**：`lib.rs` 1059 行拆成 `encode` / `order` / `equiv` / `quantify`；
  公开路径不变。
- 所有 crate 启用 `#![forbid(unsafe_code)]`（当前全仓无 `unsafe`）。
- CI 新增 `cargo package --workspace --no-verify --locked` 可发布性门。
- 变异门 53 → 56（新增 Const 取值/空信号名规则）。

## [0.8.1] - 2026-10-23

评审修复收尾 + 三项芯片侧能力 + 按模块拆分上帝文件；三值语义不变。
本版含 **API 变更**（错误类型与若干返回 `Result` 的入口），升级前请读下面的 Changed 段。

### Changed（评审修复第二批，含 API 变更）

- **Verilog 错误结构化**：`VerilogError` 从 `Message(String)` 改为
  `{ kind, line, message }`，`kind ∈ Syntax / Unsupported / Limit / Export`；
  body 语句错误带 1 起行号（注释剥离保留换行），导出错误归 `Export`。
- **拆掉 circuit 模块环**：`validate` 独立到 `circuit/validate.rs`，`sim` 不再依赖 `analyze`；
  `sim → validate → graph`、`analyze → sim/validate/graph` 单向依赖。
- **ATPG 去上帝模块**：`atpg.rs`（1042 行）拆成 `atpg/{mod,compact}.rs` + `tests.rs`；
  `generate_tests` / `AtpgReport` / `CoverageStat` / `TestPattern` 公开路径不变。
- **Circuit 去上帝模块**：`circuit.rs`（1493 行）拆成 `circuit/{types,sim,analyze,graph}.rs`
  + `tests/test_support`；`Circuit`/`CircuitError`/`analyze`/`propagate` 等公开路径不变。
- **CLI 去上帝模块**：`trit-cli/src/main.rs`（2091 行）拆成 `commands/` 下 15 个命令模块，
  main 只保留 CLI 定义、分发与共享助手（约 590 行）；命令行为与参数不变。
- **Verilog 去上帝模块**：`verilog.rs`（1537 行）拆成 `verilog/{mod,expr,parse,export}.rs`
  + `verilog/tests.rs`；`parse_verilog` / `to_verilog` / `VerilogError` 公开路径不变。
- **设计去重**：ATPG 与桥接故障的精确/贪心集合覆盖抽到私有 `coverage` 模块，
  `TestPattern` 抽到 `pattern` 模块；`bridge` 不再直接调用 `atpg` 的私有实现，
  两套覆盖策略不再各写一份。公开 API 路径保持不变（`atpg::TestPattern` / crate 根导出）。
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
- `CellLibrary.cells` 的值类型改为 `CellTiming`（`Delay(u32)` 或 `Arcs { delay, arcs }`）；
  `TimingModel::from_json` 返回 `TimingModelError`，并做键归一化与弧数校验。

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
- 多比特 Verilog 子集：`input/output [N:0]` 向量端口按位展开为 `a[3]`… 单比特网；
  支持位选 `a[3]`、拼接 `{a, b}`、等宽逐位向量赋值 `assign y = a & b;`，
  导出时按连续位重新分组为向量端口。只支持 `[N:0]` 组合逻辑；拒绝 `[0:N]`、复制语法、
  位宽不匹配、非连续向量位。CLI `netlist --from verilog --to verilog` 可往返。
- 桥接故障 ATPG：`BridgePair` / `BridgeMode`（wired-AND、wired-OR）→ `generate_bridge_tests`，
  在二进制向量上判定短路可测性并给出覆盖向量；CLI `trit bridge` 支持 `--pairs`/`--max-pairs`/JSON。
  空列表与未知网络直接报错，规模超限拒绝；逻辑级模型，不建模模拟/布局效应。
- Liberty-lite 引脚弧：`TimingModel.arcs`（按 `A1`/`A2` → `Y`）与 `CellLibrary` 的
  `{"delay": N, "arcs": [...]}` 写法；`trit timing` / `check` / `map` 按输入引脚弧计算到达时间，
  慢弧会改变映射选实现。键大小写归一化；未知门、规范化重复、弧数与元数不符都报错。
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
