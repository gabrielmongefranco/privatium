// Project:  Privatium™  |  File: crates/privatium-core/src/store/decimal.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  Exact decimal arithmetic (spec/data-dictionary.md §2.1: money is never a float),
//           as a Rust type and as the SQL functions and collation registered on every
//           connection — decimal(), decimal_add/sub/mul/cmp, decimal_sum(), and the
//           `decimal` collating sequence that sorts a DECIMAL column numerically. The same
//           type backs pv.dec (M7).

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Mul, Sub};

use rusqlite::Connection;
use rusqlite::functions::{Aggregate, Context, FunctionFlags};
use rusqlite::types::{ToSqlOutput, Value};

/// The most digits a value may carry, integer and fraction together. Money at four places
/// fits with thirty digits to spare; `i128` holds thirty-eight.
pub const MAX_DIGITS: usize = 36;

/// An exact decimal: an integer mantissa and a scale, `mantissa × 10⁻ˢᶜᵃˡᵉ`.
///
/// No `f64` anywhere: parsing reads digits, arithmetic is integer arithmetic, and
/// rendering writes the digits back with the point where the scale puts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decimal {
    mantissa: i128,
    scale: u8,
}

impl Decimal {
    /// Parse `[-+]digits[.digits]`, with surrounding whitespace allowed and nothing else.
    ///
    /// No exponent form: `1e2` is not money and is refused rather than guessed at. A bare
    /// point on either side is fine (`.5`, `5.`); a sign alone or an empty string is not.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        if !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        if whole.len() + fraction.len() > MAX_DIGITS || fraction.len() > u8::MAX as usize {
            return None;
        }
        let mut mantissa: i128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            mantissa = mantissa * 10 + i128::from(byte - b'0');
        }
        if negative {
            mantissa = -mantissa;
        }
        Some(Self {
            mantissa,
            // `fraction.len() <= u8::MAX` was checked above.
            scale: fraction.len() as u8,
        })
    }

    /// The number of places after the point.
    #[must_use]
    pub fn scale(self) -> u8 {
        self.scale
    }

    /// The same value at `scale` places: padded with zeros, or rounded half away from zero.
    #[must_use]
    pub fn with_scale(self, scale: u8) -> Self {
        if scale == self.scale {
            return self;
        }
        if scale > self.scale {
            let factor = pow10(scale - self.scale);
            return Self {
                mantissa: self.mantissa.saturating_mul(factor),
                scale,
            };
        }
        let factor = pow10(self.scale - scale);
        let quotient = self.mantissa / factor;
        let remainder = (self.mantissa % factor).abs();
        let round_up = remainder * 2 >= factor;
        let mantissa = if round_up {
            quotient + self.mantissa.signum()
        } else {
            quotient
        };
        Self { mantissa, scale }
    }

    /// Both values at the larger of the two scales.
    fn aligned(self, other: Self) -> (i128, i128, u8) {
        let scale = self.scale.max(other.scale);
        (
            self.with_scale(scale).mantissa,
            other.with_scale(scale).mantissa,
            scale,
        )
    }

    /// Numeric comparison, whatever the scales.
    #[must_use]
    pub fn compare(self, other: Self) -> Ordering {
        let (a, b, _) = self.aligned(other);
        a.cmp(&b)
    }

    /// Whether the value is negative.
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.mantissa < 0
    }
}

/// `a + b`, exact, at the larger of the two scales.
impl Add for Decimal {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let (a, b, scale) = self.aligned(other);
        Self {
            mantissa: a.saturating_add(b),
            scale,
        }
    }
}

/// `a - b`, exact, at the larger of the two scales.
impl Sub for Decimal {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        let (a, b, scale) = self.aligned(other);
        Self {
            mantissa: a.saturating_sub(b),
            scale,
        }
    }
}

/// `a × b`, exact: the scales add.
impl Mul for Decimal {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self {
            mantissa: self.mantissa.saturating_mul(other.mantissa),
            scale: self.scale.saturating_add(other.scale),
        }
    }
}

impl fmt::Display for Decimal {
    /// `-12.34`: the digits, a point where the scale puts it, a leading zero before a bare
    /// fraction, and never an exponent.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.mantissa.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        if self.mantissa < 0 {
            f.write_str("-")?;
        }
        if scale == 0 {
            return f.write_str(&digits);
        }
        if digits.len() <= scale {
            f.write_str("0.")?;
            for _ in 0..(scale - digits.len()) {
                f.write_str("0")?;
            }
            return f.write_str(&digits);
        }
        let (whole, fraction) = digits.split_at(digits.len() - scale);
        write!(f, "{whole}.{fraction}")
    }
}

