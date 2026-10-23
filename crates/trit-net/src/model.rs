//! 三值权重网络的数据模型。
//!
//! 权重与偏置都限制在 `{-1, 0, +1}`；激活是符号函数（`0` 表示该神经元弃权）。
//! 这只是**实验层**，不属于 trit 的稳定 API。

use serde::{Deserialize, Serialize};

/// 一层全连接：`out = sign(W · in + b)`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    /// 输入维数。
    pub inputs: usize,
    /// 输出维数。
    pub outputs: usize,
    /// `weights[o][i]`，取值 `-1/0/1`。
    pub weights: Vec<Vec<i8>>,
    /// 偏置，取值 `-1/0/1`。
    pub bias: Vec<i8>,
}

impl Layer {
    fn validate(&self, index: usize) -> Result<(), ModelError> {
        if self.outputs == 0 || self.inputs == 0 {
            return Err(ModelError::Shape {
                layer: index,
                message: "inputs/outputs 必须非零".to_string(),
            });
        }
        if self.weights.len() != self.outputs || self.bias.len() != self.outputs {
            return Err(ModelError::Shape {
                layer: index,
                message: format!(
                    "weights/bias 行数应为 {}，实际 {}/{}",
                    self.outputs,
                    self.weights.len(),
                    self.bias.len()
                ),
            });
        }
        for row in &self.weights {
            if row.len() != self.inputs {
                return Err(ModelError::Shape {
                    layer: index,
                    message: format!("权重列数应为 {}，实际 {}", self.inputs, row.len()),
                });
            }
            for value in row {
                if !(-1..=1).contains(value) {
                    return Err(ModelError::Value(*value));
                }
            }
        }
        for value in &self.bias {
            if !(-1..=1).contains(value) {
                return Err(ModelError::Value(*value));
            }
        }
        Ok(())
    }
}

/// 整个网络：层按顺序串联。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    /// 各层。
    pub layers: Vec<Layer>,
}

impl Model {
    /// 解析 JSON 模型并校验形状。
    pub fn from_json(text: &str) -> Result<Self, ModelError> {
        let model: Self =
            serde_json::from_str(text).map_err(|error| ModelError::Json(error.to_string()))?;
        model.validate()?;
        Ok(model)
    }

    /// 输入维数。
    pub fn inputs(&self) -> usize {
        self.layers.first().map_or(0, |layer| layer.inputs)
    }

    /// 输出维数。
    pub fn outputs(&self) -> usize {
        self.layers.last().map_or(0, |layer| layer.outputs)
    }

    /// 校验层与层之间维度衔接。
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.layers.is_empty() {
            return Err(ModelError::Empty);
        }
        for (index, layer) in self.layers.iter().enumerate() {
            layer.validate(index)?;
            if index > 0 && layer.inputs != self.layers[index - 1].outputs {
                return Err(ModelError::Shape {
                    layer: index,
                    message: format!(
                        "输入维数 {} 与上一层输出 {} 不衔接",
                        layer.inputs,
                        self.layers[index - 1].outputs
                    ),
                });
            }
        }
        Ok(())
    }
}

/// 模型错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// 没有层。
    #[error("模型没有任何层")]
    Empty,
    /// 形状不合法。
    #[error("第 {layer} 层形状不合法：{message}")]
    Shape {
        /// 层下标。
        layer: usize,
        /// 说明。
        message: String,
    },
    /// 权重或偏置超出 `{-1,0,1}`。
    #[error("权重/偏置只能是 -1/0/1，实际 {0}")]
    Value(i8),
    /// JSON 解析失败。
    #[error("模型 JSON 解析失败：{0}")]
    Json(String),
}
