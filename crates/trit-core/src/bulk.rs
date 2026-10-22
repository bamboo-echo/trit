//! 位切片三值向量：与二进制位平面零拷贝互操作。
//!
//! 三值是双导轨 `(ones, unknowns)`：`(0,0)=False`、`(1,0)=True`、`(0,1)=Unknown`。
//! 二进制数据就是 `unknowns` 缺失（`None`）的 `TriVec`：
//!
//! ```
//! use trit_core::{TriVec, Ternary};
//! let words = vec![0b1010u64];
//! let v = TriVec::from_binary(words, 4);
//! assert_eq!(v.unknowns(), None);
//! assert_eq!(v.get(1), Ternary::True);
//! assert_eq!(v.get(0), Ternary::False);
//! assert_eq!(v.to_bools().unwrap(), vec![false, true, false, true]);
//! ```
//!
//! `from_binary` 直接接管 `Vec<u64>`，不复制、不分配 unknown 平面；
//! 只有显式写入 `Unknown` 时才分配第二个平面。

use crate::algebra::Algebra;
use crate::bits::TriBit;
use crate::value::Ternary;

/// `TriVec` 形状错误。
#[derive(Debug, thiserror::Error)]
#[error("TriVec 长度不匹配：{left} vs {right}")]
pub struct ShapeError {
    /// 左操作数长度。
    pub left: usize,
    /// 右操作数长度。
    pub right: usize,
}

/// `TriVec` 构造/访问错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriVecError {
    /// 位平面字数不足。
    #[error("位平面不足以覆盖 {len} 位：需要 {need} words，实际 {actual}")]
    PlaneTooSmall {
        /// 声明的位数。
        len: usize,
        /// 需要的字数。
        need: usize,
        /// 实际字数。
        actual: usize,
    },
    /// 元素索引越界。
    #[error("索引 {index} 越界（len={len}）")]
    IndexOutOfRange {
        /// 索引。
        index: usize,
        /// 向量长度。
        len: usize,
    },
    /// 字索引越界。
    #[error("字索引 {word} 越界（words={words}）")]
    WordOutOfRange {
        /// 字索引。
        word: usize,
        /// 字数。
        words: usize,
    },
}

/// 位切片的三值向量。
///
/// **相等语义按值**：两个 `TriVec` 只要长度与逐元素取值一致就相等，
/// 与内部是否分配了全零的 `unknowns` 平面无关。
#[derive(Debug, Clone)]
pub struct TriVec {
    len: usize,
    ones: Vec<u64>,
    unknowns: Option<Vec<u64>>,
}

impl PartialEq for TriVec {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        for (w, ones) in self.ones.iter().enumerate() {
            let left = self.unknowns.as_ref().map_or(0, |p| p[w]);
            let right = other.unknowns.as_ref().map_or(0, |p| p[w]);
            if *ones != other.ones[w] || left != right {
                return false;
            }
        }
        true
    }
}

impl Eq for TriVec {}

impl TriVec {
    /// 零拷贝接管二进制位平面：`ones` 的位为 1 表示 `True`，0 表示 `False`。
    ///
    /// `ones` 必须至少覆盖 `len` 位；多余字会被截断，末尾多余位会被清零。
    ///
    /// # Panics
    ///
    /// `ones` 字数不足时 panic；边界输入请用 [`TriVec::try_from_binary`]。
    pub fn from_binary(ones: Vec<u64>, len: usize) -> Self {
        Self::try_from_binary(ones, len).unwrap_or_else(|e| panic!("{e}"))
    }

    /// 同 [`TriVec::from_binary`]，但字数不足时返回错误。
    pub fn try_from_binary(mut ones: Vec<u64>, len: usize) -> Result<Self, TriVecError> {
        let need = len.div_ceil(64);
        if ones.len() < need {
            return Err(TriVecError::PlaneTooSmall {
                len,
                need,
                actual: ones.len(),
            });
        }
        ones.truncate(need);
        if let Some(last) = ones.last_mut() {
            *last &= last_mask(len);
        }
        Ok(Self {
            len,
            ones,
            unknowns: None,
        })
    }

