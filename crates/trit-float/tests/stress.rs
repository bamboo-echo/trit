//! 浮点高压不变量：全精度档 × 全指数范围，相对误差不得越过 `3^{1-p}`。

use trit_float::{TritFloat, relative_error_bound};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0
    }
    fn signed(&mut self, precision: u32) -> Option<TritFloat> {
        let mantissa = 1.0 + self.unit() * 2.0;
        let exponent = (self.next() % 81) as i32 - 40;
        let sign = if self.next().is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        let value = sign * mantissa * 3f64.powi(exponent);
        TritFloat::from_f64(value, precision)
    }
}

#[test]
fn quantization_error_never_exceeds_the_bound() {
    for precision in [2u32, 4, 6, 9, 12, 18] {
        let mut rng = Rng(0xBEEF ^ u64::from(precision));
        let bound = relative_error_bound(precision);
        for _ in 0..1000 {
            let mantissa = 1.0 + rng.unit() * 2.0;
            let exponent = (rng.next() % 81) as i32 - 40;
            let sign = if rng.next().is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let value = sign * mantissa * 3f64.powi(exponent);
            let rounded = TritFloat::from_f64(value, precision)
                .expect("有限值")
                .to_f64();
            let error = ((rounded - value) / value).abs();
            assert!(error <= bound, "p={precision} value={value} error={error}");
        }
    }
}

#[test]
fn product_error_is_at_most_twice_the_bound() {
    let mut rng = Rng(0xFEED);
    for precision in [6u32, 12, 18] {
        let bound = relative_error_bound(precision);
        for _ in 0..200 {
            let x = rng.signed(precision).expect("有限值").to_f64();
            let y = rng.signed(precision).expect("有限值").to_f64();
            let qx = TritFloat::from_f64(x, precision).expect("有限值");
            let qy = TritFloat::from_f64(y, precision).expect("有限值");
            let got = (qx * qy).to_f64();
            let exact = x * y;
            if exact == 0.0 || !exact.is_finite() {
                continue;
            }
            let error = ((got - exact) / exact).abs();
            assert!(
                error <= 2.5 * bound,
                "p={precision} x={x} y={y} error={error}"
            );
        }
    }
}
