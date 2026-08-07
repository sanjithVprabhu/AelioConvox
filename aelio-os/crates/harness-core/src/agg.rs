//! HKv4 null-explicit aggregates and empty-vs-error (§2.2–2.5).

use crate::hash::hash_bytes;
use crate::numeric::{add, Decimal, Float, Int, Num, NumError, RoundingMode};

/// Well-defined absence of matching records — never coerces to zero (§2.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Empty {
    pub reason: String,
}

/// Three outcomes distinguished to the renderer (§2.5).
#[derive(Debug, Clone, PartialEq)]
pub enum AggOutcome<T> {
    Value(T),
    Empty(Empty),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggError {
    /// Strict aggregates distinguish encountered data absence from arithmetic
    /// failures so renderers can explain the rejected column.
    NullEncountered,
    DivByZero,
    Numeric(NumError),
    EmptyInput,
}

pub type AggResult<T> = Result<AggOutcome<T>, AggError>;

const MEAN_INT_SCALE: u32 = 6;

/// Cardinality of the collection (§2.4). `[1, null, 3]` → `3`.
pub fn count_rows<T>(items: &[T]) -> AggOutcome<Int> {
    AggOutcome::Value(Int(items.len() as i64))
}

/// Non-null values in a projected field (§2.4). `[1, null, 3]` → `2`.
pub fn count_values<T>(items: &[Option<T>]) -> AggOutcome<Int> {
    AggOutcome::Value(Int(
        items.iter().filter(|value| value.is_some()).count() as i64
    ))
}

/// Mean excluding nulls over integers — returns `Decimal{6}` (§2.2).
pub fn mean_skip_null_int(values: &[Option<Int>]) -> AggResult<Decimal> {
    let non_null: Vec<Int> = values.iter().filter_map(|v| *v).collect();
    if non_null.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no non-null values".into(),
        }));
    }
    let total = sum_int(&non_null)?;
    let count = Int(non_null.len() as i64);
    let decimal_total = total.to_decimal(0).map_err(AggError::Numeric)?;
    let decimal_count = count.to_decimal(0).map_err(AggError::Numeric)?;
    let mean = decimal_total
        .div(decimal_count, MEAN_INT_SCALE, RoundingMode::HalfEven)
        .map_err(AggError::Numeric)?;
    Ok(AggOutcome::Value(mean))
}

/// Mean excluding nulls over floats (§2.3).
pub fn mean_skip_null_float(values: &[Option<Float>]) -> AggResult<Float> {
    let non_null: Vec<Float> = values.iter().filter_map(|v| *v).collect();
    if non_null.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no non-null values".into(),
        }));
    }
    let total = sum_float(&non_null)?;
    let count = non_null.len() as f64;
    total
        .div(Float(count))
        .map(AggOutcome::Value)
        .map_err(AggError::Numeric)
}

/// Mean where nulls are explicit zeroes and remain in the denominator.
pub fn mean_zero_null_int(values: &[Option<Int>]) -> AggResult<Decimal> {
    if values.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no values".into(),
        }));
    }
    let total = values.iter().try_fold(Int::ZERO, |acc, value| {
        acc.add(value.unwrap_or(Int::ZERO))
            .map_err(AggError::Numeric)
    })?;
    mean_int_total(total, values.len())
}

/// Mean where nulls are explicit zeroes and remain in the denominator.
pub fn mean_zero_null_float(values: &[Option<Float>]) -> AggResult<Float> {
    if values.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no values".into(),
        }));
    }
    let total = sum_float(
        &values
            .iter()
            .map(|value| value.unwrap_or(Float(0.0)))
            .collect::<Vec<_>>(),
    )?;
    total
        .div(Float(values.len() as f64))
        .map(AggOutcome::Value)
        .map_err(AggError::Numeric)
}

/// Mean requiring all values — any null is a distinct refusal (§2.3).
pub fn mean_strict_int(values: &[Option<Int>]) -> AggResult<Decimal> {
    if values.iter().any(|value| value.is_none()) {
        return Err(AggError::NullEncountered);
    }
    let items: Vec<Int> = values.iter().filter_map(|v| *v).collect();
    if items.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no values".into(),
        }));
    }
    let total = sum_int(&items)?;
    mean_int_total(total, items.len())
}

/// Mean requiring all values — any null is a distinct refusal (§2.3).
pub fn mean_strict_float(values: &[Option<Float>]) -> AggResult<Float> {
    if values.iter().any(|value| value.is_none()) {
        return Err(AggError::NullEncountered);
    }
    let items: Vec<Float> = values.iter().filter_map(|v| *v).collect();
    if items.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no values".into(),
        }));
    }
    let total = sum_float(&items)?;
    let count = items.len() as f64;
    total
        .div(Float(count))
        .map(AggOutcome::Value)
        .map_err(AggError::Numeric)
}

/// Exact integer sum.
pub fn sum_int(values: &[Int]) -> Result<Int, AggError> {
    values.iter().copied().try_fold(Int(0), |acc, value| {
        acc.add(value).map_err(AggError::Numeric)
    })
}

/// Exact decimal sum, retaining the highest input scale.
pub fn sum_decimal(values: &[Decimal]) -> Result<Decimal, AggError> {
    values.iter().copied().try_fold(
        Decimal {
            scale: 0,
            mantissa: 0,
        },
        |acc, value| acc.add(value).map_err(AggError::Numeric),
    )
}