fn pow10(places: u8) -> i128 {
    10_i128.saturating_pow(u32::from(places))
}

/// The declared scale of a `DECIMAL(p,s)` type name, if it has one; `DECIMAL` alone is
/// scale 0 by the SQL convention, and anything else is not a decimal.
#[must_use]
pub fn declared_scale(ty: &str) -> Option<u8> {
    let upper = ty.trim().to_ascii_uppercase();
    let rest = upper
        .strip_prefix("DECIMAL")
        .or_else(|| upper.strip_prefix("NUMERIC"))?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(0);
    }
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    match inner.split_once(',') {
        Some((_, scale)) => scale.trim().parse().ok(),
        None => Some(0),
    }
}

// ---------------------------------------------------------------------------------------
// SQL surface
// ---------------------------------------------------------------------------------------

/// The collating sequence a `DECIMAL` column carries, so `ORDER BY` and `<` on it are
/// numeric rather than lexical (`"9.50"` would otherwise sort after `"10.00"`).
pub const COLLATION: &str = "decimal";

/// Register the decimal functions and collation on a connection.
///
/// Named as SQLite's own `decimal` extension names them, so an author who has read that
/// documentation is not surprised: `decimal(X)` normalizes, `decimal_add`, `decimal_sub`,
/// `decimal_mul` and `decimal_cmp` are exact, `decimal_sum` is the aggregate. There is no
/// division: a quotient is not exact, and a rate is a business rule, not a column.
pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("decimal", 1, flags, |ctx| Ok(text(arg(ctx, 0))))?;
    conn.create_scalar_function("decimal_add", 2, flags, |ctx| {
        Ok(text(binary(ctx, |a, b| a + b)))
    })?;
    conn.create_scalar_function("decimal_sub", 2, flags, |ctx| {
        Ok(text(binary(ctx, |a, b| a - b)))
    })?;
    conn.create_scalar_function("decimal_mul", 2, flags, |ctx| {
        Ok(text(binary(ctx, |a, b| a * b)))
    })?;
    conn.create_scalar_function("decimal_cmp", 2, flags, |ctx| {
        Ok(match (arg(ctx, 0), arg(ctx, 1)) {
            (Some(a), Some(b)) => Value::Integer(match a.compare(b) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            }),
            _ => Value::Null,
        })
    })?;
    conn.create_aggregate_function("decimal_sum", 1, flags, Sum)?;
    conn.create_collation(COLLATION, |a, b| {
        match (Decimal::parse(a), Decimal::parse(b)) {
            (Some(a), Some(b)) => a.compare(b),
            // Something that is not a number sorts as text, after the numbers.
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    })?;
    Ok(())
}

/// Argument `idx` as a decimal: text parses, an integer is exact, a NULL is none, and a
/// float — which should never be in a decimal column — is refused rather than trusted.
fn arg(ctx: &Context<'_>, idx: usize) -> Option<Decimal> {
    match ctx.get_raw(idx) {
        rusqlite::types::ValueRef::Text(bytes) => Decimal::parse(std::str::from_utf8(bytes).ok()?),
        rusqlite::types::ValueRef::Integer(i) => Some(Decimal {
            mantissa: i128::from(i),
            scale: 0,
        }),
        _ => None,
    }
}

fn binary(ctx: &Context<'_>, op: fn(Decimal, Decimal) -> Decimal) -> Option<Decimal> {
    Some(op(arg(ctx, 0)?, arg(ctx, 1)?))
}

fn text(value: Option<Decimal>) -> ToSqlOutput<'static> {
    match value {
        Some(value) => ToSqlOutput::Owned(Value::Text(value.to_string())),
        None => ToSqlOutput::Owned(Value::Null),
    }
}

/// `decimal_sum(X)`: exact over every non-NULL row; NULL over none, like `sum()`.
struct Sum;

impl Aggregate<Option<Decimal>, Option<String>> for Sum {
    fn init(&self, _: &mut Context<'_>) -> rusqlite::Result<Option<Decimal>> {
        Ok(None)
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Option<Decimal>) -> rusqlite::Result<()> {
        if let Some(value) = arg(ctx, 0) {
            *acc = Some(match *acc {
                Some(total) => total + value,
                None => value,
            });
        }
        Ok(())
    }

