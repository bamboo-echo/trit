//! 编解码高压不变量：随机长度、全部字节值、随机 i64、随机单符号错误。

use trit_codec::{CodecError, balanced, hamming, pack};
use trit_core::value::Ternary;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn trit(&mut self) -> Ternary {
        match self.next() % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        }
    }
}

#[test]
fn pack_round_trips_every_short_length() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for len in 0..=256usize {
        let values: Vec<Ternary> = (0..len).map(|_| rng.trit()).collect();
        let bytes = pack::pack(&values);
        assert_eq!(bytes.len(), len.div_ceil(pack::TRITS_PER_BYTE), "len={len}");
        assert_eq!(pack::unpack(&bytes, len).unwrap(), values, "len={len}");
    }
}

#[test]
fn pack_round_trips_large_payload() {
    let mut rng = Rng(7);
    let values: Vec<Ternary> = (0..100_000).map(|_| rng.trit()).collect();
    let bytes = pack::pack(&values);
    assert_eq!(pack::unpack(&bytes, values.len()).unwrap(), values);
}

#[test]
fn every_byte_value_is_classified() {
    for byte in 0..=255u8 {
        let result = pack::unpack(&[byte], 5);
        if u16::from(byte) >= pack::COMBINATIONS {
            assert!(matches!(result, Err(CodecError::InvalidByte(_))), "{byte}");
        } else {
            assert!(result.is_ok(), "{byte}");
        }
    }
}

#[test]
fn balanced_round_trips_random_i64() {
    let mut rng = Rng(0xBEEF);
    let mut values: Vec<i64> = (0..2000).map(|_| rng.next() as i64).collect();
    values.extend([i64::MIN, i64::MAX, 0, 1, -1]);
    for value in values {
        let text = balanced::encode(value);
        assert_eq!(balanced::decode(&text).unwrap(), value, "{text}");
        assert!(text.chars().count() <= 41, "{value} -> {text}");
    }
}

#[test]
fn hamming_corrects_random_single_errors() {
    let mut rng = Rng(0x1234_5678);
    let trits = [Ternary::False, Ternary::Unknown, Ternary::True];
    for _ in 0..200 {
        let message = [
            trits[(rng.next() % 3) as usize],
            trits[(rng.next() % 3) as usize],
        ];
        let word = hamming::encode(message[0], message[1]);
        for position in 0..hamming::CODEWORD {
            for magnitude in 1..=2u8 {
                let mut corrupted = word;
                let digit = match corrupted[position] {
                    Ternary::False => 0,
                    Ternary::Unknown => 1,
                    Ternary::True => 2,
                };
                corrupted[position] = match (digit + i32::from(magnitude)) % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                };
                let decoded = hamming::decode(corrupted);
                assert_eq!(decoded.message, message, "位置 {position} 幅度 {magnitude}");
                assert_eq!(decoded.corrected, Some((position, magnitude)));
            }
        }
    }
}
