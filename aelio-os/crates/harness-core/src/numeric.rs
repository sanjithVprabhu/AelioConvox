//! HKv4 numeric tower (§2.1–2.2): `Int`, `Float`, `Decimal` with no implicit coercion.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Integer values — counts, IDs, indices (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Int(pub i64);

/// IEEE-754 binary64 measurements (§2.1). Non-finite values are rejected at construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Float(pub f64);

/// Fixed-point decimal: `value = mantissa × 10^(-scale)` (§2.1).
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    pub scale: u32,
    pub mantissa: i128,
}

/// Numeric tower member for polymorphic dispatch at op boundaries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Num {
    Int(Int),
    Float(Float),
    Decimal(Decimal),
}

/// Banker's rounding mode — declared on ops, never inferred (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingMode {
    HalfEven,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumError {
    TypeMismatch {
        op: &'static str,
        left: &'static str,
        right: &'static str,
    },
    NonFinite,
    DivByZero,
    Overflow,
    ScaleTooLarge,
}

pub type NumResult<T> = Result<T, NumError>;

impl Int {
    pub const ZERO: Self = Int(0);
    pub const ONE: Self = Int(1);

    pub fn add(self, other: Int) -> NumResult<Int> {
        self.0
            .checked_add(other.0)
            .map(Int)
            .ok_or(NumError::Overflow)
    }

    pub fn sub(self, other: Int) -> NumResult<Int> {
        self.0
            .checked_sub(other.0)
            .map(Int)
            .ok_or(NumError::Overflow)
    }

    pub fn mul(self, other: Int) -> NumResult<Int> {
        self.0
            .checked_mul(other.0)
            .map(Int)
            .ok_or(NumError::Overflow)
    }

    /// Integer floor division (§2.1 `div_floor`).
    pub fn div_floor(self, other: Int) -> NumResult<Int> {
        if other.0 == 0 {
            return Err(NumError::DivByZero);
        }
        Ok(Int(self.0.div_euclid(other.0)))
    }

    /// Exact rational division — returns `Decimal`, never truncated `Int` (§2.1).
    pub fn div(self, other: Int) -> NumResult<Decimal> {
        if other.0 == 0 {
            return Err(NumError::DivByZero);
        }
        decimal_from_ratio(self.0, other.0, DEFAULT_DIV_SCALE, RoundingMode::HalfEven)
    }

    pub fn to_decimal(self, scale: u32) -> NumResult<Decimal> {
        Decimal::from_int(self, scale)
    }

    pub fn to_float(self) -> NumResult<Float> {
        Float::new(self.0 as f64)
    }
}

impl Float {
    pub fn new(value: f64) -> NumResult<Self> {
        if !value.is_finite() {
            return Err(NumError::NonFinite);
        }
        let normalized = if value == 0.0 { 0.0 } else { value };
        Ok(Float(normalized))
    }

    pub fn add(self, other: Float) -> NumResult<Float> {
        Float::new(self.0 + other.0)
    }

    pub fn sub(self, other: Float) -> NumResult<Float> {
        Float::new(self.0 - other.0)
    }

    pub fn mul(self, other: Float) -> NumResult<Float> {
        Float::new(self.0 * other.0)
    }

    pub fn div(self, other: Float) -> NumResult<Float> {
        if other.0 == 0.0 {
            return Err(NumError::DivByZero);
        }
        Float::new(self.0 / other.0)
    }
}

impl Decimal {
    /// The reconciliation scale ceiling. Division clamps rather than rejects
    /// `input_scale + 6` so a valid decimal column remains aggregatable.
    pub const MAX_SCALE: u32 = 28;

    pub fn from_int(value: Int, scale: u32) -> NumResult<Self> {
        if scale > Self::MAX_SCALE {
            return Err(NumError::ScaleTooLarge);
        }
        let factor = pow10_i128(scale)?;
        Ok(Decimal {
            scale,
            mantissa: i128::from(value.0)
                .checked_mul(factor)
                .ok_or(NumError::Overflow)?,
        })
    }

    pub fn add(self, other: Decimal) -> NumResult<Decimal> {
        let (a, b) = align_decimals(self, other)?;
        a.checked_add(b)
            .map(|mantissa| Decimal {
                scale: a_scale(self, other),
                mantissa,
            })
            .ok_or(NumError::Overflow)
    }

