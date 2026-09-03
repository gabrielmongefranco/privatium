// Project:  Privatium™  |  File: crates/privatium-core/src/lua/html.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The one value `<?= ?>` emits without escaping (spec/lua-api.md §4): markup the
//           framework itself produced — icon(), csrf(), render(), a layout's content. A
//           string is data and is escaped; an Html is markup and passes. Concatenating one
//           into a string yields a plain string, which is then escaped again: losing the
//           marker is the safe direction, and there is no flag to change any of it.

use mlua::{MetaMethod, UserData, UserDataMethods, Value};

use crate::icons::escape;

/// Markup the framework produced, safe to emit as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Html(pub String);

impl UserData for Html {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.clone()));
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.0.len()));
        methods.add_meta_function(MetaMethod::Concat, |lua, (a, b): (Value, Value)| {
            let mut out = concat_part(lua, &a)?;
            out.push_str(&concat_part(lua, &b)?);
            Ok(out)
        });
    }
}

/// One operand of `..` involving an `Html`: Lua's own rule — strings and numbers
/// concatenate, nothing else does — with the marker's text standing in for itself.
fn concat_part(lua: &mlua::Lua, value: &Value) -> mlua::Result<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(_) => lua_tostring(lua, value),
        Value::UserData(ud) if ud.is::<Html>() => Ok(ud.borrow::<Html>()?.0.clone()),
        other => Err(mlua::Error::runtime(format!(
            "attempt to concatenate a {} value",
            other.type_name()
        ))),
    }
}

/// What `<?= v ?>` emits for `v` (`spec/lua-api.md §4`): nothing for nil, the markup of an
/// `Html`, and the escaped text of anything else that has a text — a string, a number, a
/// boolean, a userdata with `__tostring` such as `pv.dec`. A table or a function has no
/// text and is an error, which names the template line by way of the traceback.
pub(crate) fn emit_escaped(lua: &mlua::Lua, value: &Value) -> mlua::Result<String> {
    match value {
        Value::UserData(ud) if ud.is::<Html>() => Ok(ud.borrow::<Html>()?.0.clone()),
        other => Ok(escape(&text_of(lua, other, "<?= ?>")?)),
    }
}

/// What `<?raw v ?>` emits: the same text, unescaped.
pub(crate) fn emit_raw(lua: &mlua::Lua, value: &Value) -> mlua::Result<String> {
    match value {
        Value::UserData(ud) if ud.is::<Html>() => Ok(ud.borrow::<Html>()?.0.clone()),
        other => text_of(lua, other, "<?raw ?>"),
    }
}

/// The text of a value, by Lua's `tostring` where the type has one.
fn text_of(lua: &mlua::Lua, value: &Value, tag: &str) -> mlua::Result<String> {
    match value {
        Value::Nil => Ok(String::new()),
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        Value::Number(_) | Value::UserData(_) => lua_tostring(lua, value),
        other => Err(mlua::Error::runtime(format!(
            "{tag} cannot emit a {}; give it a string, a number, or a value with a \
             tostring (spec/lua-api.md §4)",
            other.type_name()
        ))),
    }
}

/// Lua's own `tostring` — the one captured at install, before app code could reassign
/// the global — so a float prints as Lua prints it (`1.5`, `2.0`, `1e+20`) and a userdata
/// prints by its `__tostring`.
fn lua_tostring(lua: &mlua::Lua, value: &Value) -> mlua::Result<String> {
    let tostring: mlua::Function = lua.named_registry_value(TOSTRING_KEY)?;
    tostring.call(value.clone())
}

/// Registry key of the `tostring` captured before any app code could reassign it.
pub(crate) const TOSTRING_KEY: &str = "pv.tostring";

/// Capture `tostring` for [`emit_escaped`] on a fresh state.
pub(crate) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    let tostring: mlua::Function = lua.globals().get("tostring")?;
    lua.set_named_registry_value(TOSTRING_KEY, tostring)
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn state() -> Lua {
        let lua = Lua::new();
        install(&lua).unwrap();
        lua
    }

    #[test]
    fn markup_passes_and_data_is_escaped() {
        let lua = state();
        let html = Value::UserData(lua.create_userdata(Html("<b>x</b>".into())).unwrap());
        assert_eq!(emit_escaped(&lua, &html).unwrap(), "<b>x</b>");
        let text = Value::String(lua.create_string("<b>x</b>").unwrap());
        assert_eq!(emit_escaped(&lua, &text).unwrap(), "&lt;b&gt;x&lt;/b&gt;");
        assert_eq!(emit_raw(&lua, &text).unwrap(), "<b>x</b>");
        assert_eq!(emit_escaped(&lua, &Value::Nil).unwrap(), "");
        assert_eq!(emit_escaped(&lua, &Value::Integer(3)).unwrap(), "3");
        assert_eq!(emit_escaped(&lua, &Value::Number(1.5)).unwrap(), "1.5");
        assert_eq!(emit_escaped(&lua, &Value::Number(2.0)).unwrap(), "2.0");
        assert_eq!(emit_escaped(&lua, &Value::Boolean(true)).unwrap(), "true");
        let table = Value::Table(lua.create_table().unwrap());
        let error = emit_escaped(&lua, &table).unwrap_err().to_string();
        assert!(error.contains("cannot emit a table"), "{error}");
    }

    /// `..` with an `Html` yields a plain string — the marker does not survive, so the
    /// result is escaped like any other string.
    #[test]
    fn concatenation_drops_the_marker() {
        let lua = state();
        lua.globals().set("h", Html("<i>".to_owned())).unwrap();
        let joined: Value = lua.load("return 'a' .. h .. 1 .. h").eval().unwrap();
        assert!(matches!(joined, Value::String(_)));
        assert_eq!(emit_escaped(&lua, &joined).unwrap(), "a&lt;i&gt;1&lt;i&gt;");
        let len: usize = lua.load("return #h").eval().unwrap();
        assert_eq!(len, 3);
        let text: String = lua.load("return tostring(h)").eval().unwrap();
        assert_eq!(text, "<i>");
        let bad = lua.load("return h .. {}").eval::<Value>().unwrap_err();
        assert!(bad.to_string().contains("concatenate a table"), "{bad}");
    }
}
