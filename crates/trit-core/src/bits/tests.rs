use super::*;

fn tri(v: Ternary) -> TriBit {
    TriBit::constant(v)
}

#[test]
fn not_matches_scalar() {
    for a in Ternary::ALL {
        assert_eq!(tri(a).not().get(0), Algebra::Kleene.not(a), "NOT {a}");
    }
}

#[test]
fn and_or_match_scalar_for_all_algebras() {
    for alg in Algebra::ALL {
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                assert_eq!(
                    tri(a).and(tri(b), alg).get(0),
                    alg.and(a, b),
                    "{alg} {a}&{b}"
                );
                assert_eq!(tri(a).or(tri(b), alg).get(0), alg.or(a, b), "{alg} {a}|{b}");
            }
        }
    }
}

#[test]
fn strong_add_matches_scalar_for_all_algebras() {
    for alg in Algebra::ALL {
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                assert_eq!(
                    tri(a).strong_add(tri(b), alg).get(0),
                    alg.strong_add(a, b),
                    "{alg} {a}+{b}"
                );
            }
        }
    }
}

#[test]
fn strong_and_matches_scalar_for_all_algebras() {
    for alg in Algebra::ALL {
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                assert_eq!(
                    tri(a).strong_and(tri(b), alg).get(0),
                    alg.strong_and(a, b),
                    "{alg} {a}*{b}"
                );
            }
        }
    }
}

#[test]
fn ops_never_produce_illegal_rails() {
    for alg in Algebra::ALL {
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for r in [
                    tri(a).and(tri(b), alg),
                    tri(a).or(tri(b), alg),
                    tri(a).strong_add(tri(b), alg),
                ] {
                    assert_eq!(r.ones.0 & r.unknowns.0, 0, "{alg} {a} {b} 产生了 (1,1)");
                }
            }
        }
    }
}

#[test]
fn implies_matches_scalar_for_all_algebras() {
    for alg in Algebra::ALL {
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                assert_eq!(
                    tri(a).implies(tri(b), alg).get(0),
                    alg.implies(a, b),
                    "{alg} {a}=>{b}"
                );
            }
        }
    }
}

#[test]
fn equiv_matches_scalar_for_all_algebras() {
    for alg in Algebra::ALL {
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                assert_eq!(
                    tri(a).equiv(tri(b), alg).get(0),
                    alg.equiv(a, b),
                    "{alg} {a}<=>{b}"
                );
            }
        }
    }
}

#[test]
fn bitwise_ops_agree_across_64_lanes() {
    // 构造一个混合 0/1/X 的 64 位平面，逐位与标量结果比对
    let mut ones = 0u64;
    let mut unknowns = 0u64;
    for i in 0..64u32 {
        match i % 3 {
            1 => ones |= 1u64 << i,
            2 => unknowns |= 1u64 << i,
            _ => {}
        }
    }
    let t = TriBit {
        ones: BitPlane(ones),
        unknowns: BitPlane(unknowns),
    };
    let other = TriBit {
        ones: BitPlane(0x5555_5555_5555_5555),
        unknowns: BitPlane(0x0f0f_0f0f_0f0f_0f0f),
    };
    for alg in Algebra::ALL {
        let got = t.and(other, alg);
        for i in 0..64u32 {
            assert_eq!(
                got.get(i),
                alg.and(t.get(i), other.get(i)),
                "{alg} lane {i}"
            );
        }
    }
}

#[test]
fn evaluator_covers_all_assignments() {
    let mut ev = Evaluator::new(Algebra::Kleene);
    ev.declare("a");
    ev.declare("b");
    assert_eq!(ev.assignment_count().unwrap(), 9);
    let tt = ev.evaluate(|p| p[0]).unwrap();
    assert_eq!(tt.values.len(), 9);
    // 三进制混合序：第 0 组 a=0，第 1 组 a=1，第 2 组 a=X
    assert_eq!(tt.value_at(0), Some(Ternary::False));
    assert_eq!(tt.value_at(1), Some(Ternary::True));
    assert_eq!(tt.value_at(2), Some(Ternary::Unknown));
    assert_eq!(tt.value_at(3), Some(Ternary::False));
}