    pub fn sub(self, other: Decimal) -> NumResult<Decimal> {
        let (a, b) = align_decimals(self, other)?;
        a.checked_sub(b)
            .map(|mantissa| Decimal {
                scale: a_scale(self, other),
                mantissa,
            })
            .ok_or(NumError::Overflow)
    }

    pub fn mul(self, other: Decimal) -> NumResult<Decimal> {
        let scale = self
            .scale
            .checked_add(other.scale)
            .ok_or(NumError::ScaleTooLarge)?;
        if scale > Self::MAX_SCALE {
            return Err(NumError::ScaleTooLarge);
        }
        self.mantissa
            .checked_mul(other.mantissa)
            .map(|mantissa| Decimal { scale, mantissa })
            .ok_or(NumError::Overflow)
    }

    /// Division with banker's rounding at `result_scale` (§2.1).
    pub fn div(self, other: Decimal, result_scale: u32, mode: RoundingMode) -> NumResult<Decimal> {
        if other.mantissa == 0 {
            return Err(NumError::DivByZero);
        }
        if result_scale > Self::MAX_SCALE {
            return Err(NumError::ScaleTooLarge);
        }
        match mode {
            RoundingMode::HalfEven => {
                let exp = i64::from(result_scale) + i64::from(other.scale) - i64::from(self.scale);
                let numerator = if exp >= 0 {
                    self.mantissa
                        .checked_mul(pow10_i128(exp as u32)?)
                        .ok_or(NumError::Overflow)?
                } else {
                    let divisor = pow10_i128((-exp) as u32)?;
                    div_round_half_even(self.mantissa, divisor)?
                };
                let mantissa = div_round_half_even(numerator, other.mantissa)?;
                Ok(Decimal {
                    scale: result_scale,
                    mantissa,
                })
            }
        }
    }

    /// Numeric comparison after exact scale alignment.
    pub fn cmp_value(self, other: Decimal) -> NumResult<std::cmp::Ordering> {
        let (left, right) = align_decimals(self, other)?;
        Ok(left.cmp(&right))
    }

    pub fn to_float(self) -> NumResult<Float> {
        let factor = pow10_f64(self.scale)?;
        Float::new(self.mantissa as f64 / factor)
    }
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        normalized_decimal_parts(*self) == normalized_decimal_parts(*other)
    }
}

impl Eq for Decimal {}

impl Hash for Decimal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        normalized_decimal_parts(*self).hash(state);
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.cmp_value(*other).ok()
    }
}

/// Type-strict binary op dispatch — mixed-type inputs are errors (§2.1).
pub fn add(left: Num, right: Num) -> NumResult<Num> {
    match (left, right) {
        (Num::Int(a), Num::Int(b)) => Ok(Num::Int(a.add(b)?)),
        (Num::Float(a), Num::Float(b)) => Ok(Num::Float(a.add(b)?)),
        (Num::Decimal(a), Num::Decimal(b)) => Ok(Num::Decimal(a.add(b)?)),
        (left, right) => Err(type_mismatch("add", left, right)),
    }
}

pub fn sub(left: Num, right: Num) -> NumResult<Num> {
    match (left, right) {
        (Num::Int(a), Num::Int(b)) => Ok(Num::Int(a.sub(b)?)),
        (Num::Float(a), Num::Float(b)) => Ok(Num::Float(a.sub(b)?)),
        (Num::Decimal(a), Num::Decimal(b)) => Ok(Num::Decimal(a.sub(b)?)),
        (left, right) => Err(type_mismatch("sub", left, right)),
    }
}

pub fn mul(left: Num, right: Num) -> NumResult<Num> {
    match (left, right) {
        (Num::Int(a), Num::Int(b)) => Ok(Num::Int(a.mul(b)?)),
        (Num::Float(a), Num::Float(b)) => Ok(Num::Float(a.mul(b)?)),
        (Num::Decimal(a), Num::Decimal(b)) => Ok(Num::Decimal(a.mul(b)?)),
        (left, right) => Err(type_mismatch("mul", left, right)),
    }
}

pub fn div(left: Num, right: Num) -> NumResult<Num> {
    match (left, right) {
        (Num::Int(a), Num::Int(b)) => Ok(Num::Decimal(a.div(b)?)),
        (Num::Float(a), Num::Float(b)) => Ok(Num::Float(a.div(b)?)),
        (Num::Decimal(a), Num::Decimal(b)) => Ok(Num::Decimal(a.div(
            b,
            division_scale(a.scale.max(b.scale)),
            RoundingMode::HalfEven,
        )?)),
        (left, right) => Err(type_mismatch("div", left, right)),
    }
}

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Num::Int(v) => write!(f, "Int({})", v.0),
            Num::Float(v) => write!(f, "Float({})", v.0),
            Num::Decimal(v) => write!(f, "Decimal({}, {})", v.mantissa, v.scale),
        }
    }
}

