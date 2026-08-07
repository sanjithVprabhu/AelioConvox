use harness_core::numeric::{add, div, mul, sub, Decimal, Float, Int, Num, NumError, RoundingMode};

#[test]
fn int_div_returns_decimal_not_int() {
    let result = Int(1).div(Int(3)).unwrap();
    assert_eq!(result.scale, 6);
    assert_eq!(result.mantissa, 333_333);
}

#[test]
fn add_int_float_is_type_error() {
    let err = add(Num::Int(Int(1)), Num::Float(Float(1.0))).unwrap_err();
    assert!(matches!(err, NumError::TypeMismatch { op: "add", .. }));
}

#[test]
fn decimal_addition_is_exact_where_float_is_not() {
    let decimal_sum = Decimal {
        scale: 1,
        mantissa: 1,
    }
    .add(Decimal {
        scale: 1,
        mantissa: 2,
    })
    .unwrap();
    assert_eq!(
        decimal_sum,
        Decimal {
            scale: 1,
            mantissa: 3
        }
    );
    assert_ne!(0.1_f64 + 0.2_f64, 0.3_f64);
}

#[test]
fn float_rejects_non_finite() {
    assert!(Float::new(f64::NAN).is_err());
    assert!(Float::new(f64::INFINITY).is_err());
    assert!(Float::new(f64::NEG_INFINITY).is_err());
}

#[test]
fn float_normalizes_negative_zero() {
    let value = Float::new(-0.0).unwrap();
    assert_eq!(value.0, 0.0);
}

#[test]
fn int_arithmetic_is_exact() {
    assert_eq!(Int(2).add(Int(3)).unwrap(), Int(5));
    assert_eq!(Int(10).sub(Int(4)).unwrap(), Int(6));
    assert_eq!(Int(6).mul(Int(7)).unwrap(), Int(42));
    assert_eq!(Int(7).div_floor(Int(3)).unwrap(), Int(2));
}

#[test]
fn decimal_bankers_rounding_half_to_even() {
    let a = Decimal {
        scale: 1,
        mantissa: 25,
    };
    let b = Decimal {
        scale: 1,
        mantissa: 10,
    };
    // 2.5 / 1.0 at scale 0 → 2 (round half to even)
    let result = a.div(b, 0, RoundingMode::HalfEven).unwrap();
    assert_eq!(result.mantissa, 2);

    let c = Decimal {
        scale: 1,
        mantissa: 35,
    };
    // 3.5 / 1.0 at scale 0 → 4
    let result = c.div(b, 0, RoundingMode::HalfEven).unwrap();
    assert_eq!(result.mantissa, 4);

    let negative_one = Decimal {
        scale: 0,
        mantissa: -1,
    };
    let negative_three = Decimal {
        scale: 0,
        mantissa: -3,
    };
    let two = Decimal {
        scale: 0,
        mantissa: 2,
    };
    assert_eq!(
        negative_one
            .div(two, 0, RoundingMode::HalfEven)
            .unwrap()
            .mantissa,
        0
    );
    assert_eq!(
        negative_three
            .div(two, 0, RoundingMode::HalfEven)
            .unwrap()
            .mantissa,
        -2
    );
}

#[test]
fn div_by_zero_is_err() {
    assert!(matches!(Int(1).div(Int(0)), Err(NumError::DivByZero)));
    assert!(matches!(
        Float(1.0).div(Float(0.0)),
        Err(NumError::DivByZero)
    ));
    assert!(matches!(
        Decimal {
            scale: 0,
            mantissa: 1
        }
        .div(
            Decimal {
                scale: 0,
                mantissa: 0
            },
            0,
            RoundingMode::HalfEven
        ),
        Err(NumError::DivByZero)
    ));
}

#[test]
fn decimal_add_aligns_scales() {
    let a = Decimal {
        scale: 2,
        mantissa: 150,
    };
    let b = Decimal {
        scale: 1,
        mantissa: 25,
    };
    let sum = a.add(b).unwrap();
    assert_eq!(sum.scale, 2);
    assert_eq!(sum.mantissa, 400);
}

#[test]
fn decimal_ordering_across_scales() {
    let one = Decimal {
        scale: 0,
        mantissa: 1,
    };
    let one_hundredths = Decimal {
        scale: 2,
        mantissa: 100,
    };
    let greater = Decimal {
        scale: 2,
        mantissa: 101,
    };
    assert_eq!(one, one_hundredths);
    assert!(one < greater);
    assert_eq!(
        one.cmp_value(one_hundredths).unwrap(),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn decimal_division_scale_has_a_six_place_floor_and_clamps_at_max() {
    let result = div(
        Num::Decimal(Decimal {
            scale: 1,
            mantissa: 10,
        }),
        Num::Decimal(Decimal {
            scale: 1,
            mantissa: 10,
        }),
    )
    .unwrap();
    match result {
        Num::Decimal(value) => assert_eq!(value.scale, 6),
        _ => panic!("expected decimal"),
    }

    let at_limit = Decimal {
        scale: Decimal::MAX_SCALE,
        mantissa: 1,
    };
    let result = div(Num::Decimal(at_limit), Num::Decimal(at_limit)).unwrap();
    match result {
        Num::Decimal(value) => assert_eq!(value.scale, Decimal::MAX_SCALE),
        _ => panic!("expected decimal"),
    }
    assert!(matches!(
        Decimal::from_int(Int(1), Decimal::MAX_SCALE + 1),
        Err(NumError::ScaleTooLarge)
    ));
}

#[test]
fn numeric_boundary_failures_are_errors() {
    assert!(matches!(Int(i64::MAX).add(Int(1)), Err(NumError::Overflow)));
    assert!(matches!(
        Decimal {
            scale: 0,
            mantissa: i128::MAX
        }
        .add(Decimal {
            scale: 0,
            mantissa: 1
        }),
        Err(NumError::Overflow)
    ));
    assert!(matches!(
        Decimal {
            scale: 0,
            mantissa: i128::MIN
        }
        .div(
            Decimal {
                scale: 0,
                mantissa: 1
            },
            0,
            RoundingMode::HalfEven
        ),
        Err(NumError::Overflow)
    ));
}

#[test]
fn type_strict_mul_and_sub() {
    assert!(matches!(
        mul(Num::Int(Int(2)), Num::Float(Float(3.0))),
        Err(NumError::TypeMismatch { op: "mul", .. })
    ));
    assert!(matches!(
        sub(
            Num::Decimal(Decimal {
                scale: 0,
                mantissa: 1
            }),
            Num::Int(Int(1))
        ),
        Err(NumError::TypeMismatch { op: "sub", .. })
    ));
}

#[test]
fn div_int_int_via_num_dispatch() {
    match div(Num::Int(Int(10)), Num::Int(Int(4))).unwrap() {
        Num::Decimal(d) => assert_eq!(d.mantissa, 2_500_000),
        other => panic!("expected Decimal, got {other:?}"),
    }
}
