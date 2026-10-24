//! 三值差分编码（delta coding, mod 3）。
//!
//! 不直接存原序列，而存「第一个 trit + 每相邻两位的差值 mod 3」：
//!
//! - `d[0] = t[0]`；
//! - `d[i] = (t[i] - t[i-1]) mod 3`，i ≥ 1。
//!
//! 对**缓变序列**（相邻 trit 经常相等 → 差值 0 = `False`，出现频次高）效果好；
//! 逐位乱跳的序列差值也是均匀分布的三个值，没有压缩收益。
//!
//! 算术约定：`Ternary` 序号 False=0 / Unknown=1 / True=2，mod 3 的减法/加法一律用
//! `(a.index() as i8 - b.index() as i8).rem_euclid(3)`，**不要**用裸 `u8` 减法
//! （`0 - 1` 会下溢 panic）。加法还原同理：`(a.index() + b.index()) % 3` 对
//! `usize` 不会下溢，可以直接取模。
//!
//! 可逆性：任何合法 trit 序列都能编码，任何长度的合法 trit delta 序列都能逐位
//! 累加还原——没有非法 delta 输入，所以本模块不返回 `CodecError`。

use trit_core::value::Ternary;

/// 编码：首 trit 原样存，之后每位存「与前一位的差值 mod 3」。
///
/// 空输入返回空列表。
pub fn encode(values: &[Ternary]) -> Vec<Ternary> {
    let mut out = Vec::with_capacity(values.len());
    let mut prev: Option<Ternary> = None;
    for &v in values {
        let d = match prev {
            None => v,
            Some(p) => {
                let diff = (v.index() as i8 - p.index() as i8).rem_euclid(3) as usize;
                Ternary::from_index(diff)
            }
        };
        out.push(d);
        prev = Some(v);
    }
    out
}

/// 解码：首 trit 即为原首 trit，之后每位 = 前一位累加 delta mod 3。
///
/// 空输入返回空列表。任何长度的合法 trit delta 序列都可逆。
pub fn decode(deltas: &[Ternary]) -> Vec<Ternary> {
    let mut out = Vec::with_capacity(deltas.len());
    let mut acc: Option<usize> = None;
    for &d in deltas {
        let cur = match acc {
            None => d.index(),
            Some(prev) => (prev + d.index()) % 3,
        };
        out.push(Ternary::from_index(cur));
        acc = Some(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(chars: &[char]) -> Vec<Ternary> {
        chars.iter().map(|c| Ternary::parse_char(*c).unwrap()).collect()
    }

    #[test]
    fn empty_round_trips() {
        assert!(encode(&[]).is_empty());
        assert!(decode(&[]).is_empty());
    }

    #[test]
    fn single_trit_is_its_own_delta() {
        let values = v(&['1']);
        assert_eq!(encode(&values), values);
        assert_eq!(decode(&values), values);
    }

    #[test]
    fn hand_known_example() {
        // t = 0(0), X(1), 1(2), 0(0), X(1)
        // d[0] = 0
        // d[1] = (1-0) mod 3 = 1 = X
        // d[2] = (2-1) mod 3 = 1 = X
        // d[3] = (0-2) mod 3 = 1 = X
        // d[4] = (1-0) mod 3 = 1 = X
        let values = v(&['0', 'X', '1', '0', 'X']);
        let d = encode(&values);
        assert_eq!(d, v(&['0', 'X', 'X', 'X', 'X']));
        assert_eq!(decode(&d), values);
    }

    #[test]
    fn zero_delta_for_constant_run() {
        // 全 True 序列：首 True，后续差值全 0 = False。
        let values = vec![Ternary::True; 5];
        let d = encode(&values);
        assert_eq!(d, vec![Ternary::True, Ternary::False, Ternary::False, Ternary::False, Ternary::False]);
        assert_eq!(decode(&d), values);
    }

    #[test]
    fn wrap_around_mod_3_is_handled() {
        // t = True(2) → False(0)：差值 (0-2) mod 3 = 1 = Unknown。
        let values = v(&['1', '0']);
        let d = encode(&values);
        assert_eq!(d, v(&['1', 'X']));
        assert_eq!(decode(&d), values);
    }

    #[test]
    fn round_trips_every_short_length() {
        let mut seq = Vec::new();
        for len in 0..=40usize {
            //  deterministic 小序列：按 0,1,2,0,1,2,… 缓变
            seq.clear();
            for i in 0..len {
                seq.push(Ternary::from_index(i % 3));
            }
            let d = encode(&seq);
            assert_eq!(d.len(), len);
            assert_eq!(decode(&d), seq, "len={len}");
        }
    }

    #[test]
    fn arbitrary_deltas_are_all_decodable() {
        // 任何合法 trit delta 序列都可逆——逐位累加 mod 3 必然回到某个合法 trit。
        let deltas = v(&['X', '0', '1', 'X', 'X', '0']);
        let back = decode(&deltas);
        assert_eq!(encode(&back), deltas);
    }
}