const DEFAULT_DIV_SCALE: u32 = 6;

/// Every decimal division retains at least six fractional places, bounded by
/// the fixed-point reconciliation limit.
pub const fn division_scale(input_scale: u32) -> u32 {
    let scale = if input_scale > DEFAULT_DIV_SCALE {
        input_scale
    } else {
        DEFAULT_DIV_SCALE
    };
    if scale > Decimal::MAX_SCALE {
        Decimal::MAX_SCALE
    } else {
        scale
    }
}

fn type_mismatch(op: &'static str, left: Num, right: Num) -> NumError {
    NumError::TypeMismatch {
        op,
        left: num_type_name(left),
        right: num_type_name(right),
    }
}

fn num_type_name(value: Num) -> &'static str {
    match value {
        Num::Int(_) => "Int",
        Num::Float(_) => "Float",
        Num::Decimal(_) => "Decimal",
    }
}

fn a_scale(a: Decimal, b: Decimal) -> u32 {
    a.scale.max(b.scale)
}

fn align_decimals(a: Decimal, b: Decimal) -> NumResult<(i128, i128)> {
    if a.scale == b.scale {
        return Ok((a.mantissa, b.mantissa));
    }
    if a.scale > b.scale {
        let factor = pow10_i128(a.scale - b.scale)?;
        let b_m = i128::from(b.mantissa)
            .checked_mul(factor)
            .ok_or(NumError::Overflow)?;
        Ok((a.mantissa, b_m))
    } else {
        let factor = pow10_i128(b.scale - a.scale)?;
        let a_m = i128::from(a.mantissa)
            .checked_mul(factor)
            .ok_or(NumError::Overflow)?;
        Ok((a_m, b.mantissa))
    }
}

fn normalized_decimal_parts(value: Decimal) -> (i128, u32) {
    let mut mantissa = value.mantissa;
    let mut scale = value.scale;
    while scale > 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        scale -= 1;
    }
    (mantissa, scale)
}

fn pow10_i128(exp: u32) -> NumResult<i128> {
    if exp > Decimal::MAX_SCALE {
        return Err(NumError::ScaleTooLarge);
    }
    let mut value = 1_i128;
    for _ in 0..exp {
        value = value.checked_mul(10).ok_or(NumError::Overflow)?;
    }
    Ok(value)
}

fn pow10_f64(exp: u32) -> NumResult<f64> {
    if exp > Decimal::MAX_SCALE {
        return Err(NumError::ScaleTooLarge);
    }
    let mut value = 1.0_f64;
    for _ in 0..exp {
        value *= 10.0;
    }
    Ok(value)
}

fn decimal_from_ratio(
    numerator: i64,
    denominator: i64,
    scale: u32,
    mode: RoundingMode,
) -> NumResult<Decimal> {
    match mode {
        RoundingMode::HalfEven => {
            let num = i128::from(numerator)
                .checked_mul(pow10_i128(scale)?)
                .ok_or(NumError::Overflow)?;
            let mantissa = div_round_half_even(num, i128::from(denominator))?;
            Ok(Decimal { scale, mantissa })
        }
    }
}

/// Banker's rounding (round half to even).
fn div_round_half_even(numerator: i128, denominator: i128) -> NumResult<i128> {
    if denominator == 0 {
        return Err(NumError::DivByZero);
    }
    let negative = (numerator < 0) ^ (denominator < 0);
    let num = numerator.checked_abs().ok_or(NumError::Overflow)?;
    let den = denominator.checked_abs().ok_or(NumError::Overflow)?;
    let mut q = num / den;
    let r = num % den;
    let twice_r = r.checked_mul(2).ok_or(NumError::Overflow)?;
    if twice_r > den {
        q = q.checked_add(1).ok_or(NumError::Overflow)?;
    } else if twice_r == den {
        if q % 2 == 1 {
            q = q.checked_add(1).ok_or(NumError::Overflow)?;
        }
    }
    let signed = if negative { -q } else { q };
    Ok(signed)
}