#[test]
fn assignment_count_is_checked_not_saturated() {
    let mut ev = Evaluator::new(Algebra::Kleene);
    for i in 0..41 {
        ev.declare(format!("v{i}"));
    }
    assert_eq!(
        ev.assignment_count().unwrap_err(),
        LimitError::CountOverflow { vars: 41 }
    );
}

#[test]
fn chunk_base_must_be_aligned_and_not_overflow() {
    let mut ev = Evaluator::new(Algebra::Kleene);
    ev.declare("a");
    assert_eq!(
        ev.chunk_planes(1).unwrap_err(),
        LimitError::ChunkBase { base: 1 }
    );
    assert!(matches!(
        ev.chunk_planes(u64::MAX - 1).unwrap_err(),
        LimitError::ChunkBase { .. }
    ));
    assert!(ev.chunk_planes(0).is_ok());
}

#[test]
fn evaluate_enforces_row_budget_before_allocating() {
    let mut ev = Evaluator::new(Algebra::Kleene);
    ev.declare("a");
    let limits = Limits {
        max_rows: 2,
        ..Limits::default()
    };
    assert_eq!(
        ev.evaluate_within(|p| p[0], limits).unwrap_err(),
        LimitError::Rows { limit: 2 }
    );
}

#[test]
fn evaluate_within_enforces_var_budget_directly() {
    let mut ev = Evaluator::new(Algebra::Kleene);
    for i in 0..13 {
        ev.declare(format!("v{i}"));
    }
    let limits = Limits {
        max_vars: 12,
        ..Limits::default()
    };
    assert_eq!(
        ev.evaluate_within(|_| TriBit::FALSE, limits).unwrap_err(),
        LimitError::Vars { limit: 12 }
    );
}

#[test]
fn bit_plane_checked_access_rejects_lane_overflow() {
    let plane = BitPlane::with_bit(3);
    assert_eq!(BitPlane::checked_with_bit(64), None);
    assert_eq!(plane.checked_bit(3), Some(true));
    assert_eq!(plane.checked_bit(64), None);
    // 环绕语义保留在未检查版本里
    assert_eq!(BitPlane::with_bit(64), BitPlane::with_bit(0));
    let tri = TriBit::from_binary(0b1);
    assert_eq!(tri.checked_get(0), Some(Ternary::True));
    assert_eq!(tri.checked_get(64), None);
}

#[test]
fn checked_bit_access_covers_all_boundaries() {
    for bit in [0u32, 63] {
        assert_eq!(BitPlane::checked_with_bit(bit), Some(BitPlane(1u64 << bit)));
    }
    for bit in [64u32, 65, u32::MAX] {
        assert_eq!(BitPlane::checked_with_bit(bit), None);
        assert_eq!(BitPlane::ONES.checked_bit(bit), None);
    }
    assert_eq!(BitPlane::ONES.checked_bit(63), Some(true));
}

#[test]
fn evaluator_with_zero_variables_has_one_row() {
    let ev = Evaluator::new(Algebra::Kleene);
    assert_eq!(ev.assignment_count().unwrap(), 1);
    let tt = ev.evaluate(|_| TriBit::TRUE).unwrap();
    assert!(tt.variables.is_empty());
    assert_eq!(tt.values, vec![Ternary::True]);
}

#[test]
fn evaluator_spans_multiple_chunks() {
    // 4 个变量 → 81 组赋值，跨越 64 组边界
    let mut ev = Evaluator::new(Algebra::Kleene);
    for name in ["a", "b", "c", "d"] {
        ev.declare(name);
    }
    assert_eq!(ev.assignment_count().unwrap(), 81);
    let tt = ev.evaluate(|p| p[3]).unwrap();
    assert_eq!(tt.values.len(), 81);
    for (i, v) in tt.values.iter().enumerate() {
        let digit = (i as u64 / 27) % 3;
        let expect = match digit {
            0 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        };
        assert_eq!(*v, expect, "group {i}");
    }
}