    /// 从 `bool` 切片打包。
    pub fn from_bools(bits: &[bool]) -> Self {
        let mut ones = vec![0u64; bits.len().div_ceil(64)];
        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                ones[i / 64] |= 1u64 << (i % 64);
            }
        }
        Self {
            len: bits.len(),
            ones,
            unknowns: None,
        }
    }

    /// 从标量三值切片打包。
    pub fn from_ternary(values: &[Ternary]) -> Self {
        let mut ones = vec![0u64; values.len().div_ceil(64)];
        let mut unknowns: Option<Vec<u64>> = None;
        for (i, &value) in values.iter().enumerate() {
            match value {
                Ternary::False => {}
                Ternary::True => ones[i / 64] |= 1u64 << (i % 64),
                Ternary::Unknown => {
                    let planes = unknowns.get_or_insert_with(|| vec![0u64; ones.len()]);
                    planes[i / 64] |= 1u64 << (i % 64);
                }
            }
        }
        Self {
            len: values.len(),
            ones,
            unknowns,
        }
    }

    /// 若全部是二值，解包成 `bool`；含 `Unknown` 时返回 `None`。
    pub fn to_bools(&self) -> Option<Vec<bool>> {
        if !self.is_binary() {
            return None;
        }
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            out.push(self.get(i).is_true());
        }
        Some(out)
    }

    /// 长度。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 位平面字数。
    pub fn words(&self) -> usize {
        self.ones.len()
    }

    /// 是否为纯二值（没有 `Unknown`）。
    pub fn is_binary(&self) -> bool {
        self.unknowns
            .as_ref()
            .is_none_or(|planes| planes.iter().all(|&w| w == 0))
    }

    /// `ones` 位平面（只读）。
    pub fn ones(&self) -> &[u64] {
        &self.ones
    }

    /// `unknowns` 位平面；`None` 表示纯二值。
    pub fn unknowns(&self) -> Option<&[u64]> {
        self.unknowns.as_deref()
    }

    /// 取第 `i` 个三值。
    ///
    /// # Panics
    ///
    /// `i` 越界时 panic；边界输入请用 [`TriVec::try_get`]。
    pub fn get(&self, i: usize) -> Ternary {
        self.try_get(i).unwrap_or_else(|e| panic!("{e}"))
    }

    /// 取第 `i` 个三值；越界返回错误。
    pub fn try_get(&self, i: usize) -> Result<Ternary, TriVecError> {
        if i >= self.len {
            return Err(TriVecError::IndexOutOfRange {
                index: i,
                len: self.len,
            });
        }
        let ones = (self.ones[i / 64] >> (i % 64)) & 1;
        let unknowns = self
            .unknowns
            .as_ref()
            .map_or(0, |planes| (planes[i / 64] >> (i % 64)) & 1);
        Ok(match (ones, unknowns) {
            (1, 0) => Ternary::True,
            (0, 1) => Ternary::Unknown,
            _ => Ternary::False,
        })
    }

    /// 写第 `i` 个三值；首次写 `Unknown` 时分配 unknown 平面。
    ///
    /// # Panics
    ///
    /// `i` 越界时 panic；边界输入请用 [`TriVec::try_set`]。
    pub fn set(&mut self, i: usize, value: Ternary) {
        self.try_set(i, value).unwrap_or_else(|e| panic!("{e}"));
    }

    /// 写第 `i` 个三值；越界返回错误。
    pub fn try_set(&mut self, i: usize, value: Ternary) -> Result<(), TriVecError> {
        if i >= self.len {
            return Err(TriVecError::IndexOutOfRange {
                index: i,
                len: self.len,
            });
        }
        let word = i / 64;
        let bit = 1u64 << (i % 64);
        let mut cleared_last_unknown = false;
        match value {
            Ternary::False => {
                self.ones[word] &= !bit;
                if let Some(planes) = &mut self.unknowns {
                    planes[word] &= !bit;
                    cleared_last_unknown = planes.iter().all(|w| *w == 0);
                }
            }
            Ternary::True => {
                self.ones[word] |= bit;
                if let Some(planes) = &mut self.unknowns {
                    planes[word] &= !bit;
                    cleared_last_unknown = planes.iter().all(|w| *w == 0);
                }
            }
            Ternary::Unknown => {
                let planes = self
                    .unknowns
                    .get_or_insert_with(|| vec![0u64; self.ones.len()]);
                planes[word] |= bit;
                self.ones[word] &= !bit;
            }
        }
        if cleared_last_unknown {
            self.unknowns = None;
        }
        Ok(())
    }

    /// `Unknown` 的个数。
    pub fn count_unknowns(&self) -> usize {
        self.unknowns.as_ref().map_or(0, |planes| {
            planes.iter().map(|w| w.count_ones() as usize).sum()
        })
    }

    /// 取整个字（64 路）的位并行表示。
    ///
    /// # Panics
    ///
    /// 字索引越界时 panic；边界输入请用 [`TriVec::try_word`]。
    pub fn word(&self, word: usize) -> TriBit {
        self.try_word(word).unwrap_or_else(|e| panic!("{e}"))
    }

    /// 取整个字的位并行表示；越界返回错误。
    pub fn try_word(&self, word: usize) -> Result<TriBit, TriVecError> {
        if word >= self.ones.len() {
            return Err(TriVecError::WordOutOfRange {
                word,
                words: self.ones.len(),
            });
        }
        Ok(TriBit {
            ones: crate::bits::BitPlane(self.ones[word]),
            unknowns: crate::bits::BitPlane(
                self.unknowns.as_ref().map_or(0, |planes| planes[word]),
            ),
        })
    }

    /// 原地取反：`True`/`False` 互换，`Unknown` 不变。
    pub fn not(&mut self) {
        let mask = last_mask(self.len);
        for (w, ones) in self.ones.iter_mut().enumerate() {
            let x = self.unknowns.as_ref().map_or(0, |planes| planes[w]);
            *ones = !(*ones | x) & mask_of(w, self.len, mask);
        }
    }

    /// 原地与。K3/Ł3 用格序，B3 用毒值。
    pub fn and_assign(&mut self, other: &Self, alg: Algebra) -> Result<(), ShapeError> {
        self.check_shape(other)?;
        let mask = last_mask(self.len);
        let both_binary = self.unknowns.is_none() && other.unknowns.is_none();
        let mut new_unknowns = if both_binary {
            None
        } else {
            Some(Vec::with_capacity(self.ones.len()))
        };
        for w in 0..self.ones.len() {
            let o1 = self.ones[w];
            let o2 = other.ones[w];
            let x1 = self.unknowns.as_ref().map_or(0, |p| p[w]);
            let x2 = other.unknowns.as_ref().map_or(0, |p| p[w]);
            let m = mask_of(w, self.len, mask);
            let (o, x) = match alg {
                Algebra::Bochvar => {
                    let x_out = (x1 | x2) & m;
                    ((o1 & o2) & !x_out & m, x_out)
                }
                Algebra::Kleene | Algebra::Lukasiewicz => {
                    let f1 = !o1 & !x1;
                    let f2 = !o2 & !x2;
                    let o_out = (o1 & o2) & m;
                    let f_out = (f1 | f2) & m;
                    (o_out, !(o_out | f_out) & m)
                }
            };
            self.ones[w] = o;
            if let Some(planes) = &mut new_unknowns {
                planes.push(x);
            }
        }
        match new_unknowns {
            Some(planes) => self.replace_unknowns(planes),
            None => self.unknowns = None,
        }
        Ok(())
    }

    /// 原地或。K3/Ł3 用格序，B3 用毒值。
    pub fn or_assign(&mut self, other: &Self, alg: Algebra) -> Result<(), ShapeError> {
        self.check_shape(other)?;
        let mask = last_mask(self.len);
        let both_binary = self.unknowns.is_none() && other.unknowns.is_none();
        let mut new_unknowns = if both_binary {
            None
        } else {
            Some(Vec::with_capacity(self.ones.len()))
        };
        for w in 0..self.ones.len() {
            let o1 = self.ones[w];
            let o2 = other.ones[w];
            let x1 = self.unknowns.as_ref().map_or(0, |p| p[w]);
            let x2 = other.unknowns.as_ref().map_or(0, |p| p[w]);
            let m = mask_of(w, self.len, mask);
            let (o, x) = match alg {
                Algebra::Bochvar => {
                    let x_out = (x1 | x2) & m;
                    ((o1 | o2) & !x_out & m, x_out)
                }
                Algebra::Kleene | Algebra::Lukasiewicz => {
                    let f1 = !o1 & !x1;
                    let f2 = !o2 & !x2;
                    let o_out = (o1 | o2) & m;
                    let f_out = (f1 & f2) & m;
                    (o_out, !(o_out | f_out) & m)
                }
            };
            self.ones[w] = o;
            if let Some(planes) = &mut new_unknowns {
                planes.push(x);
            }
        }
        match new_unknowns {
            Some(planes) => self.replace_unknowns(planes),
            None => self.unknowns = None,
        }
        Ok(())
    }

    fn replace_unknowns(&mut self, planes: Vec<u64>) {
        if planes.iter().all(|&w| w == 0) {
            self.unknowns = None;
        } else {
            self.unknowns = Some(planes);
        }
    }

    fn check_shape(&self, other: &Self) -> Result<(), ShapeError> {
        if self.len == other.len {
            Ok(())
        } else {
            Err(ShapeError {
                left: self.len,
                right: other.len,
            })
        }
    }
}

