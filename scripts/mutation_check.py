#!/usr/bin/env python3
"""变异门：对关键规则做单点文本变异，跑对应回归；测试必须变红。

用法：python scripts/mutation_check.py
每个变异都会在 finally 里恢复原文件；退出码 1 表示有 mutant 存活（测试没抓住）。
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]

MUTANTS = [
    {
        "name": "core: K3 strong_add 用 OR 代替 AND",
        "file": "crates/trit-core/src/algebra.rs",
        "old": "    pub const fn strong_and(self, a: Ternary, b: Ternary) -> Ternary {\n        match self {\n            Self::Lukasiewicz => {\n                let s = a.as_f64() + b.as_f64() - 1.0;\n                if s <= 0.0 {\n                    Ternary::False\n                } else if s >= 1.0 {\n                    Ternary::True\n                } else {\n                    Ternary::Unknown\n                }\n            }\n            Self::Kleene | Self::Bochvar => {\n                if a.is_unknown() || b.is_unknown() {\n                    Ternary::Unknown\n                } else if a.is_true() && b.is_true() {",
        "new": "    pub const fn strong_and(self, a: Ternary, b: Ternary) -> Ternary {\n        match self {\n            Self::Lukasiewicz => {\n                let s = a.as_f64() + b.as_f64() - 1.0;\n                if s <= 0.0 {\n                    Ternary::False\n                } else if s >= 1.0 {\n                    Ternary::True\n                } else {\n                    Ternary::Unknown\n                }\n            }\n            Self::Kleene | Self::Bochvar => {\n                if a.is_unknown() || b.is_unknown() {\n                    Ternary::Unknown\n                } else if a.is_true() || b.is_true() {",
        "pkg": "trit-core",
        "filter": "strong_and_lukasiewicz_matches_formula"
    },
    {
        "name": "core: B3 或忘记扣掉未知位",
        "file": "crates/trit-core/src/bits.rs",
        "old": "let res1 = (a1 | b1) & !resx;",
        "new": "let res1 = a1 | b1;",
        "pkg": "trit-core",
        "filter": "ops_never_produce_illegal_rails"
    },
    {
        "name": "core: from_binary 不截断多余字",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "        ones.truncate(need);",
        "new": "        let _ = need;",
        "pkg": "trit-core",
        "filter": "from_binary_normalizes"
    },
    {
        "name": "eval: OR 吸收误用 AND 形状",
        "file": "crates/trit-eval/src/simplify.rs",
        "old": "    if let Expr::And(p, q) = y",
        "new": "    if let Expr::Or(p, q) = y",
        "pkg": "trit-eval",
        "filter": "absorption_only_removes_matching_operator"
    },
    {
        "name": "eval: K3 a^a 不再折回 a",
        "file": "crates/trit-eval/src/simplify.rs",
        "old": "    if x == y && alg != Algebra::Lukasiewicz {",
        "new": "    if x == y && alg == Algebra::Lukasiewicz {",
        "pkg": "trit-eval",
        "filter": "xor_and_equiv_self_respect_unknown"
    },
    {
        "name": "eval: 自等价对所有代数折叠成 1",
        "file": "crates/trit-eval/src/simplify.rs",
        "old": "    if x == y && alg == Algebra::Lukasiewicz {",
        "new": "    if x == y {",
        "pkg": "trit-eval",
        "filter": "xor_and_equiv_self_respect_unknown"
    },
    {
        "name": "sat: 二值 strong_add 的 Ł3 分支用 AND",
        "file": "crates/trit-sat/src/symbolic.rs",
        "old": "            Algebra::Lukasiewicz => self.binary_or(a, b),",
        "new": "            Algebra::Lukasiewicz => self.binary_and(a, b),",
        "pkg": "trit-sat",
        "filter": "binary_fast_path_uses_strong_add_not_xor"
    },
    {
        "name": "circuit: 拓扑排序退回声明序",
        "file": "crates\\trit-circuit\\src\\circuit\\graph.rs",
        "old": "            if deps.iter().all(|d| resolved.contains(d)) {",
        "new": "            if true {",
        "pkg": "trit-circuit",
        "filter": "evaluation_is_independent_of_gate_declaration_order"
    },
    {
        "name": "circuit: 引脚故障不注入",
        "file": "crates\\trit-circuit\\src\\circuit\\sim.rs",
        "old": "                ins[pin] = value;",
        "new": "                let _ = value;",
        "pkg": "trit-circuit",
        "filter": "fault_polarities_are_reported_separately"
    },
    {
        "name": "sat: 前提过滤不再要求 premise.t=1",
        "file": "crates/trit-sat/src/symbolic.rs",
        "old": "    enc.clause(&[Lit::from_var(p.t, true)]);",
        "new": "    enc.clause(&[Lit::from_var(p.t, false)]);",
        "pkg": "trit-policy",
        "filter": "counterexample_satisfies_premise"
    },
    {
        "name": "atpg: 全向量矩阵枚举为空",
        "file": "crates\\trit-circuit\\src\\atpg\\mod.rs",
        "old": "    let total = 1u64 << circuit.input_count();",
        "new": "    let total = 0;",
        "pkg": "trit-circuit",
        "filter": "atpg"
    },
    {
        "name": "circuit: 不可观测误判为被遮蔽",
        "file": "crates\\trit-circuit\\src\\circuit\\analyze.rs",
        "old": "                } else if reachable.contains(gate.output.name()) {",
        "new": "                } else if true {",
        "pkg": "trit-circuit",
        "filter": "untestable_reasons"
    },
    {
        "name": "circuit: Const 缺值不再报错",
        "file": "crates\\trit-circuit\\src\\circuit\\validate.rs",
        "old": "            (GateKind::Const, None) => {",
        "new": "            (GateKind::Const, None) if false => {",
        "pkg": "trit-circuit",
        "filter": "const_gate_without_value_is_rejected"
    },
    {
        "name": "circuit: 非 Const 带值不再报错",
        "file": "crates\\trit-circuit\\src\\circuit\\validate.rs",
        "old": "            (kind, Some(_)) if kind != GateKind::Const => {",
        "new": "            (_, Some(_)) if false => {",
        "pkg": "trit-circuit",
        "filter": "non_const_gate_with_value_is_rejected"
    },
    {
        "name": "circuit: 空信号名不再报错",
        "file": "crates\\trit-circuit\\src\\circuit\\validate.rs",
        "old": "        if sig.name().is_empty() {",
        "new": "        if false {",
        "pkg": "trit-circuit",
        "filter": "empty_signal_name_is_rejected"
    },
    {
        "name": "verilog: 向量声明记成 1 位",
        "file": "crates/trit-circuit/src/verilog/parse.rs",
        "old": "                    .insert(name.clone(), Bus::new(name.clone(), msb + 1));",
        "new": "                    .insert(name.clone(), Bus::new(name.clone(), 1));",
        "pkg": "trit-circuit",
        "filter": "vector_declarations_are_recorded_as_buses"
    },
    {
        "name": "verilog: 导出忽略 bus 宽度",
        "file": "crates/trit-circuit/src/verilog/export.rs",
        "old": "        let (max, vector) = if let Some(bus) = buses.get(&base) {",
        "new": "        let (max, vector) = if let Some(bus) = buses.get(&base).filter(|_| false) {",
        "pkg": "trit-circuit",
        "filter": "export_uses_declared_bus_width_for_partial_wire"
    },
    {
        "name": "circuit: 零宽 bus 不再报错",
        "file": "crates/trit-circuit/src/circuit/validate.rs",
        "old": "            || bus.width == 0",
        "new": "            || false",
        "pkg": "trit-circuit",
        "filter": "zero_width_bus_is_rejected"
    },
    {
        "name": "verilog: 尺寸常量位序反转",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "        'b' => {\n            for ch in digits.chars().rev() {",
        "new": "        'b' => {\n            for ch in digits.chars() {",
        "pkg": "trit-circuit",
        "filter": "sized_literals_preserve_bit_order"
    },
    {
        "name": "verilog: 尺寸常量零扩展成 X",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "    bits.resize(width, Ternary::False);",
        "new": "    bits.resize(width, Ternary::Unknown);",
        "pkg": "trit-circuit",
        "filter": "sized_literal_zero_extends_to_wider_vector"
    },
    {
        "name": "netlist: JSON 新版本不拒绝",
        "file": "crates/trit-circuit/src/netlist.rs",
        "old": "    if parsed.format > JSON_FORMAT {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "json_format_is_versioned_and_rejects_newer"
    },
    {
        "name": "netlist: gate 参数计数失效",
        "file": "crates/trit-circuit/src/netlist.rs",
        "old": "                if parts.len() != 5 {",
        "new": "                if false {",
        "pkg": "trit-circuit",
        "filter": "reports_line_numbers"
    },
    {
        "name": "atpg: 精确覆盖递归错误清除已覆盖位",
        "file": "crates/trit-circuit/src/coverage.rs",
        "old": "        search_cover(\n            uncovered & !faults,",
        "new": "        search_cover(\n            uncovered & faults,",
        "pkg": "trit-circuit",
        "filter": "minimum_cover_matches_bruteforce"
    },
    {
        "name": "bdd: 节点预算永不触发",
        "file": "crates/trit-bdd/src/encode.rs",
        "old": "    pub(crate) fn guard(&self, bdd: Bdd) -> Result<Bdd, BddError> {\n        if bdd.size() > self.limits.max_nodes {",
        "new": "    pub(crate) fn guard(&self, bdd: Bdd) -> Result<Bdd, BddError> {\n        if false {",
        "pkg": "trit-bdd",
        "filter": "budget_rejects"
    },
    {
        "name": "bdd: 频率降序写反",
        "file": "crates/trit-bdd/src/order.rs",
        "old": "                    cr.cmp(&cl)",
        "new": "                    cl.cmp(&cr)",
        "pkg": "trit-bdd",
        "filter": "frequency_order"
    },
    {
        "name": "bdd: is_valid 退回布尔全集",
        "file": "crates/trit-bdd/src/quantify.rs",
        "old": "        self.bdd.not().and(&self.valid_free).is_false()",
        "new": "        self.bdd.is_true()",
        "pkg": "trit-bdd",
        "filter": "forall_with_free_ternary_variable_is_judged_on_valid_domain"
    },
    {
        "name": "bdd: satisfiable 不限制合法域",
        "file": "crates/trit-bdd/src/equiv.rs",
        "old": "    let sat = enc.guard(enc.valid()?.and(&node.t).and(&node.x.not()))?;",
        "new": "    let sat = enc.guard(node.t.and(&node.x.not()))?;",
        "pkg": "trit-bdd",
        "filter": "satisfiable_requires_a_legal_valuation"
    },
    {
        "name": "verilog: 多输入 NAND/NOR 忘记最终反相",
        "file": "crates\\trit-circuit\\src\\verilog\\parse.rs",
        "old": "                        if invert {",
        "new": "                        if false {",
        "pkg": "trit-circuit",
        "filter": "multi_input_nand_nor_are_not_left_folded"
    },
    {
        "name": "atpg: X 位只验证一种填充",
        "file": "crates\\trit-circuit\\src\\atpg\\mod.rs",
        "old": "    for fill in 0..(1u64 << xbits.len()) {",
        "new": "    for fill in 0..1 {",
        "pkg": "trit-circuit",
        "filter": "dont_care_bits_are_jointly_fillable"
    },
    {
        "name": "check: 覆盖率无法执行时仍然 PASS",
        "file": "crates\\trit-cli\\src\\commands\\check.rs",
        "old": "    if atpg.is_none() && min_coverage > 0.0 {",
        "new": "    if false {",
        "pkg": "trit-cli",
        "filter": "check_fails_closed_when_coverage_cannot_run"
    },
    {
        "name": "aiger: 忽略时序锁存器",
        "file": "crates/trit-circuit/src/aiger.rs",
        "old": "    if latches > 0 {",
        "new": "    if false {",
        "pkg": "trit-circuit",
        "filter": "rejects_latches_strong_xor_and_bad_header"
    },
    {
        "name": "blif: 把 '-' 当成 0 字面量",
        "file": "crates/trit-circuit/src/blif.rs",
        "old": "                        \"-\" => None,",
        "new": "                        \"-\" => Some(false),",
        "pkg": "trit-circuit",
        "filter": "constants_and_dont_care_rows"
    },
    {
        "name": "verilog: assign 的 ^ 退化成强加三值 Xor",
        "file": "crates\\trit-circuit\\src\\verilog\\parse.rs",
        "old": "                    VExpr::Or(..) => GateKind::Or,\n                    _ => GateKind::XorClassic,",
        "new": "                    VExpr::Or(..) => GateKind::Or,\n                    _ => GateKind::Xor,",
        "pkg": "trit-circuit",
        "filter": "assign_xor_is_not_lukasiewicz_strong_sum"
    },
    {
        "name": "timing: 负载延迟不参与到达时间",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "        start + u64::from(self.load_delay) * fanout as u64",
        "new": "        start",
        "pkg": "trit-circuit",
        "filter": "load_delay_accumulates_with_fanout"
    },
    {
        "name": "map: 直接单元到达时间漏加单元延迟",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "            let total = start + u64::from(load_delay) * fanout;",
        "new": "            let total = u64::from(load_delay) * fanout;",
        "pkg": "trit-circuit",
        "filter": "timing_driven_picks_lower_arrival_implementation"
    },
    {
        "name": "map: 面积策略按延迟排序",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                MapStrategy::Area => (candidate.cells as u64, candidate.arrival as u64),",
        "new": "                MapStrategy::Area => (candidate.arrival as u64, candidate.cells as u64),",
        "pkg": "trit-circuit",
        "filter": "timing_driven_picks_lower_arrival_implementation"
    },
    {
        "name": "parser: 二元链不累计深度",
        "file": "crates/trit-core/src/parser.rs",
        "old": "        let depth = 1 + left.1.max(right.1);\n",
        "new": "        let depth = 1;\n",
        "pkg": "trit-core",
        "filter": "rejects_long_flat_binary_chain"
    },
    {
        "name": "parser: 解析期节点预算失效",
        "file": "crates/trit-core/src/parser.rs",
        "old": "        if self.nodes > self.limits.max_nodes {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "node_budget_is_enforced_while_parsing"
    },
    {
        "name": "parser: 输入字节上限失效",
        "file": "crates/trit-core/src/parser.rs",
        "old": "        if input.len() > MAX_INPUT_BYTES {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "oversized_input_is_rejected_before_collecting_chars"
    },
    {
        "name": "core: TriVec 相等忽略 unknown 平面",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "            if *ones != other.ones[w] || left != right {\n",
        "new": "            if *ones != other.ones[w] {\n",
        "pkg": "trit-core",
        "filter": "eq_compares_values_not_storage"
    },
    {
        "name": "core: TriVec 位平面不足不报错",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "        if ones.len() < need {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "fallible_constructors_and_accessors_report_bounds"
    },
    {
        "name": "core: 清空最后一个 Unknown 不规范化平面",
        "file": "crates/trit-core/src/bulk.rs",
        "old": "        if cleared_last_unknown {\n            self.unknowns = None;\n        }\n",
        "new": "        if false {\n            self.unknowns = None;\n        }\n",
        "pkg": "trit-core",
        "filter": "eq_compares_values_not_storage"
    },
    {
        "name": "core: 枚举变量数预算失效",
        "file": "crates/trit-core/src/bits.rs",
        "old": "        if self.variables.len() > limits.max_vars {\n",
        "new": "        if false {\n",
        "pkg": "trit-core",
        "filter": "evaluate_within_enforces_var_budget_directly"
    },
    {
        "name": "eval: 解码忽略剩余高位",
        "file": "crates/trit-eval/src/eval.rs",
        "old": "    if t != 0 {\n        return Err(AssignmentCodecError::OutOfDomain {\n",
        "new": "    if false {\n        return Err(AssignmentCodecError::OutOfDomain {\n",
        "pkg": "trit-eval",
        "filter": "checked_codec_rejects_out_of_domain_and_duplicates"
    },
    {
        "name": "eval: 编码对高位零也报溢出",
        "file": "crates/trit-eval/src/eval.rs",
        "old": "        if digit == 0 {\n            continue;\n        }\n",
        "new": "        if digit == 0 {\n        }\n",
        "pkg": "trit-eval",
        "filter": "assignment_index_does_not_over_reject_high_zero_digits"
    },
    {
        "name": "policy: 未引用环不报错",
        "file": "crates/trit-policy/src/lib.rs",
        "old": "                        return Err(PolicyError::Cycle(cycle.join(\" -> \")));\n",
        "new": "                        let _ = cycle;\n",
        "pkg": "trit-policy",
        "filter": "unused_definition_cycle_is_rejected_before_expansion"
    },
    {
        "name": "policy: binary 允许派生定义",
        "file": "crates/trit-policy/src/lib.rs",
        "old": "        if definitions.contains_key(name) {\n",
        "new": "        if false && definitions.contains_key(name) {\n",
        "pkg": "trit-policy",
        "filter": "binary_names_must_be_original_variables"
    },
    {
        "name": "circuit: 组合数溢出后饱和",
        "file": "crates\\trit-circuit\\src\\circuit\\types.rs",
        "old": "        checked_pow3(self.input_count())\n",
        "new": "        Ok(checked_pow3(self.input_count()).unwrap_or(u64::MAX))\n",
        "pkg": "trit-circuit",
        "filter": "analyze_rejects_too_many_inputs_before_enumeration"
    },
    {
        "name": "timing: 到达时间忽略引脚弧",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "            .map(|(index, arrival)| u64::from(*arrival) + u64::from(self.arc_delay(kind, index)))\n",
        "new": "            .map(|(_index, arrival)| u64::from(*arrival))\n",
        "pkg": "trit-circuit",
        "filter": "pin_arcs_change_arrival_and_critical_path"
    },
    {
        "name": "timing: 关键路径忽略引脚弧",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "                    let arc = u64::from(model.arc_delay(gate.kind, input_index));\n",
        "new": "                    let arc = 0u64;\n",
        "pkg": "trit-circuit",
        "filter": "pin_arcs_change_arrival_and_critical_path"
    },
    {
        "name": "map: 直接单元忽略引脚弧",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                let at = u64::from(arrival.get(signal.name()).copied().unwrap_or(0))\n                    + u64::from(library.arc_delay(gate.kind, input_index).unwrap_or(0));\n",
        "new": "                let at = u64::from(arrival.get(signal.name()).copied().unwrap_or(0));\n",
        "pkg": "trit-circuit",
        "filter": "pin_arcs_flip_implementation_choice"
    },
    {
        "name": "map: 弧数校验放宽",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                && arcs.len() != kind.arity()\n",
        "new": "                && arcs.len() > kind.arity()\n",
        "pkg": "trit-circuit",
        "filter": "cell_library_accepts_arc_form_and_validates"
    },
    {
        "name": "atpg: X 合并忽略唯一覆盖（保留全部故障）",
        "file": "crates\\trit-circuit\\src\\atpg\\compact.rs",
        "old": "                        need.retain(|fault| !detected.contains(fault));",
        "new": "                        need.retain(|_fault| true);",
        "pkg": "trit-circuit",
        "filter": "compaction_merges_witness_patterns"
    },
    {
        "name": "bridge: wired-OR 被当成 wired-AND",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "            Self::Or => Ternary::True,\n",
        "new": "            Self::Or => Ternary::False,\n",
        "pkg": "trit-circuit",
        "filter": "and_bridge_is_detected_only_for_wired_or"
    },
    {
        "name": "bridge: 两端值恒等导致全部不可测",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "                .map(|&(a, b, _)| (good[a], good[b]))\n",
        "new": "                .map(|&(a, _b, _)| (good[a], good[a]))\n",
        "pkg": "trit-circuit",
        "filter": "and_bridge_is_detected_only_for_wired_or"
    },
    {
        "name": "bridge: 空列表不失败",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "    if pairs.is_empty() {\n        return Err(BridgeError::NoPairs);\n    }\n",
        "new": "    if false {\n        return Err(BridgeError::NoPairs);\n    }\n",
        "pkg": "trit-circuit",
        "filter": "validation_rejects_empty_unknown_and_same_pairs"
    },
    {
        "name": "bridge: 同一网络不报错",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "        if a == b {\n",
        "new": "        if false {\n",
        "pkg": "trit-circuit",
        "filter": "validation_rejects_empty_unknown_and_same_pairs"
    },
    {
        "name": "verilog: 向量位选择恒取 bit 0",
        "file": "crates\\trit-circuit\\src\\verilog\\expr.rs",
        "old": "                Ok(VExpr::Var(format!(\"{name}[{index}]\")))\n",
        "new": "                Ok(VExpr::Var(format!(\"{name}[0]\")))\n",
        "pkg": "trit-circuit",
        "filter": "parses_vector_ports_and_vectorized_bitwise_assign"
    },
    {
        "name": "verilog: 拼接按 MSB-first 误解",
        "file": "crates\\trit-circuit\\src\\verilog\\expr.rs",
        "old": "            for item in items.iter().rev() {\n",
        "new": "            for item in items.iter() {\n",
        "pkg": "trit-circuit",
        "filter": "parses_bit_selects_and_concatenation"
    },
    {
        "name": "verilog: 向量端口只展开 bit 0",
        "file": "crates\\trit-circuit\\src\\verilog\\parse.rs",
        "old": "                    .insert(name.clone(), Bus::new(name.clone(), msb + 1));\n                for index in (0..=msb).rev() {",
        "new": "                    .insert(name.clone(), Bus::new(name.clone(), msb + 1));\n                for index in [0] {",
        "pkg": "trit-circuit",
        "filter": "parses_vector_ports_and_vectorized_bitwise_assign"
    },
    {
        "name": "verilog: 导出不分组向量端口",
        "file": "crates\\trit-circuit\\src\\verilog\\export.rs",
        "old": "            (max, true)",
        "new": "            (max, false)",
        "pkg": "trit-circuit",
        "filter": "vector_round_trip_regroups_ports"
    },
    {
        "name": "verilog: 右值深度预算失效",
        "file": "crates\\trit-circuit\\src\\verilog\\expr.rs",
        "old": "        if self.depth > MAX_RHS_DEPTH {\n",
        "new": "        if false {\n",
        "pkg": "trit-circuit",
        "filter": "deep_rhs_is_rejected"
    },
    {
        "name": "hierarchy: 所有模块都当作顶层",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "        .filter(|def| !instantiated.contains(def.name.as_str()))\n",
        "new": "        .filter(|_def| true)\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_named_connections_can_be_reordered"
    },
    {
        "name": "hierarchy: 位置连接按逆序绑定",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "                    let Some(port) = child.ports.get(index) else {\n",
        "new": "                    let Some(port) = child.ports.get(child.ports.len().saturating_sub(1 + index)) else {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_positional_connections_bind_by_port_order"
    },
    {
        "name": "hierarchy: 生成名冲突不报错",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "    if used.contains(&renamed) {\n",
        "new": "    if false && used.contains(&renamed) {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_generated_names_do_not_collide_with_user_signals"
    },
    {
        "name": "hierarchy: 向量端口宽度不校验",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "    if parent_width != child_width {\n",
        "new": "    if false && parent_width != child_width {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_vector_width_mismatch_is_rejected"
    },
    {
        "name": "hierarchy: 向量端口接受单比特连接",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "    if index.is_some() {\n",
        "new": "    if false && index.is_some() {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_vector_port_rejects_single_bit"
    },
    {
        "name": "hierarchy: 向量 bus 逆序连线",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "            let parent_index = if parent_asc {\n",
        "new": "            let parent_index = if true || parent_asc {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_vector_bit_order_is_preserved"
    },
    {
        "name": "verilog: 升序向量按降序取位",
        "file": "crates/trit-circuit/src/verilog/expr.rs",
        "old": "                let declared = if ascending.contains(name) {\n",
        "new": "                let declared = if false && ascending.contains(name) {\n",
        "pkg": "trit-circuit",
        "filter": "ascending_vector_bit_order_matches_verilog"
    },
    {
        "name": "verilog: 升序 LHS 不按位序落位",
        "file": "crates/trit-circuit/src/verilog/parse.rs",
        "old": "                                    let lhs_index = if ascending.contains(&lhs_base) {\n",
        "new": "                                    let lhs_index = if false && ascending.contains(&lhs_base) {\n",
        "pkg": "trit-circuit",
        "filter": "literal_assignment_to_ascending_vector_keeps_verilog_order"
    },
    {
        "name": "verilog: 导出升序向量写成降序",
        "file": "crates/trit-circuit/src/verilog/export.rs",
        "old": "        if group.ascending {\n",
        "new": "        if false {\n",
        "pkg": "trit-circuit",
        "filter": "ascending_ranges_parse_export_and_json_round_trip"
    },
    {
        "name": "hierarchy: 跨方向 bus 不按子端口位序",
        "file": "crates/trit-circuit/src/verilog/hierarchy.rs",
        "old": "            let child_index = if child_asc {\n",
        "new": "            let child_index = if false && child_asc {\n",
        "pkg": "trit-circuit",
        "filter": "hierarchical_cross_direction_bus_maps_by_position"
    },
    {
        "name": "sat: 常量 false 被强制成真",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "        solver.add_clause(&[Lit::from_var(false_var, false)]);\n",
        "new": "        solver.add_clause(&[Lit::from_var(false_var, true)]);\n",
        "pkg": "trit-circuit",
        "filter": "sat_matches_exhaustive_on_random_small_circuits"
    },
    {
        "name": "sat: 不要求输出不同",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "        self.solver.add_clause(&differences);\n",
        "new": "        if false { self.solver.add_clause(&differences); }\n",
        "pkg": "trit-circuit",
        "filter": "sat_matches_exhaustive_on_random_small_circuits"
    },
    {
        "name": "sat: 故障引脚不注入",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "            let lit = if faulty && inject.is_some_and(|(pin, _)| pin == index) {\n",
        "new": "            let lit = if false && faulty && inject.is_some_and(|(pin, _)| pin == index) {\n",
        "pkg": "trit-circuit",
        "filter": "sat_matches_exhaustive_on_random_small_circuits"
    },
    {
        "name": "sat: Xor 一律按经典 XOR",
        "file": "crates/trit-circuit/src/sat_cnf.rs",
        "old": "                    GateKind::Xor => match self.alg {\n                        // Ł3 强加法在 0/1 上是 OR；K3/B3 是 AND。\n                        Algebra::Lukasiewicz => self.or(y, a, b),\n                        Algebra::Kleene | Algebra::Bochvar => self.and(y, a, b),\n                    },\n",
        "new": "                    GateKind::Xor => self.xor(y, a, b),\n",
        "pkg": "trit-circuit",
        "filter": "sat_encodes_strong_xor_by_algebra"
    },
    {
        "name": "bridge: 不要求两端取值不同",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "        cnf.require_good_difference(&pair.a, &pair.b)?;\n",
        "new": "        if false { cnf.require_good_difference(&pair.a, &pair.b)?; }\n",
        "pkg": "trit-circuit",
        "filter": "sat_bridge_matches_exhaustive_for_small_circuits"
    },
    {
        "name": "bridge: wired 值反转",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "        let value = matches!(pair.mode, BridgeMode::Or);\n",
        "new": "        let value = !matches!(pair.mode, BridgeMode::Or);\n",
        "pkg": "trit-circuit",
        "filter": "sat_bridge_matches_exhaustive_for_small_circuits"
    },
    {
        "name": "bridge: 不要求输出不同",
        "file": "crates/trit-circuit/src/bridge.rs",
        "old": "        cnf.require_output_difference()?;\n",
        "new": "        if false { cnf.require_output_difference()?; }\n",
        "pkg": "trit-circuit",
        "filter": "sat_bridge_matches_exhaustive_for_small_circuits"
    },
    {
        "name": "map: area-first 忽略 area",
        "file": "crates/trit-circuit/src/map.rs",
        "old": "                MapStrategy::Area => (candidate.area as u64, candidate.arrival as u64),\n",
        "new": "                MapStrategy::Area => (candidate.cells as u64, candidate.arrival as u64),\n",
        "pkg": "trit-circuit",
        "filter": "area_first_uses_declared_area"
    },
    {
        "name": "timing: 忽略输入到达约束",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "            let arrival = constraints\n                .input_arrival\n                .get(signal.name())\n                .copied()\n                .unwrap_or(0);\n",
        "new": "            let arrival = 0;\n",
        "pkg": "trit-circuit",
        "filter": "input_arrival_and_output_required_constraints"
    },
    {
        "name": "timing: 忽略逐输出 required",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "            let required = constraints\n                .output_required\n                .get(signal.name())\n                .copied()\n                .unwrap_or(clock);\n",
        "new": "            let required = clock;\n",
        "pkg": "trit-circuit",
        "filter": "per_output_required_times_are_independent"
    },
    {
        "name": "timing: --paths 忽略输入到达",
        "file": "crates/trit-circuit/src/timing.rs",
        "old": "        let arrival = constraints\n            .input_arrival\n            .get(input.name())\n            .copied()\n            .unwrap_or(0);\n",
        "new": "        let arrival = 0;\n",
        "pkg": "trit-circuit",
        "filter": "input_arrival_and_output_required_constraints"
    }
]


def ran_any(output: str) -> bool:
    match = re.search(r"test result: ok\. (\d+) passed", output)
    return bool(match and int(match.group(1)) > 0)


def run_tests(pkg: str, test_filter: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "test", "-q", "-p", pkg, test_filter, "--locked"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


def main() -> int:
    # Windows 控制台默认可能是 GBK；变异名里有中国字与 Ł，统一按 UTF-8 输出。
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    baseline: dict[tuple[str, str], bool] = {}
    survived = 0
    for mutant in MUTANTS:
        key = (mutant["pkg"], mutant["filter"])
        if key not in baseline:
            result = run_tests(*key)
            if result.returncode != 0:
                print(f"BASELINE RED: {key}\n{result.stdout[-2000:]}")
                return 1
            baseline[key] = True

        path = ROOT / mutant["file"]
        original = path.read_text(encoding="utf-8")
        count = original.count(mutant["old"])
        if count != 1:
            print(f"BAD MUTANT {mutant['name']}: anchor 出现 {count} 次")
            return 1

        path.write_text(
            original.replace(mutant["old"], mutant["new"]), encoding="utf-8"
        )
        try:
            result = run_tests(*key)
        finally:
            path.write_text(original, encoding="utf-8")
            assert path.read_text(encoding="utf-8") == original, "恢复失败"

        if result.returncode == 0:
            survived += 1
            print(f"SURVIVED  {mutant['name']}")
        else:
            tail = next(
                (line for line in result.stdout.splitlines() if "FAILED" in line),
                "(有测试失败)",
            )
            print(f"caught    {mutant['name']}  [{tail.strip()}]")

    print(f"\n{len(MUTANTS) - survived}/{len(MUTANTS)} 个变异被抓住")
    return 1 if survived else 0


if __name__ == "__main__":
    sys.exit(main())