/// Aggregate dispatch preserves the numeric tower: a heterogeneous column is
/// rejected instead of silently widening or truncating values.
pub fn sum_num(values: &[Num]) -> Result<Num, AggError> {
    let (first, rest) = values.split_first().ok_or(AggError::EmptyInput)?;
    rest.iter().copied().try_fold(*first, |acc, value| {
        add(acc, value).map_err(AggError::Numeric)
    })
}

/// Pairwise float sum with recursion order fixed by input index (§6.3).
pub fn sum_float(values: &[Float]) -> Result<Float, AggError> {
    let raw = pairwise_sum_f64(values.iter().map(|v| v.0).collect::<Vec<_>>().as_slice());
    Float::new(raw).map_err(AggError::Numeric)
}

/// Exact decimal mean excluding nulls. Its result scale is `input_scale + 6`,
/// clamped to [`Decimal::MAX_SCALE`].
pub fn mean_skip_null_decimal(values: &[Option<Decimal>]) -> AggResult<Decimal> {
    let non_null: Vec<Decimal> = values.iter().filter_map(|value| *value).collect();
    if non_null.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no non-null values".into(),
        }));
    }
    mean_decimal_total(sum_decimal(&non_null)?, non_null.len())
}

/// Exact decimal mean requiring every row to contain a value.
pub fn mean_strict_decimal(values: &[Option<Decimal>]) -> AggResult<Decimal> {
    if values.iter().any(|value| value.is_none()) {
        return Err(AggError::NullEncountered);
    }
    let items: Vec<Decimal> = values.iter().filter_map(|value| *value).collect();
    if items.is_empty() {
        return Ok(AggOutcome::Empty(Empty {
            reason: "no values".into(),
        }));
    }
    mean_decimal_total(sum_decimal(&items)?, items.len())
}

/// Exact decimal mean treating nulls as zeroes.
pub fn mean_zero_null_decimal(values: &[Option<Decimal>]) -> AggResult<Decimal> {
    let Some(scale) = values.iter().flatten().map(|value| value.scale).max() else {
        return if values.is_empty() {
            Ok(AggOutcome::Empty(Empty {
                reason: "no values".into(),
            }))
        } else {
            Ok(AggOutcome::Value(Decimal {
                scale: MEAN_INT_SCALE,
                mantissa: 0,
            }))
        };
    };
    let total = sum_decimal(
        &values
            .iter()
            .map(|value| value.unwrap_or(Decimal { scale, mantissa: 0 }))
            .collect::<Vec<_>>(),
    )?;
    mean_decimal_total(total, values.len())
}

/// Reference vector for determinism pinning — 10_000-element pairwise sum input.
pub fn float_sum_reference_input() -> Vec<f64> {
    (0..10_000).map(|i| (i as f64 * 0.001) + 0.0001).collect()
}

/// Hash the reference pairwise sum for cross-profile/architecture parity tests.
pub fn float_sum_reference_hash() -> String {
    let input = float_sum_reference_input();
    let sum = pairwise_sum_f64(&input);
    hash_bytes(format!("{sum:.17}").as_bytes())
}

fn pairwise_sum_f64(values: &[f64]) -> f64 {
    match values.len() {
        0 => 0.0,
        1 => values[0],
        _ => {
            let mid = values.len() / 2;
            pairwise_sum_f64(&values[..mid]) + pairwise_sum_f64(&values[mid..])
        }
    }
}

/// `Empty` propagates through division — never becomes zero (§2.5).
pub fn div_int_or_empty(
    numerator: AggOutcome<Int>,
    denominator: Int,
) -> Result<AggOutcome<Decimal>, AggError> {
    match numerator {
        AggOutcome::Empty(empty) => Ok(AggOutcome::Empty(empty)),
        AggOutcome::Value(num) => num
            .div(denominator)
            .map(AggOutcome::Value)
            .map_err(AggError::Numeric),
    }
}

fn mean_int_total(total: Int, count: usize) -> AggResult<Decimal> {
    let decimal_total = total.to_decimal(0).map_err(AggError::Numeric)?;
    let decimal_count = Decimal {
        scale: 0,
        mantissa: count as i128,
    };
    decimal_total
        .div(decimal_count, MEAN_INT_SCALE, RoundingMode::HalfEven)
        .map(AggOutcome::Value)
        .map_err(AggError::Numeric)
}

fn mean_decimal_total(total: Decimal, count: usize) -> AggResult<Decimal> {
    total
        .div(
            Decimal {
                scale: 0,
                mantissa: count as i128,
            },
            total
                .scale
                .saturating_add(MEAN_INT_SCALE)
                .min(Decimal::MAX_SCALE),
            RoundingMode::HalfEven,
        )
        .map(AggOutcome::Value)
        .map_err(AggError::Numeric)
}

#[cfg(test)]
mod tests {
    #[test]
    fn float_sum_order_sentinel() {
        // Documents A6: sequential float summation is order-sensitive (§5.2, §6.3).
        let ordered = [1e16, 1.0, -1e16];
        let reordered = [-1e16, 1e16, 1.0];
        let sequential = |values: &[f64]| values.iter().copied().sum::<f64>();
        assert_ne!(sequential(&ordered), sequential(&reordered));
    }
}