fn last_mask(len: usize) -> u64 {
    match len % 64 {
        0 => u64::MAX,
        r => (1u64 << r) - 1,
    }
}

fn mask_of(word: usize, len: usize, last: u64) -> u64 {
    let words = len.div_ceil(64);
    if word + 1 == words { last } else { u64::MAX }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(len: usize, seed: u64) -> Vec<Ternary> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                match (x >> 32) % 3 {
                    0 => Ternary::False,
                    1 => Ternary::True,
                    _ => Ternary::Unknown,
                }
            })
            .collect()
    }

    #[test]
    fn binary_view_is_zero_copy_and_roundtrips() {
        for len in [0usize, 1, 63, 64, 65, 130] {
            let bits: Vec<bool> = (0..len).map(|i| i % 3 == 0).collect();
            let v = TriVec::from_bools(&bits);
            assert_eq!(v.unknowns(), None);
            assert!(v.is_binary());
            assert_eq!(v.to_bools().unwrap(), bits);
            let words = v.ones().to_vec();
            let moved = TriVec::from_binary(words, len);
            assert_eq!(moved.to_bools().unwrap(), bits);
        }
    }

    #[test]
    fn from_binary_normalizes_oversized_planes() {
        let normalized = TriVec::from_binary(vec![0, 0], 1);
        assert_eq!(normalized.words(), 1);
        assert_eq!(normalized, TriVec::from_binary(vec![0], 1));

        // 尾部 padding 必须被清零
        let masked = TriVec::from_binary(vec![u64::MAX], 3);
        assert_eq!(masked.to_bools().unwrap(), vec![true, true, true]);

        // 物理字数不同的两个向量可以安全运算
        let mut left = TriVec::from_binary(vec![1, 0], 1);
        let right = TriVec::from_binary(vec![1], 1);
        left.and_assign(&right, Algebra::Kleene).unwrap();
        assert_eq!(left.to_bools().unwrap(), vec![true]);
    }

    #[test]
    fn unknown_allocates_lazily() {
        let mut v = TriVec::from_bools(&[true, false]);
        assert_eq!(v.unknowns(), None);
        v.set(1, Ternary::Unknown);
        assert!(v.unknowns().is_some());
        assert_eq!(v.get(1), Ternary::Unknown);
        assert_eq!(v.to_bools(), None);
        assert_eq!(v.count_unknowns(), 1);
    }

    #[test]
    fn bit_ops_match_scalar() {
        for alg in Algebra::ALL {
            let a = sample(130, 7);
            let b = sample(130, 11);
            let mut va = TriVec::from_ternary(&a);
            let vb = TriVec::from_ternary(&b);
            va.and_assign(&vb, alg).unwrap();
            for i in 0..a.len() {
                assert_eq!(va.get(i), alg.and(a[i], b[i]), "{alg:?} and at {i}");
            }
            let mut va = TriVec::from_ternary(&a);
            va.or_assign(&vb, alg).unwrap();
            for i in 0..a.len() {
                assert_eq!(va.get(i), alg.or(a[i], b[i]), "{alg:?} or at {i}");
            }
        }
    }

    #[test]
    fn not_matches_scalar() {
        let a = sample(96, 3);
        let mut v = TriVec::from_ternary(&a);
        v.not();
        for (i, &value) in a.iter().enumerate() {
            assert_eq!(v.get(i), Algebra::Kleene.not(value), "not at {i}");
        }
    }

    #[test]
    fn shape_mismatch_is_an_error() {
        let mut a = TriVec::from_bools(&[true, false]);
        let b = TriVec::from_bools(&[true]);
        assert!(a.and_assign(&b, Algebra::Kleene).is_err());
    }

    #[test]
    fn eq_compares_values_not_storage() {
        let mut a = TriVec::from_ternary(&[Ternary::Unknown]);
        assert!(a.unknowns().is_some());
        a.set(0, Ternary::False);
        // 清掉最后一个 Unknown 后平面被规范化，表示差异不能影响相等。
        assert_eq!(a.unknowns(), None);
        assert_eq!(a, TriVec::from_bools(&[false]));
        assert_ne!(a, TriVec::from_bools(&[true]));
        // 未知平面非零时不能与二值零向量相等
        assert_ne!(
            TriVec::from_ternary(&[Ternary::Unknown]),
            TriVec::from_bools(&[false])
        );
    }

    #[test]
    fn fallible_constructors_and_accessors_report_bounds() {
        assert_eq!(
            TriVec::try_from_binary(vec![], 1).unwrap_err(),
            TriVecError::PlaneTooSmall {
                len: 1,
                need: 1,
                actual: 0
            }
        );
        let v = TriVec::from_bools(&[true]);
        assert_eq!(
            v.try_get(1).unwrap_err(),
            TriVecError::IndexOutOfRange { index: 1, len: 1 }
        );
        assert_eq!(
            v.try_word(1).unwrap_err(),
            TriVecError::WordOutOfRange { word: 1, words: 1 }
        );
        let mut w = v.clone();
        assert!(w.try_set(1, Ternary::False).is_err());
        assert_eq!(w, v);
    }
}
