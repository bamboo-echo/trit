//! 平衡三进制浮点实验层（`publish = false`，不属于稳定 API）。
//!
//! 表示：`value = mantissa · 3^exponent`，尾数用平衡三进制整数（数字 `{-1,0,1}`），
//! 规格化后最高位是 `±1`，有效位数不超过 `precision`。
//!
//! 舍入：去掉最低位时余数只能是 `±1`（不到半个 ulp），所以**最近舍入就是平衡三进制的截断**。
//!
//! 关键结论（`trit-float stats` 可复算）：同样**存储比特**下，三值浮点的相对误差上界约是二进制的
//! **3 倍**（`3^{1-p}` vs `2^{-B}`，其中 `B = p·log2 3`）——基数换了，信息量没变，白丢约 1.58 bit。

#![forbid(unsafe_code)]

/// `log2(3)`。
pub const LOG2_3: f64 = 1.584_962_500_721_156_2;

/// 精度下限。
pub const MIN_PRECISION: u32 = 1;
/// 精度上限（尾数留在 `i128` 的乘法安全区）。
pub const MAX_PRECISION: u32 = 40;

/// 一个平衡三进制浮点数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TritFloat {
    mantissa: i128,
    exponent: i32,
    precision: u32,
}

impl TritFloat {
    /// 用 `mantissa · 3^exponent` 构造并规格化；`precision` 会被夹到 `[1, 40]`。
    pub fn new(mantissa: i128, exponent: i32, precision: u32) -> Self {
        let precision = precision.clamp(MIN_PRECISION, MAX_PRECISION);
        let (mantissa, exponent) = normalize(mantissa, exponent, precision);
        Self {
            mantissa,
            exponent,
            precision,
        }
    }

    /// 正零。
    pub fn zero(precision: u32) -> Self {
        Self::new(0, 0, precision)
    }

    /// 从 `f64` 转换；`NaN` / `±Inf` 返回 `None`。
    pub fn from_f64(value: f64, precision: u32) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let precision = precision.clamp(MIN_PRECISION, MAX_PRECISION);
        if value == 0.0 {
            return Some(Self::zero(precision));
        }
        let sign = if value.is_sign_negative() { -1i128 } else { 1 };
        let mut x = value.abs();
        let mut exponent = 0i32;
        while x >= 1.5 {
            x /= 3.0;
            exponent += 1;
        }
        while x < 0.5 {
            x *= 3.0;
            exponent -= 1;
        }
        let scaled = x * 3f64.powi(precision as i32);
        let mantissa = (scaled.round() as i128) * sign;
        Some(Self::new(mantissa, exponent - precision as i32, precision))
    }

    /// 转回 `f64`（可能溢出为 `±Inf` 或下溢为 `0`）。
    pub fn to_f64(self) -> f64 {
        self.mantissa as f64 * 3f64.powi(self.exponent)
    }

    /// 加法实现见 [`std::ops::Add`]。
    fn plus(self, other: Self) -> Self {
        let precision = self.precision.max(other.precision);
        // 对齐到**较小**指数：把大指数那项的尾数乘 3^shift（精确），避免除法的精度损失。
        let (high, low, high_exp, low_exp) = if self.exponent >= other.exponent {
            (self.mantissa, other.mantissa, self.exponent, other.exponent)
        } else {
            (other.mantissa, self.mantissa, other.exponent, self.exponent)
        };
        let shift = high_exp - low_exp;
        if shift > 76 {
            // 差值超过 i128 的安全范围，较小项相对影响 < 3^-76，直接丢弃。
            return Self::new(high, high_exp, precision);
        }
        let boosted = high.saturating_mul(3i128.pow(shift as u32));
        Self::new(boosted.saturating_add(low), low_exp, precision)
    }

    /// 乘法实现见 [`std::ops::Mul`]。
    fn times(self, other: Self) -> Self {
        let precision = self.precision.max(other.precision);
        Self::new(
            self.mantissa.saturating_mul(other.mantissa),
            self.exponent + other.exponent,
            precision,
        )
    }

    /// 取反实现见 [`std::ops::Neg`]。
    fn flipped(self) -> Self {
        Self {
            mantissa: -self.mantissa,
            ..self
        }
    }

    /// 精度（有效三进制位数上限）。
    pub fn precision(self) -> u32 {
        self.precision
    }

    /// 尾数。
    pub fn mantissa(self) -> i128 {
        self.mantissa
    }

    /// 指数。
    pub fn exponent(self) -> i32 {
        self.exponent
    }

    /// 实际有效位数。
    pub fn significant_digits(self) -> u32 {
        digit_count(self.mantissa)
    }

    /// 这套精度占多少比特（`precision · log2 3`）。
    pub fn bits(self) -> f64 {
        self.precision as f64 * LOG2_3
    }

    /// 是否为零。
    pub fn is_zero(self) -> bool {
        self.mantissa == 0
    }
}

