use harness_core::agg::{
    count_rows, count_values, div_int_or_empty, mean_skip_null_decimal, mean_skip_null_float,
    mean_skip_null_int, mean_strict_float, mean_strict_int, mean_zero_null_int, sum_decimal,
    sum_float, sum_int, sum_num, AggError, AggOutcome, Empty,
};
use harness_core::numeric::{Decimal, Float, Int, Num, NumError};

fn sample_values() -> Vec<Option<Int>> {
    vec![Some(Int(1)), None, Some(Int(3))]
}

#[test]
fn count_rows_and_values_differ() {
    let values = sample_values();
    assert_eq!(count_rows(&values), AggOutcome::Value(Int(3)));
    assert_eq!(count_values(&values), AggOutcome::Value(Int(2)));
}

#[test]
fn count_empty_returns_zero() {
    let empty: Vec<Option<Int>> = vec![];
    assert_eq!(count_rows(&empty), AggOutcome::Value(Int(0)));
    assert_eq!(count_values(&empty), AggOutcome::Value(Int(0)));
}

#[test]
fn mean_skip_null_excludes_nulls() {
    let values = sample_values();
    match mean_skip_null_int(&values).unwrap() {
        AggOutcome::Value(decimal) => {
            assert_eq!(decimal.scale, 6);
            assert_eq!(decimal.mantissa, 2_000_000);
        }
        AggOutcome::Empty(_) => panic!("expected value"),
    }

    let floats = vec![Some(Float(1.0)), None, Some(Float(3.0))];
    match mean_skip_null_float(&floats).unwrap() {
        AggOutcome::Value(mean) => assert_eq!(mean.0, 2.0),
        AggOutcome::Empty(_) => panic!("expected value"),
    }
}

#[test]
fn mean_strict_rejects_null() {
    let values = sample_values();
    assert_eq!(mean_strict_int(&values), Err(AggError::NullEncountered));
    let floats = vec![Some(Float(1.0)), None, Some(Float(3.0))];
    assert_eq!(mean_strict_float(&floats), Err(AggError::NullEncountered));
}

#[test]
fn the_adversarial_null_case_from_the_golden_set() {
    let values = sample_values();
    assert_eq!(
        mean_skip_null_int(&values),
        Ok(AggOutcome::Value(Decimal {
            scale: 6,
            mantissa: 2_000_000
        }))
    );
    assert_eq!(mean_strict_int(&values), Err(AggError::NullEncountered));
    assert_eq!(
        mean_zero_null_int(&values),
        Ok(AggOutcome::Value(Decimal {
            scale: 6,
            mantissa: 1_333_333
        }))
    );
}

#[test]
fn mean_all_null_is_empty_not_zero() {
    let values = vec![None, None];
    match mean_skip_null_int(&values).unwrap() {
        AggOutcome::Empty(empty) => assert_eq!(empty.reason, "no non-null values"),
        AggOutcome::Value(_) => panic!("expected Empty"),
    }
}

#[test]
fn empty_never_coerces_to_zero_in_div() {
    let empty = AggOutcome::Empty(Empty {
        reason: "no records matched".into(),
    });
    match div_int_or_empty(empty, Int(5)).unwrap() {
        AggOutcome::Empty(e) => assert_eq!(e.reason, "no records matched"),
        AggOutcome::Value(_) => panic!("expected Empty propagation"),
    }
}

#[test]
fn sum_int_is_exact() {
    assert_eq!(sum_int(&[Int(1), Int(2), Int(3)]).unwrap(), Int(6));
    assert_eq!(
        sum_int(&[Int(9_007_199_254_740_992), Int(1)]).unwrap(),
        Int(9_007_199_254_740_993)
    );
}

#[test]
fn decimal_sum_and_mean_are_exact_at_their_declared_scale() {
    assert_eq!(
        sum_decimal(&[
            Decimal {
                scale: 2,
                mantissa: 10
            },
            Decimal {
                scale: 1,
                mantissa: 2
            },
        ])
        .unwrap(),
        Decimal {
            scale: 2,
            mantissa: 30
        }
    );
    assert_eq!(
        mean_skip_null_decimal(&[
            Some(Decimal {
                scale: 2,
                mantissa: 100
            }),
            Some(Decimal {
                scale: 2,
                mantissa: 200
            }),
        ])
        .unwrap(),
        AggOutcome::Value(Decimal {
            scale: 8,
            mantissa: 150_000_000
        })
    );
}

#[test]
fn sum_float_is_pairwise_by_index() {
    let values = vec![Float(1.0), Float(2.0), Float(3.0)];
    assert_eq!(sum_float(&values).unwrap().0, 6.0);
}

#[test]
fn float_sum_depends_on_order_which_is_why_ordering_is_mandated() {
    let ordered = [Float(1e16), Float(1.0), Float(-1e16), Float(1.0)];
    let reordered = [Float(1e16), Float(-1e16), Float(1.0), Float(1.0)];
    assert_ne!(sum_float(&ordered).unwrap(), sum_float(&reordered).unwrap());
}

#[test]
fn pairwise_beats_naive_accumulation_on_error() {
    let mut values = vec![Float(1e16)];
    values.extend(vec![Float(1.0); 10_000]);
    values.push(Float(-1e16));
    let naive = values.iter().fold(0.0, |acc, value| acc + value.0);
    let pairwise = sum_float(&values).unwrap().0;
    assert!(
        (pairwise - 10_000.0).abs() < (naive - 10_000.0).abs(),
        "pairwise={pairwise}, naive={naive}"
    );
}

#[test]
fn mixed_numeric_types_in_one_column_are_rejected() {
    let err = sum_num(&[Num::Int(Int(1)), Num::Float(Float(2.0))]).unwrap_err();
    assert!(matches!(
        err,
        AggError::Numeric(NumError::TypeMismatch { op: "add", .. })
    ));
}

#[test]
fn mean_skip_null_equiv_sum_over_count_values() {
    let values = sample_values();
    let total = sum_int(&[Int(1), Int(3)]).unwrap();
    let count = match count_values(&values) {
        AggOutcome::Value(n) => n,
        AggOutcome::Empty(_) => panic!("count_values should not be empty"),
    };
    let mean = match div_int_or_empty(AggOutcome::Value(total), count).unwrap() {
        AggOutcome::Value(v) => v,
        AggOutcome::Empty(_) => panic!("unexpected empty"),
    };
    let direct = match mean_skip_null_int(&values).unwrap() {
        AggOutcome::Value(v) => v,
        AggOutcome::Empty(_) => panic!("unexpected empty"),
    };
    assert_eq!(mean, direct);
}
