//! assign 右值到门的降级：临时信号命名、门数预算与表达式综合。

use std::collections::BTreeSet;

use trit_core::value::Ternary;

use crate::circuit::{Circuit, Gate, GateKind, Signal};

use super::VerilogError;
use super::expr::VExpr;
use super::parse::MAX_GATES;

pub(super) struct Builder<'a> {
    pub(super) circuit: &'a mut Circuit,
    pub(super) used: &'a mut BTreeSet<String>,
    pub(super) counter: &'a mut usize,
}

impl Builder<'_> {
    pub(super) fn fresh(&mut self) -> String {
        loop {
            let name = format!("__v{}", *self.counter);
            *self.counter += 1;
            if self.used.insert(name.clone()) {
                return name;
            }
        }
    }

    fn check_gate_budget(&self) -> Result<(), VerilogError> {
        if self.circuit.gates.len() >= MAX_GATES {
            return Err(VerilogError::limit(format!(
                "生成的门数超过上限 {MAX_GATES}"
            )));
        }
        Ok(())
    }

    pub(super) fn add_gate(
        &mut self,
        kind: GateKind,
        inputs: Vec<String>,
        output: &str,
    ) -> Result<(), VerilogError> {
        self.check_gate_budget()?;
        self.used.insert(output.to_string());
        self.circuit.add_gate(Gate::new(
            kind,
            inputs.into_iter().map(Signal::new).collect(),
            Signal::new(output),
        ));
        Ok(())
    }

    pub(super) fn add_const(&mut self, value: Ternary, target: &str) -> Result<(), VerilogError> {
        self.check_gate_budget()?;
        self.used.insert(target.to_string());
        self.circuit
            .add_gate(Gate::constant(value, Signal::new(target)));
        Ok(())
    }

    pub(super) fn synth_v(&mut self, expr: &VExpr, target: &str) -> Result<(), VerilogError> {
        match expr {
            VExpr::Var(name) => {
                if name != target {
                    self.add_gate(GateKind::Buf, vec![name.clone()], target)?;
                }
            }
            VExpr::Const(value) => {
                self.add_const(*value, target)?;
            }
            VExpr::ConstBits { bits, .. } => {
                let value = bits.first().copied().unwrap_or(Ternary::False);
                self.add_const(value, target)?;
            }
            VExpr::Not(inner) => {
                let source = self.temp_v(inner)?;
                self.add_gate(GateKind::Not, vec![source], target)?;
            }
            VExpr::And(a, b) | VExpr::Or(a, b) | VExpr::Xor(a, b) => {
                let left = self.temp_v(a)?;
                let right = self.temp_v(b)?;
                let kind = match expr {
                    VExpr::And(..) => GateKind::And,
                    VExpr::Or(..) => GateKind::Or,
                    _ => GateKind::XorClassic,
                };
                self.add_gate(kind, vec![left, right], target)?;
            }
            VExpr::Concat(_) => {
                return Err(VerilogError::unsupported("拼接只能用于向量化 assign"));
            }
        }
        Ok(())
    }

    fn temp_v(&mut self, expr: &VExpr) -> Result<String, VerilogError> {
        match expr {
            VExpr::Var(name) => Ok(name.clone()),
            VExpr::Const(value) => {
                let name = self.fresh();
                self.add_const(*value, &name)?;
                Ok(name)
            }
            VExpr::ConstBits { bits, .. } => {
                let name = self.fresh();
                self.add_const(bits.first().copied().unwrap_or(Ternary::False), &name)?;
                Ok(name)
            }
            VExpr::Concat(_) => Err(VerilogError::unsupported("拼接只能用于向量化 assign")),
            _ => {
                let name = self.fresh();
                self.synth_v(expr, &name)?;
                Ok(name)
            }
        }
    }
}