impl std::ops::Add for TritFloat {
    type Output = Self;
    /// 对齐到较小指数后相加，再按精度舍入。
    ///
    /// 指数差超过 76 时较小项低于 `i128` 的安全范围，直接丢弃（相对影响 `< 3^-76`）。
    fn add(self, other: Self) -> Self {
        self.plus(other)
    }
}

impl std::ops::Mul for TritFloat {
    type Output = Self;
    /// 尾数相乘、指数相加，再按精度舍入。
    fn mul(self, other: Self) -> Self {
        self.times(other)
    }
}

impl std::ops::Neg for TritFloat {
    type Output = Self;
    fn neg(self) -> Self {
        self.flipped()
    }
}

/// 相对误差上界的理论值：`3^{1-p}`。
pub fn relative_error_bound(precision: u32) -> f64 {
    3f64.powi(1 - precision as i32)
}

fn normalize(mut mantissa: i128, mut exponent: i32, precision: u32) -> (i128, i32) {
    if mantissa == 0 {
        return (0, 0);
    }
    while mantissa % 3 == 0 {
        mantissa /= 3;
        exponent += 1;
    }
    while digit_count(mantissa) > precision {
        mantissa = drop_digit(mantissa);
        exponent += 1;
        if mantissa == 0 {
            return (0, 0);
        }
        while mantissa % 3 == 0 {
            mantissa /= 3;
            exponent += 1;
        }
    }
    (mantissa, exponent)
}

/// 丢掉最低位并做最近舍入；平衡三进制的余数只有 `±1`，所以就是向零截断。
fn drop_digit(value: i128) -> i128 {
    let remainder = value.rem_euclid(3);
    let quotient = (value - remainder) / 3;
    if remainder <= 1 {
        quotient
    } else {
        quotient + 1
    }
}

fn digit_count(value: i128) -> u32 {
    let mut count = 0;
    let mut rest = value.abs();
    while rest > 0 {
        rest /= 3;
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_integers_round_trip_exactly() {
        for value in [0i64, 1, -1, 2, -2, 3, 9, -27, 13, 243] {
            let float = TritFloat::from_f64(value as f64, 20).unwrap();
            assert_eq!(float.to_f64(), value as f64, "{value}");
        }
    }

    #[test]
    fn arithmetic_is_exact_when_digits_fit() {
        let one = TritFloat::from_f64(1.0, 20).unwrap();
        let two = TritFloat::from_f64(2.0, 20).unwrap();
        let three = TritFloat::from_f64(3.0, 20).unwrap();
        assert_eq!((one + two).to_f64(), 3.0);
        assert_eq!((three * three).to_f64(), 9.0);
        assert_eq!((-one).to_f64(), -1.0);
        // 指数不同也要对：1 + 3 = 4。
        assert_eq!((one + three).to_f64(), 4.0);
    }

    #[test]
    fn rounding_is_nearest_in_balanced_ternary() {
        // 11 = 102_3，位数 3 > 2：最近的是 12（距离 1），不是 9（距离 2）。
        let rounded = TritFloat::new(11, 0, 2);
        assert_eq!(rounded.to_f64(), 12.0);
        assert!(rounded.significant_digits() <= 2);
    }

    #[test]
    fn trailing_zeros_are_stripped() {
        let float = TritFloat::new(9, 0, 2);
        assert_eq!(float.significant_digits(), 1);
        assert_eq!(float.to_f64(), 9.0);
    }

    #[test]
    fn relative_error_stays_under_the_bound() {
        for precision in [4u32, 6, 8] {
            let mut x = 0.37f64;
            for _ in 0..50 {
                let float = TritFloat::from_f64(x, precision).unwrap();
                let error = ((float.to_f64() - x) / x).abs();
                assert!(
                    error <= relative_error_bound(precision),
                    "p={precision} x={x} error={error}"
                );
                x = (x * 1.7 + 0.3) % 10.0;
            }
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        assert!(TritFloat::from_f64(f64::NAN, 8).is_none());
        assert!(TritFloat::from_f64(f64::INFINITY, 8).is_none());
    }

    #[test]
    fn precision_is_clamped() {
        let float = TritFloat::new(1, 0, 0);
        assert_eq!(float.precision(), MIN_PRECISION);
        let float = TritFloat::new(1, 0, 99);
        assert_eq!(float.precision(), MAX_PRECISION);
    }
}