    fn finalize(
        &self,
        _: &mut Context<'_>,
        acc: Option<Option<Decimal>>,
    ) -> rusqlite::Result<Option<String>> {
        Ok(acc.flatten().map(|d| d.to_string()))
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }

    #[test]
    fn parsing_and_rendering_round_trip() {
        for s in [
            "0",
            "12.34",
            "-12.34",
            "0.05",
            "99999999999.99",
            "100",
            "0.0725",
        ] {
            assert_eq!(d(s).to_string(), s);
        }
        assert_eq!(d(".5").to_string(), "0.5");
        assert_eq!(d("5.").to_string(), "5");
        assert_eq!(d("+7.10").to_string(), "7.10");
        assert_eq!(d(" 3 ").to_string(), "3");
        for bad in ["", "-", ".", "1e2", "12,34", "abc", "1.2.3", "NaN"] {
            assert!(Decimal::parse(bad).is_none(), "{bad}");
        }
    }

    /// The case that decided the engine question: 0.1 + 0.2 is 0.3, not 0.30000000000000004.
    #[test]
    fn arithmetic_is_exact() {
        assert_eq!((d("0.1") + d("0.2")).to_string(), "0.3");
        assert_eq!((d("12.34") + d("12.34")).to_string(), "24.68");
        assert_eq!((d("10.00") - d("9.50")).to_string(), "0.50");
        assert_eq!((d("1.10") * d("3")).to_string(), "3.30");
        assert_eq!((d("0.1") * d("0.1")).to_string(), "0.01");
        assert_eq!((d("-1.5") + d("1.5")).to_string(), "0.0");
        assert_eq!(d("9.50").compare(d("10.00")), Ordering::Less);
        assert_eq!(d("1.0").compare(d("1")), Ordering::Equal);
    }

    #[test]
    fn scaling_pads_and_rounds_half_away_from_zero() {
        assert_eq!(d("12.3").with_scale(2).to_string(), "12.30");
        assert_eq!(d("12.345").with_scale(2).to_string(), "12.35");
        assert_eq!(d("12.344").with_scale(2).to_string(), "12.34");
        assert_eq!(d("-12.345").with_scale(2).to_string(), "-12.35");
        assert_eq!(d("0.005").with_scale(2).to_string(), "0.01");
        assert_eq!(d("7").with_scale(0).to_string(), "7");
    }

    #[test]
    fn declared_scales_are_read_from_the_type_name() {
        assert_eq!(declared_scale("DECIMAL(18,2)"), Some(2));
        assert_eq!(declared_scale("decimal(9, 4)"), Some(4));
        assert_eq!(declared_scale("DECIMAL"), Some(0));
        assert_eq!(declared_scale("NUMERIC(10)"), Some(0));
        assert_eq!(declared_scale("VARCHAR"), None);
        assert_eq!(declared_scale("BIGINT"), None);
    }

    /// The functions and the collation, through SQLite itself.
    #[test]
    fn the_sql_surface_is_exact_and_sorts_numerically() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE t (amount TEXT COLLATE decimal);
             INSERT INTO t VALUES ('0.1'), ('0.2'), ('9.50'), ('10.00'), (NULL);",
        )
        .unwrap();
        let sum: String = conn
            .query_row("SELECT decimal_sum(amount) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sum, "19.80");
        let order: Vec<String> = conn
            .prepare("SELECT amount FROM t WHERE amount IS NOT NULL ORDER BY amount")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(order, ["0.1", "0.2", "9.50", "10.00"]);
        let (add, sub, mul, cmp): (String, String, String, i64) = conn
            .query_row(
                "SELECT decimal_add('0.1', '0.2'), decimal_sub('1', '0.25'), \
                        decimal_mul('1.5', 2), decimal_cmp('9.50', '10')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (add.as_str(), sub.as_str(), mul.as_str(), cmp),
            ("0.3", "0.75", "3.0", -1)
        );
        let none: Option<String> = conn
            .query_row("SELECT decimal_add('abc', '1')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(none, None);
        let empty: Option<String> = conn
            .query_row(
                "SELECT decimal_sum(amount) FROM t WHERE amount IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(empty, None);
    }
}
