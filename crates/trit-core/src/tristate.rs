//! 三态（0 / 1 / Z）值、总线与冲突解析。
//!
//! 与 [`Ternary`] 的 `X` 不同：`Z` 是**高阻/未驱动**，多条线并联时可以被其他驱动覆盖；
//! `X` 是逻辑未知。两者只在需要时显式转换。

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::value::Ternary;

/// 一条线上的三态值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriState {
    /// 低电平（0）。
    Low,
    /// 高电平（1）。
    High,
    /// 高阻（Z）；不驱动总线。
    HighZ,
}

impl TriState {
    /// 全部取值。
    pub const ALL: [Self; 3] = [Self::Low, Self::High, Self::HighZ];

    /// 字符形式：`0` / `1` / `Z`。
    pub const fn as_char(self) -> char {
        match self {
            Self::Low => '0',
            Self::High => '1',
            Self::HighZ => 'Z',
        }
    }

    /// 从字符解析；大小写 `z` 也接受。
    pub const fn from_char(value: char) -> Option<Self> {
        match value {
            '0' => Some(Self::Low),
            '1' => Some(Self::High),
            'Z' | 'z' => Some(Self::HighZ),
            _ => None,
        }
    }

    /// 转为三值逻辑值：`Z -> Unknown`。
    pub const fn to_ternary(self) -> Ternary {
        match self {
            Self::Low => Ternary::False,
            Self::High => Ternary::True,
            Self::HighZ => Ternary::Unknown,
        }
    }

    /// 从三值逻辑值转换；`Unknown` 没有唯一的三态对应，返回 `None`。
    pub const fn from_ternary(value: Ternary) -> Option<Self> {
        match value {
            Ternary::False => Some(Self::Low),
            Ternary::True => Some(Self::High),
            Ternary::Unknown => None,
        }
    }

    /// 解析两条驱动：`Z` 是单位元，同值保持，0 与 1 同时驱动为冲突。
    pub fn resolve(self, other: Self) -> Result<Self, TriStateConflict> {
        match (self, other) {
            (Self::HighZ, value) | (value, Self::HighZ) => Ok(value),
            (left, right) if left == right => Ok(left),
            (left, right) => Err(TriStateConflict { left, right }),
        }
    }

    /// 解析任意多条驱动；遇到冲突立即返回。
    pub fn resolve_all(values: impl IntoIterator<Item = Self>) -> Result<Self, TriStateConflict> {
        let mut resolved = Self::HighZ;
        for value in values {
            resolved = resolved.resolve(value)?;
        }
        Ok(resolved)
    }
}

impl fmt::Display for TriState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_char().to_string())
    }
}

/// 两条线同时驱动相反电平。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("总线冲突：{left:?} 与 {right:?} 同时驱动")]
pub struct TriStateConflict {
    /// 一侧驱动。
    pub left: TriState,
    /// 另一侧驱动。
    pub right: TriState,
}

/// 三态总线；逐位解析多个驱动。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriStateBus {
    /// 每一位的三态值。
    pub values: Vec<TriState>,
}

impl TriStateBus {
    /// 从逐位取值构造。
    pub fn new(values: Vec<TriState>) -> Self {
        Self { values }
    }

    /// 用同一个值填充 `width` 位。
    pub fn filled(width: usize, value: TriState) -> Self {
        Self {
            values: vec![value; width],
        }
    }

    /// 位数。
    pub fn width(&self) -> usize {
        self.values.len()
    }

    /// 读取一位；越界返回 `None`。
    pub fn get(&self, lane: usize) -> Option<TriState> {
        self.values.get(lane).copied()
    }

    /// 逐位解析另一组驱动；位宽必须一致。
    pub fn resolve(&self, other: &Self) -> Result<Self, BusError> {
        if self.width() != other.width() {
            return Err(BusError::WidthMismatch {
                left: self.width(),
                right: other.width(),
            });
        }
        let mut values = Vec::with_capacity(self.width());
        for (lane, (left, right)) in self.values.iter().zip(&other.values).enumerate() {
            values.push(
                left.resolve(*right)
                    .map_err(|conflict| BusError::Conflict {
                        lane,
                        left: conflict.left,
                        right: conflict.right,
                    })?,
            );
        }
        Ok(Self { values })
    }

    /// 转为三值逻辑位：`Z -> Unknown`。
    pub fn to_ternary(&self) -> Vec<Ternary> {
        self.values.iter().map(|value| value.to_ternary()).collect()
    }
}

/// 总线解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BusError {
    /// 两边位宽不一致。
    #[error("总线位宽不一致：{left} vs {right}")]
    WidthMismatch {
        /// 左侧位宽。
        left: usize,
        /// 右侧位宽。
        right: usize,
    },
    /// 某一位冲突。
    #[error("第 {lane} 位冲突：{left:?} 与 {right:?}")]
    Conflict {
        /// 位号。
        lane: usize,
        /// 左侧驱动。
        left: TriState,
        /// 右侧驱动。
        right: TriState,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_table_and_conflict() {
        assert_eq!(TriState::HighZ.resolve(TriState::Low), Ok(TriState::Low));
        assert_eq!(TriState::HighZ.resolve(TriState::High), Ok(TriState::High));
        assert_eq!(TriState::High.resolve(TriState::High), Ok(TriState::High));
        assert_eq!(TriState::Low.resolve(TriState::Low), Ok(TriState::Low));
        assert_eq!(
            TriState::Low.resolve(TriState::High),
            Err(TriStateConflict {
                left: TriState::Low,
                right: TriState::High
            })
        );
        assert_eq!(
            TriState::resolve_all([TriState::HighZ, TriState::High, TriState::HighZ]),
            Ok(TriState::High)
        );
        assert!(TriState::resolve_all([TriState::Low, TriState::High]).is_err());
    }

    #[test]
    fn conversion_and_chars_round_trip() {
        for value in TriState::ALL {
            assert_eq!(TriState::from_char(value.as_char()), Some(value));
        }
        assert_eq!(TriState::HighZ.to_ternary(), Ternary::Unknown);
        assert_eq!(TriState::from_ternary(Ternary::Unknown), None);
        assert_eq!(TriState::from_ternary(Ternary::True), Some(TriState::High));
        assert_eq!(TriState::from_char('x'), None);
    }

    #[test]
    fn bus_resolves_lane_by_lane_and_checks_widths() {
        let first = TriStateBus::new(vec![TriState::HighZ, TriState::Low, TriState::High]);
        let second = TriStateBus::new(vec![TriState::High, TriState::HighZ, TriState::High]);
        let resolved = first.resolve(&second).unwrap();
        assert_eq!(
            resolved.values,
            vec![TriState::High, TriState::Low, TriState::High]
        );
        assert_eq!(
            resolved.to_ternary(),
            vec![Ternary::True, Ternary::False, Ternary::True]
        );
        assert!(matches!(
            first.resolve(&TriStateBus::filled(2, TriState::HighZ)),
            Err(BusError::WidthMismatch { .. })
        ));
        let conflict =
            TriStateBus::new(vec![TriState::Low]).resolve(&TriStateBus::new(vec![TriState::High]));
        assert!(matches!(conflict, Err(BusError::Conflict { lane: 0, .. })));
    }
}
