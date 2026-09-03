// Project:  Privatium™  |  File: crates/privatium-core/src/lua/dec.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  pv.dec (spec/lua-api.md §3.2): store::Decimal as a Lua userdata. Construction
//           from a string or an integer, never a float; + - * and unary minus exact and
//           erroring on overflow rather than saturating; comparison across scales; `/` at
//           the larger scale of the operands, rounded half away from zero, and `:div` for
//           an author who wants to name the scale.

use std::cmp::Ordering;

use mlua::{MetaMethod, UserData, UserDataMethods, Value};

use crate::store::Decimal;

/// The userdata behind `pv.dec(...)`.
#[derive(Debug, Clone, Copy)]
pub struct Dec(pub Decimal);

impl Dec {
    /// A decimal from a Lua value: another `pv.dec`, a string, or an integer.
    ///
    /// A float is refused: `0.1` is already not `0.1` by the time Lua hands it over, and
    /// silently accepting it would be the exact mistake `pv.dec` exists to prevent.
    pub fn coerce(value: &Value) -> mlua::Result<Decimal> {
        match value {
            Value::UserData(ud) => ud.borrow::<Dec>().map(|dec| dec.0).map_err(|_| {
                mlua::Error::runtime("pv.dec: expected a decimal, got another userdata")
            }),
            Value::String(text) => {
                let text = text.to_str()?;
                Decimal::parse(&text).ok_or_else(|| {
                    mlua::Error::runtime(format!(
                        "pv.dec: {:?} is not a decimal ([-]digits[.digits])",
                        &*text
                    ))
                })
            }
            Value::Integer(n) => Ok(Decimal::from_i64(*n)),
            Value::Number(_) => Err(mlua::Error::runtime(
                "pv.dec: a Lua number is a float and not exact; pass the digits as a string \
                 (spec/lua-api.md §3.2)",
            )),
            other => Err(mlua::Error::runtime(format!(
                "pv.dec: expected a decimal, got {}",
                other.type_name()
            ))),
        }
    }
}

fn overflow() -> mlua::Error {
    mlua::Error::runtime(format!(
        "pv.dec: the result does not fit in {} digits",
        crate::store::decimal::MAX_DIGITS
    ))
}

fn binary(a: &Value, b: &Value, op: fn(Decimal, Decimal) -> Option<Decimal>) -> mlua::Result<Dec> {
    let (a, b) = (Dec::coerce(a)?, Dec::coerce(b)?);
    op(a, b).map(Dec).ok_or_else(overflow)
}

fn compare(a: &Value, b: &Value) -> mlua::Result<Ordering> {
    Ok(Dec::coerce(a)?.compare(Dec::coerce(b)?))
}

/// A scale argument: a non-negative integer that fits the type.
fn scale_arg(value: i64) -> mlua::Result<u8> {
    u8::try_from(value)
        .ok()
        .filter(|scale| usize::from(*scale) <= crate::store::decimal::MAX_DIGITS)
        .ok_or_else(|| {
            mlua::Error::runtime(format!(
                "pv.dec: scale must be between 0 and {}",
                crate::store::decimal::MAX_DIGITS
            ))
        })
}

impl UserData for Dec {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            binary(&a, &b, Decimal::checked_add)
        });
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            binary(&a, &b, Decimal::checked_sub)
        });
        methods.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| {
            binary(&a, &b, Decimal::checked_mul)
        });
        // `a / b`: at the larger scale of the two, rounded half away from zero — the rule
        // `a:div(b, scale)` lets the author override.
        methods.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| {
            let (a, b) = (Dec::coerce(&a)?, Dec::coerce(&b)?);
            if b.is_zero() {
                return Err(mlua::Error::runtime("pv.dec: division by zero"));
            }
            a.div_scaled(b, a.scale().max(b.scale()))
                .map(Dec)
                .ok_or_else(overflow)
        });
        methods.add_meta_method(MetaMethod::Unm, |_, this, ()| {
            this.0.checked_neg().map(Dec).ok_or_else(overflow)
        });
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(compare(&a, &b)? == Ordering::Equal)
        });
        methods.add_meta_function(MetaMethod::Lt, |_, (a, b): (Value, Value)| {
            Ok(compare(&a, &b)? == Ordering::Less)
        });
        methods.add_meta_function(MetaMethod::Le, |_, (a, b): (Value, Value)| {
            Ok(compare(&a, &b)? != Ordering::Greater)
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));

        methods.add_method("tostring", |_, this, ()| Ok(this.0.to_string()));
        methods.add_method("scale", |_, this, ()| Ok(i64::from(this.0.scale())));
        methods.add_method("is_negative", |_, this, ()| Ok(this.0.is_negative()));
        methods.add_method("is_zero", |_, this, ()| Ok(this.0.is_zero()));
        methods.add_method("neg", |_, this, ()| {
            this.0.checked_neg().map(Dec).ok_or_else(overflow)
        });
        // `d:with_scale(2)`: pad, or round half away from zero.
        methods.add_method("with_scale", |_, this, scale: i64| {
            this.0
                .checked_with_scale(scale_arg(scale)?)
                .map(Dec)
                .ok_or_else(overflow)
        });
        // `a:div(b, scale)`: the one division there is.
        methods.add_method("div", |_, this, (other, scale): (Value, i64)| {
            let divisor = Dec::coerce(&other)?;
            if divisor.is_zero() {
                return Err(mlua::Error::runtime("pv.dec: division by zero"));
            }
            this.0
                .div_scaled(divisor, scale_arg(scale)?)
                .map(Dec)
                .ok_or_else(overflow)
        });
        // `d:compare(other)`: -1, 0 or 1, for sorting.
        methods.add_method("compare", |_, this, other: Value| {
            Ok(match this.0.compare(Dec::coerce(&other)?) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        });
    }
}
