// Project:  Privatium™  |  File: crates/privatium-core/src/lint/lua.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The Lua rules over a full_moon AST, never a regex (docs/plans/phase-1.md M12):
//           PV201 concatenated SQL, PV203 the sandbox's removed names, PV301 a literal
//           mount path, PV302 a DECIMAL or BIGINT column treated as a number, PV303 and
//           PV308 over the SQL literals handed to pv.query, PV305 outbox bookkeeping by
//           name, PV306 appends that should be one batch, PV307 a handler's global or a
//           mutated load-time table, PV503 icon names, PV505 filesystem paths, PV506 routes
//           a framework prefix shadows. The same walk serves a template's compiled chunk,
//           which is why what it learns — the views pv.render names — comes back as facts.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use full_moon::ast::{
    self, Ast, BinOp, Block, Call, Expression, Field, FunctionArgs, FunctionBody, Index, LastStmt,
    Parameter, Prefix, Stmt, Suffix, Var,
};
use full_moon::tokenizer::{TokenReference, TokenType};

use crate::lint::{Columns, Ctx, Edit, RuleId, is_absolute_fs_path, mount_path};

/// What a walk learns that other rules need.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    /// Views named by `pv.render('<name>', …)`.
    pub rendered: BTreeSet<String>,
    /// Route patterns registered by `pv.get`, `pv.post`, `pv.route`.
    pub routes: Vec<String>,
}

/// A finding before it is attached to a file: the rule, the line in the parsed source,
/// the message, the fix, and an edit as byte offsets into that source.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub rule: RuleId,
    pub line: u32,
    pub message: String,
    pub fix: Option<String>,
    pub edit: Option<(usize, usize, String)>,
}

/// What the walker needs to know about the app.
#[derive(Debug, Clone)]
pub(crate) struct Env {
    pub slug: String,
    pub columns: Columns,
    /// True for a template's compiled chunk: the rules about a handler's state do not
    /// apply, and edits cannot be made in generated text.
    pub template: bool,
}

/// `app.lua` and every `lib/**/*.lua`: parse, walk, and record what was learned.
pub(crate) fn check_app(ctx: &mut Ctx<'_>) -> Facts {
    let mut facts = Facts::default();
    let tier_lua = ctx
        .manifest
        .as_ref()
        .is_none_or(|m| m.app.tier == crate::app::manifest::Tier::Lua);
    if !tier_lua {
        return facts;
    }
    let mut files = Vec::new();
    if ctx.dir.join("app.lua").is_file() {
        files.push("app.lua".to_owned());
    }
    collect_lua(&ctx.dir.join("lib"), "lib", &mut files);
    files.sort();
    let env = Env {
        slug: ctx.slug(),
        columns: Columns::of(ctx.schema.as_ref()),
        template: false,
    };
    for rel in files {
        let Some(text) = ctx.read(&rel) else {
            continue;
        };
        let ast = match full_moon::parse(&text) {
            Ok(ast) => ast,
            Err(errors) => {
                for error in errors {
                    let line = error.range().0.line() as u32;
                    ctx.push(
                        RuleId::PV105,
                        &rel,
                        line,
                        format!("does not parse as Lua 5.4: {}", error.error_message()),
                    );
                }
                continue;
            }
        };
        let (pending, learned) = walk(&ast, &env);
        facts.rendered.extend(learned.rendered);
        facts.routes.extend(learned.routes);
        let path = ctx.path(&rel);
        for p in pending {
            let finding = ctx.push(p.rule, &rel, p.line, p.message);
            finding.fix = p.fix;
            finding.edit = p.edit.map(|(start, end, replacement)| Edit {
                file: path.clone(),
                start,
                end,
                replacement,
            });
        }
    }
    facts
}

fn collect_lua(dir: &Path, rel: &str, into: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_lua(&path, &format!("{rel}/{name}"), into);
        } else if name.ends_with(".lua") {
            into.push(format!("{rel}/{name}"));
        }
    }
}

/// Walk one chunk.
pub(crate) fn walk(ast: &Ast, env: &Env) -> (Vec<Pending>, Facts) {
    let mut walker = Walker {
        env,
        locals: BTreeSet::new(),
        module_locals: BTreeSet::new(),
        inner_locals: BTreeSet::new(),
        tainted: BTreeSet::new(),
        depth: 0,
        in_batch: 0,
        noted: BTreeSet::new(),
        findings: Vec::new(),
        facts: Facts::default(),
    };
    walker.declare_block(ast.nodes(), 0);
    walker.block(ast.nodes());
    (walker.findings, walker.facts)
}

/// The names `spec/lua-api.md §5` removes, as `(base, member)`; a `None` member is the
/// whole global.
const REMOVED: &[(&str, Option<&str>)] = &[
    ("io", None),
    ("debug", None),
    ("load", None),
    ("loadstring", None),
    ("dofile", None),
    ("loadfile", None),
    ("os", Some("execute")),
    ("os", Some("exit")),
    ("os", Some("getenv")),
    ("os", Some("remove")),
    ("os", Some("rename")),
    ("os", Some("tmpname")),
    ("os", Some("setlocale")),
    ("package", Some("loadlib")),
    ("package", Some("cpath")),
];

/// Name fragments that mean an outbox is being deduplicated by hand (`PV305`).
const DEDUPE_NAMES: &[&str] = &[
    "dedupe",
    "dedup",
    "txid",
    "tx_id",
    "transaction_id",
    "transactionid",
    "acked",
    "ack_at",
    "acknowledg",
    "outbox",
];

struct Walker<'a> {
    env: &'a Env,
    /// Every name declared `local` or as a parameter anywhere in the chunk.
    locals: BTreeSet<String>,
    /// Names declared `local` at module scope — the VM's baseline.
    module_locals: BTreeSet<String>,
    /// Names declared inside a function, which may shadow a module local.
    inner_locals: BTreeSet<String>,
    /// Locals assigned from a concatenation (`PV201` through a variable).
    tainted: BTreeSet<String>,
    depth: u32,
    in_batch: u32,
    /// `PV305` names already reported.
    noted: BTreeSet<String>,
    findings: Vec<Pending>,
    facts: Facts,
}

fn ident(token: &TokenReference) -> Option<String> {
    match token.token().token_type() {
        TokenType::Identifier { identifier } => Some(identifier.to_string()),
        _ => None,
    }
}

fn string_literal(token: &TokenReference) -> Option<String> {
    match token.token().token_type() {
        TokenType::StringLiteral { literal, .. } => Some(literal.to_string()),
        _ => None,
    }
}

fn line(node: &impl full_moon::node::Node) -> u32 {
    node.start_position().map_or(0, |p| p.line() as u32)
}

/// The callee of a call as a dotted path — `pv.query`, `tonumber`, `string.format` —
/// and the arguments of its first call suffix; `None` when the callee is not a plain
/// name chain.
fn callee<'b>(prefix: &Prefix, suffixes: &[&'b Suffix]) -> Option<(Vec<String>, &'b FunctionArgs)> {
    let mut path = match prefix {
        Prefix::Name(token) => vec![ident(token)?],
        _ => return None,
    };
    for suffix in suffixes {
        match suffix {
            Suffix::Index(Index::Dot { name, .. }) => path.push(ident(name)?),
            Suffix::Index(Index::Brackets {
                expression: Expression::String(token),
                ..
            }) => path.push(string_literal(token)?),
            Suffix::Index(Index::Brackets { .. }) => return None,
            Suffix::Call(Call::AnonymousCall(args)) => return Some((path, args)),
            Suffix::Call(Call::MethodCall(method)) => {
                path.push(ident(method.name())?);
                return Some((path, method.args()));
            }
            _ => return None,
        }
    }
    None
}

/// The positional arguments of a call.
fn args_of(args: &FunctionArgs) -> Vec<&Expression> {
    match args {
        FunctionArgs::Parentheses { arguments, .. } => arguments.iter().collect(),
        _ => Vec::new(),
    }
}

/// A string argument: `call('x')` or `call 'x'`.
fn string_arg(args: &FunctionArgs, index: usize) -> Option<(String, &TokenReference)> {
    match args {
        FunctionArgs::String(token) if index == 0 => string_literal(token).map(|s| (s, token)),
        FunctionArgs::Parentheses { arguments, .. } => match arguments.iter().nth(index)? {
            Expression::String(token) => string_literal(token).map(|s| (s, token)),
            _ => None,
        },
        _ => None,
    }
}

/// The column a `row.col` / `row['col']` expression ends in.
fn column_of(expression: &Expression) -> Option<String> {
    let Expression::Var(Var::Expression(var)) = expression else {
        return None;
    };
    let last = var.suffixes().last()?;
    match last {
        Suffix::Index(Index::Dot { name, .. }) => ident(name),
        Suffix::Index(Index::Brackets {
            expression: Expression::String(token),
            ..
        }) => string_literal(token),
        _ => None,
    }
}

/// Whether an expression concatenates anything that is not a literal, or formats.
fn concatenates(expression: &Expression, tainted: &BTreeSet<String>) -> bool {
    match expression {
        Expression::BinaryOperator { lhs, binop, rhs } => {
            if matches!(binop, BinOp::TwoDots(_)) {
                let literal =
                    |e: &Expression| matches!(e, Expression::String(_) | Expression::Number(_));
                if !(literal(lhs) && literal(rhs)) {
                    return true;
                }
            }
            concatenates(lhs, tainted) || concatenates(rhs, tainted)
        }
        Expression::Parentheses { expression, .. } => concatenates(expression, tainted),
        Expression::Var(Var::Name(token)) => ident(token).is_some_and(|n| tainted.contains(&n)),
        Expression::FunctionCall(call) => {
            let suffixes: Vec<&Suffix> = call.suffixes().collect();
            match callee(call.prefix(), &suffixes) {
                Some((path, _)) => {
                    let last = path.last().map(String::as_str).unwrap_or_default();
                    matches!(last, "format" | "rep" | "gsub")
                }
                None => false,
            }
        }
        _ => false,
    }
}

impl Walker<'_> {
    fn push(&mut self, rule: RuleId, line: u32, message: impl Into<String>) -> &mut Pending {
        self.findings.push(Pending {
            rule,
            line,
            message: message.into(),
            fix: None,
            edit: None,
        });
        self.findings
            .last_mut()
            .unwrap_or_else(|| unreachable!("a finding was just pushed"))
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }

    fn declare(&mut self, name: String, depth: u32) {
        if depth == 0 {
            self.module_locals.insert(name.clone());
        } else {
            self.inner_locals.insert(name.clone());
        }
        self.locals.insert(name);
    }

    /// Pass one: every declaration, so the checks know what is local.
    fn declare_block(&mut self, block: &Block, depth: u32) {
        for stmt in block.stmts() {
            match stmt {
                Stmt::LocalAssignment(local) => {
                    for name in local.names().iter() {
                        if let Some(name) = ident(name) {
                            self.declare(name, depth);
                        }
                    }
                    for expression in local.expressions().iter() {
                        self.declare_expression(expression, depth);
                    }
                }
                Stmt::LocalFunction(function) => {
                    if let Some(name) = ident(function.name()) {
                        self.declare(name, depth);
                    }
                    self.declare_body(function.body(), depth + 1);
                }
                Stmt::FunctionDeclaration(function) => {
                    self.declare_body(function.body(), depth + 1)
                }
                Stmt::Assignment(assignment) => {
                    for expression in assignment.expressions().iter() {
                        self.declare_expression(expression, depth);
                    }
                }
                Stmt::FunctionCall(call) => {
                    self.declare_call_args(call.suffixes().collect::<Vec<_>>().as_slice(), depth)
                }
                Stmt::Do(block) => self.declare_block(block.block(), depth),
                Stmt::While(w) => self.declare_block(w.block(), depth),
                Stmt::Repeat(r) => self.declare_block(r.block(), depth),
                Stmt::If(i) => {
                    self.declare_block(i.block(), depth);
                    if let Some(else_ifs) = i.else_if() {
                        for e in else_ifs {
                            self.declare_block(e.block(), depth);
                        }
                    }
                    if let Some(b) = i.else_block() {
                        self.declare_block(b, depth);
                    }
                }
                Stmt::NumericFor(f) => {
                    if let Some(name) = ident(f.index_variable()) {
                        self.declare(name, depth.max(1));
                    }
                    self.declare_block(f.block(), depth);
                }
                Stmt::GenericFor(f) => {
                    for name in f.names().iter() {
                        if let Some(name) = ident(name) {
                            self.declare(name, depth.max(1));
                        }
                    }
                    for expression in f.expressions().iter() {
                        self.declare_expression(expression, depth);
                    }
                    self.declare_block(f.block(), depth);
                }
                _ => {}
            }
        }
        if let Some(LastStmt::Return(r)) = block.last_stmt() {
            for expression in r.returns().iter() {
                self.declare_expression(expression, depth);
            }
        }
    }

    fn declare_call_args(&mut self, suffixes: &[&Suffix], depth: u32) {
        for suffix in suffixes {
            if let Suffix::Call(call) = suffix {
                let args = match call {
                    Call::AnonymousCall(args) => args,
                    Call::MethodCall(method) => method.args(),
                    _ => continue,
                };
                for expression in args_of(args) {
                    self.declare_expression(expression, depth);
                }
            }
        }
    }

    fn declare_expression(&mut self, expression: &Expression, depth: u32) {
        match expression {
            Expression::Function(function) => self.declare_body(function.body(), depth + 1),
            Expression::FunctionCall(call) => {
                self.declare_call_args(call.suffixes().collect::<Vec<_>>().as_slice(), depth);
            }
            Expression::TableConstructor(table) => {
                for field in table.fields().iter() {
                    match field {
                        Field::NameKey { value, .. } | Field::NoKey(value) => {
                            self.declare_expression(value, depth)
                        }
                        Field::ExpressionKey { key, value, .. } => {
                            self.declare_expression(key, depth);
                            self.declare_expression(value, depth);
                        }
                        _ => {}
                    }
                }
            }
            Expression::BinaryOperator { lhs, rhs, .. } => {
                self.declare_expression(lhs, depth);
                self.declare_expression(rhs, depth);
            }
            Expression::Parentheses { expression, .. }
            | Expression::UnaryOperator { expression, .. } => {
                self.declare_expression(expression, depth);
            }
            _ => {}
        }
    }

    fn declare_body(&mut self, body: &FunctionBody, depth: u32) {
        for parameter in body.parameters().iter() {
            if let Parameter::Name(token) = parameter
                && let Some(name) = ident(token)
            {
                self.declare(name, depth);
            }
        }
        self.declare_block(body.block(), depth);
    }

    /// Pass two: the checks.
    fn block(&mut self, block: &Block) {
        let mut writes_here: u32 = 0;
        for stmt in block.stmts() {
            let write = match stmt {
                Stmt::FunctionCall(call) => self.is_write_call(call),
                Stmt::LocalAssignment(local) => local.expressions().iter().any(
                    |e| matches!(e, Expression::FunctionCall(call) if self.is_write_call(call)),
                ),
                _ => false,
            };
            if write && self.in_batch == 0 && !self.env.template {
                writes_here += 1;
                if writes_here == 2 {
                    self.push(
                        RuleId::PV306,
                        line(stmt),
                        "a second pv.append/pv.delete in the same block lands as its own event; if the two must land together, write them in one pv.batch",
                    )
                    .fix = Some("pv.batch(function(tx) tx.append(...); tx.append(...) end)".into());
                }
            }
            self.stmt(stmt);
        }
        if let Some(LastStmt::Return(r)) = block.last_stmt() {
            for expression in r.returns().iter() {
                self.expression(expression);
            }
        }
    }

    fn is_write_call(&self, call: &ast::FunctionCall) -> bool {
        let suffixes: Vec<&Suffix> = call.suffixes().collect();
        matches!(
            callee(call.prefix(), &suffixes)
                .map(|(path, _)| path.join("."))
                .as_deref(),
            Some("pv.append" | "pv.delete")
        )
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assignment(assignment) => {
                for var in assignment.variables().iter() {
                    self.assigned(var, line(stmt));
                }
                for expression in assignment.expressions().iter() {
                    self.expression(expression);
                }
            }
            Stmt::LocalAssignment(local) => {
                let names: Vec<String> = local.names().iter().filter_map(ident).collect();
                for (name, token) in names.iter().zip(local.names().iter()) {
                    self.dedupe_name(name, line(token));
                }
                for (i, expression) in local.expressions().iter().enumerate() {
                    if concatenates(expression, &self.tainted)
                        && let Some(name) = names.get(i)
                    {
                        self.tainted.insert(name.clone());
                    }
                    self.expression(expression);
                }
            }
            Stmt::FunctionCall(call) => self.call(call),
            Stmt::FunctionDeclaration(function) => {
                for token in function.name().names().iter() {
                    if let Some(name) = ident(token) {
                        self.dedupe_name(&name, line(token));
                    }
                }
                self.body(function.body());
            }
            Stmt::LocalFunction(function) => {
                if let Some(name) = ident(function.name()) {
                    self.dedupe_name(&name, line(function.name()));
                }
                self.body(function.body());
            }
            Stmt::Do(d) => self.block(d.block()),
            Stmt::While(w) => {
                self.expression(w.condition());
                self.block(w.block());
            }
            Stmt::Repeat(r) => {
                self.block(r.block());
                self.expression(r.until());
            }
            Stmt::If(i) => {
                self.expression(i.condition());
                self.block(i.block());
                if let Some(else_ifs) = i.else_if() {
                    for e in else_ifs {
                        self.expression(e.condition());
                        self.block(e.block());
                    }
                }
                if let Some(b) = i.else_block() {
                    self.block(b);
                }
            }
            Stmt::NumericFor(f) => {
                self.expression(f.start());
                self.expression(f.end());
                if let Some(step) = f.step() {
                    self.expression(step);
                }
                self.block(f.block());
            }
            Stmt::GenericFor(f) => {
                for expression in f.expressions().iter() {
                    self.expression(expression);
                }
                self.block(f.block());
            }
            _ => {}
        }
    }

    fn body(&mut self, body: &FunctionBody) {
        self.depth += 1;
        self.block(body.block());
        self.depth -= 1;
    }

    /// `PV307`: the two footguns of `spec/lua-api.md §5`.
    fn assigned(&mut self, var: &Var, at: u32) {
        if self.env.template {
            return;
        }
        match var {
            Var::Name(token) => {
                let Some(name) = ident(token) else {
                    return;
                };
                if !self.is_local(&name) && self.depth > 0 {
                    self.push(
                        RuleId::PV307,
                        at,
                        format!("global `{name}` assigned in a handler lasts one request and is never seen by another; a value that must persist goes through the event log"),
                    )
                    .fix = Some("make it `local`, or pv.append it".into());
                }
            }
            Var::Expression(var) => {
                let Prefix::Name(token) = var.prefix() else {
                    return;
                };
                let Some(name) = ident(token) else {
                    return;
                };
                let baseline = self.module_locals.contains(&name)
                    && !self.inner_locals.contains(&name)
                    || (!self.is_local(&name) && name != "_G");
                if self.depth > 0
                    && baseline
                    && var.suffixes().any(|s| matches!(s, Suffix::Index(_)))
                {
                    self.push(
                        RuleId::PV307,
                        at,
                        format!("`{name}` is a load-time table mutated from a handler: the change persists on this VM only and is never shared — the per-VM footgun"),
                    )
                    .fix = Some("keep per-request state local; shared state is an event".into());
                }
                for suffix in var.suffixes() {
                    self.suffix(suffix);
                }
            }
            _ => {}
        }
    }

    fn suffix(&mut self, suffix: &Suffix) {
        match suffix {
            Suffix::Index(Index::Brackets { expression, .. }) => self.expression(expression),
            Suffix::Index(Index::Dot { name, .. }) => {
                if let Some(name) = ident(name) {
                    self.dedupe_name(&name, line(suffix));
                }
            }
            Suffix::Call(Call::AnonymousCall(args)) => {
                for expression in args_of(args) {
                    self.expression(expression);
                }
            }
            Suffix::Call(Call::MethodCall(method)) => {
                for expression in args_of(method.args()) {
                    self.expression(expression);
                }
            }
            _ => {}
        }
    }

    fn dedupe_name(&mut self, name: &str, at: u32) {
        let lower = name.to_ascii_lowercase();
        if DEDUPE_NAMES.iter().any(|n| lower.contains(n)) && self.noted.insert(lower) {
            self.push(
                RuleId::PV305,
                at,
                format!("`{name}` looks like outbox bookkeeping — a dedupe key, a transaction id, an acknowledgement; ULIDs already make replay idempotent"),
            )
            .fix = Some("drop it: retry with the same id and the merge rule converges".into());
        }
    }

    fn expression(&mut self, expression: &Expression) {
        match expression {
            Expression::BinaryOperator { lhs, binop, rhs } => {
                let arithmetic = matches!(
                    binop,
                    BinOp::Plus(_)
                        | BinOp::Minus(_)
                        | BinOp::Star(_)
                        | BinOp::Slash(_)
                        | BinOp::Percent(_)
                        | BinOp::Caret(_)
                        | BinOp::DoubleSlash(_)
                );
                if arithmetic {
                    for side in [lhs.as_ref(), rhs.as_ref()] {
                        if let Some(column) = column_of(side)
                            && self.env.columns.is_decimal(&column)
                        {
                            self.push(
                                RuleId::PV302,
                                line(side),
                                format!("arithmetic on `{column}`, a DECIMAL column that arrives as a string — Lua coerces it to a float"),
                            )
                            .fix = Some(format!("pv.dec(row.{column}) for exact arithmetic, or decimal_sum() in SQL"));
                        }
                    }
                }
                self.expression(lhs);
                self.expression(rhs);
            }
            Expression::Parentheses { expression, .. }
            | Expression::UnaryOperator { expression, .. } => {
                self.expression(expression);
            }
            Expression::Function(function) => self.body(function.body()),
            Expression::FunctionCall(call) => self.call(call),
            Expression::TableConstructor(table) => {
                for field in table.fields().iter() {
                    match field {
                        Field::NameKey { key, value, .. } => {
                            if let Some(name) = ident(key) {
                                self.dedupe_name(&name, line(key));
                            }
                            self.expression(value);
                        }
                        Field::ExpressionKey { key, value, .. } => {
                            self.expression(key);
                            self.expression(value);
                        }
                        Field::NoKey(value) => self.expression(value),
                        _ => {}
                    }
                }
            }
            Expression::String(token) => self.string(token),
            Expression::Var(var) => self.var(var),
            _ => {}
        }
    }

    /// `PV203`, and the dedupe names of `PV305`.
    fn var(&mut self, var: &Var) {
        match var {
            Var::Name(token) => self.global_use(token, None),
            Var::Expression(var) => {
                if let Prefix::Name(token) = var.prefix() {
                    let member = var.suffixes().next().and_then(|s| match s {
                        Suffix::Index(Index::Dot { name, .. }) => ident(name),
                        Suffix::Index(Index::Brackets {
                            expression: Expression::String(t),
                            ..
                        }) => string_literal(t),
                        _ => None,
                    });
                    self.global_use(token, member.as_deref());
                } else if let Prefix::Expression(expression) = var.prefix() {
                    self.expression(expression);
                }
                for suffix in var.suffixes() {
                    self.suffix(suffix);
                }
            }
            _ => {}
        }
    }

    fn global_use(&mut self, token: &TokenReference, member: Option<&str>) {
        let Some(name) = ident(token) else {
            return;
        };
        if self.is_local(&name) {
            return;
        }
        let banned = REMOVED
            .iter()
            .any(|(base, m)| *base == name && (m.is_none() || *m == member));
        if banned {
            let spelled = match member {
                Some(m) if REMOVED.iter().any(|(b, mm)| *b == name && *mm == Some(m)) => {
                    format!("{name}.{m}")
                }
                _ => name.clone(),
            };
            self.push(
                RuleId::PV203,
                line(token),
                format!("`{spelled}` is removed from the sandbox; there is no way to reach it and no workaround to suggest"),
            )
            .fix = Some("read and write through pv.*; os.date, os.time and os.clock stay".into());
        }
        self.dedupe_name(&name, line(token));
    }

    /// `PV301`, `PV505`.
    fn string(&mut self, token: &TokenReference) {
        let Some(text) = string_literal(token) else {
            return;
        };
        let at = line(token);
        if let Some((slug, path)) = mount_path(&text) {
            let own = slug == self.env.slug;
            let template = self.env.template;
            let finding = self.push(
                RuleId::PV301,
                at,
                format!(
                    "literal mount path '/a/{slug}/' breaks solo mode{}",
                    if own {
                        ""
                    } else {
                        " — and names another app, which has no mount at all there"
                    }
                ),
            );
            if own {
                finding.fix = Some(format!("url('{path}')"));
                if !template {
                    let start = token.token().start_position().bytes();
                    let end = token.token().end_position().bytes();
                    finding.edit = Some((start, end, format!("url('{path}')")));
                }
            } else {
                finding.fix = Some(
                    "link between apps through their titles in the launcher, or pass the path in"
                        .into(),
                );
            }
        }
        if is_absolute_fs_path(&text) {
            self.push(
                RuleId::PV505,
                at,
                format!("absolute filesystem path {text:?} — the node owns its data root and nothing else is writable"),
            )
            .fix = Some("keep everything under the app folder or the event log; XDG paths are the node's".into());
        }
    }

    /// Every call: the pv surface, `tonumber`, `icon`, and the arguments.
    fn call(&mut self, call: &ast::FunctionCall) {
        let suffixes: Vec<&Suffix> = call.suffixes().collect();
        if let Prefix::Expression(expression) = call.prefix() {
            self.expression(expression);
        }
        let at = line(call);
        if let Some((path, args)) = callee(call.prefix(), &suffixes) {
            let name = path.join(".");
            let positional = args_of(args);
            match name.as_str() {
                "pv.query" | "pv.query1" | "pv.sql" => {
                    if let Some(first) = positional.first() {
                        if concatenates(first, &self.tainted) {
                            self.push(
                                RuleId::PV201,
                                at,
                                format!("{name} is given SQL built by concatenation; a value reaches the engine as SQL"),
                            )
                            .fix = Some("write `?` in the SQL and pass the values as the second argument: pv.query('… WHERE x = ?', {value})".into());
                        }
                        if let Expression::String(token) = first
                            && let Some(sql) = string_literal(token)
                        {
                            self.sql_literal(&sql, line(token));
                        }
                    }
                }
                "tonumber" | "math.tointeger" | "math.floor" | "math.ceil" => {
                    if let Some(first) = positional.first()
                        && let Some(column) = column_of(first)
                        && (self.env.columns.is_decimal(&column)
                            || self.env.columns.is_integer(&column))
                    {
                        let kind = if self.env.columns.is_decimal(&column) {
                            "DECIMAL"
                        } else {
                            "BIGINT"
                        };
                        self.push(
                            RuleId::PV302,
                            at,
                            format!("{name}() on `{column}`, a {kind} column — a double loses what the text keeps"),
                        )
                        .fix = Some(if kind == "DECIMAL" {
                            format!("pv.dec(row.{column}) — exact arithmetic; fmt.money to display")
                        } else {
                            format!("row.{column} is already a Lua integer; use it as it is")
                        });
                    }
                }
                "pv.get" | "pv.post" | "pv.put" | "pv.delete_route" | "pv.route" => {
                    let index = usize::from(name == "pv.route");
                    if let Some((pattern, _)) = string_arg(args, index) {
                        if let Some(prefix) = crate::wire::router::shadowing_prefix(&pattern) {
                            self.push(
                                RuleId::PV506,
                                at,
                                format!("route {pattern} is shadowed by the framework prefix {prefix} in solo mode, where the app owns /"),
                            )
                            .fix = Some("name the route beneath another path".into());
                        }
                        self.facts.routes.push(pattern);
                    }
                }
                "pv.render" => {
                    if let Some((view, _)) = string_arg(args, 0) {
                        self.facts.rendered.insert(view);
                    }
                }
                "icon" => {
                    if let Some((icon, token)) = string_arg(args, 0)
                        && !crate::icons::exists(&icon)
                    {
                        self.push(
                            RuleId::PV503,
                            line(token),
                            format!("icon {icon:?} is not in the vendored Bootstrap Icons set; question-circle would render in its place"),
                        )
                        .fix = Some("name a file of assets/icons/ without .svg (docs/icons.md)".into());
                    }
                }
                "pv.batch" => {
                    self.in_batch += 1;
                    for expression in &positional {
                        self.expression(expression);
                    }
                    self.in_batch -= 1;
                    return;
                }
                _ => {}
            }
            if (path.len() == 1 || path[0] != "pv")
                && let Prefix::Name(token) = call.prefix()
            {
                let member = path.get(1).map(String::as_str);
                self.global_use(token, member);
            }
        } else if let Prefix::Name(token) = call.prefix() {
            self.global_use(token, None);
        }
        for suffix in &suffixes {
            self.suffix(suffix);
        }
    }

    /// `PV303` and `PV308` over one SQL literal.
    fn sql_literal(&mut self, sql: &str, literal_line: u32) {
        let line_at = |offset: usize| {
            literal_line
                + sql[..offset.min(sql.len())]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count() as u32
        };
        for problem in super::sql::write_problems(sql) {
            let at = line_at(problem.offset);
            self.push(RuleId::PV303, at, problem.message.clone()).fix = Some(problem.fix.clone());
        }
        for problem in super::sql::arithmetic_problems(sql, &self.env.columns) {
            let at = line_at(problem.offset);
            self.push(RuleId::PV308, at, problem.message.clone()).fix = Some(problem.fix.clone());
        }
    }
}

/// The dotted callee and its first-call arguments of `expression`, for the template
/// layer's reading of an emit.
pub(crate) fn callee_of(expression: &Expression) -> Option<(Vec<String>, Vec<Option<String>>)> {
    let Expression::FunctionCall(call) = expression else {
        return None;
    };
    let suffixes: Vec<&Suffix> = call.suffixes().collect();
    let (path, args) = callee(call.prefix(), &suffixes)?;
    let strings = match args {
        FunctionArgs::String(token) => vec![string_literal(token)],
        FunctionArgs::Parentheses { arguments, .. } => arguments
            .iter()
            .map(|e| match e {
                Expression::String(token) => string_literal(token),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Some((path, strings))
}

/// A bare name — `content` in a layout.
pub(crate) fn name_of(expression: &Expression) -> Option<String> {
    match expression {
        Expression::Var(Var::Name(token)) => ident(token),
        _ => None,
    }
}

/// The text of a double-quoted Lua literal as the compiler writes it.
pub(crate) fn decode_literal(token: &TokenReference) -> Option<String> {
    let raw = string_literal(token)?;
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(d) if d.is_ascii_digit() => {
                let mut code = d.to_digit(10).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek() {
                        Some(n) if n.is_ascii_digit() => {
                            code = code * 10 + n.to_digit(10).unwrap_or(0);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    Some(out)
}

/// A map from a rule's line in a template's compiled chunk back to the `.lsp` line.
pub(crate) type LineMapper<'a> = &'a dyn Fn(u32) -> u32;

/// Attach pending findings from a chunk to a file, mapping lines and dropping edits
/// the caller cannot make.
pub(crate) fn attach(ctx: &mut Ctx<'_>, rel: &str, pending: Vec<Pending>, map: LineMapper<'_>) {
    for p in pending {
        let finding = ctx.push(p.rule, rel, map(p.line), p.message);
        finding.fix = p.fix;
    }
}

/// Column names for the table-constructor keys of `PV304`'s Lua sibling are not a rule:
/// a Lua table becomes `d`, never the envelope. Kept as a note for the reader of
/// `spec/data-api.md §2`.
pub(crate) fn _envelope_note() -> &'static [&'static str] {
    &["seq", "lam", "ts", "dev", "app"]
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Schema;

    fn env() -> Env {
        let schema = Schema::parse("CREATE TABLE fill (id VARCHAR PRIMARY KEY, copay DECIMAL(18,2), n BIGINT, due_on DATE);").unwrap();
        Env {
            slug: "meds".into(),
            columns: Columns::of(Some(&schema)),
            template: false,
        }
    }

    fn rules_in(code: &str) -> Vec<(RuleId, u32)> {
        let ast = full_moon::parse(code).unwrap();
        let (pending, _) = walk(&ast, &env());
        pending.into_iter().map(|p| (p.rule, p.line)).collect()
    }

    #[test]
    fn each_lua_rule_fires_on_its_case() {
        let cases: &[(&str, RuleId)] = &[
            (
                "local pv = require 'privatium'\npv.get('/', function(req)\n  return pv.query('SELECT * FROM fill WHERE drug = \\'' .. req.form.drug .. '\\'')\nend)",
                RuleId::PV201,
            ),
            (
                "local pv = require 'privatium'\nlocal sql = 'SELECT ' .. x\npv.get('/', function() return pv.query(sql) end)",
                RuleId::PV201,
            ),
            ("local f = io.open('x')", RuleId::PV203),
            ("os.execute('ls')", RuleId::PV203),
            (
                "local pv = require 'privatium'\npv.get('/', function() return pv.redirect('/a/meds/') end)",
                RuleId::PV301,
            ),
            (
                "local pv = require 'privatium'\npv.get('/', function(req)\n local row = pv.get_row('fill', req.params.id)\n return tonumber(row.copay)\nend)",
                RuleId::PV302,
            ),
            (
                "local pv = require 'privatium'\npv.get('/', function(req)\n local row = pv.get_row('fill', req.params.id)\n return row.copay + 1\nend)",
                RuleId::PV302,
            ),
            (
                "local pv = require 'privatium'\npv.get('/', function() return pv.query('UPDATE fill SET copay = 1') end)",
                RuleId::PV303,
            ),
            (
                "local pv = require 'privatium'\npv.get('/', function() return pv.query('SELECT SUM(copay) FROM fill') end)",
                RuleId::PV308,
            ),
            (
                "local pv = require 'privatium'\npv.get('/', function() return pv.query('SELECT due_on + 30 FROM fill') end)",
                RuleId::PV308,
            ),
            ("local seen_txid = {}", RuleId::PV305),
            (
                "local pv = require 'privatium'\npv.post('/', function()\n  pv.append('fill', {a=1})\n  pv.append('fill', {b=2})\nend)",
                RuleId::PV306,
            ),
            (
                "local pv = require 'privatium'\npv.post('/', function()\n  last = 1\nend)",
                RuleId::PV307,
            ),
            (
                "local pv = require 'privatium'\nlocal cache = {}\npv.post('/', function()\n  cache['k'] = 1\nend)",
                RuleId::PV307,
            ),
            // Spelled with a gap so `cargo xtask icons-verify` does not read it as a name.
            (
                concat!("local x = icon", "('no-such-icon-xyz')"),
                RuleId::PV503,
            ),
            ("local p = '/home/me/notes.txt'", RuleId::PV505),
            (
                "local pv = require 'privatium'\npv.get('/settings', function() end)",
                RuleId::PV506,
            ),
        ];
        for (code, rule) in cases {
            let found = rules_in(code);
            assert!(
                found.iter().any(|(r, _)| r == rule),
                "{rule:?} did not fire on:\n{code}\nfound {found:?}"
            );
        }
    }

    #[test]
    fn the_reference_shapes_are_clean() {
        let clean = "local pv = require 'privatium'\nlocal M = {}\nfunction M.clean(s) return s end\n\
                     pv.get('/', function(req)\n  local rows = pv.query('SELECT * FROM fill WHERE drug = ?', {req.form.drug})\n\
                     local one = pv.query1('SELECT decimal_sum(copay) AS t, date(due_on, \\'+30 days\\') FROM fill')\n\
                     local h = tonumber(os.date('!%H'))\n  pv.batch(function(tx) tx.append('fill', {a=1}); tx.append('fill', {b=2}) end)\n\
                     return pv.redirect(url('/'))\nend)\nreturn M";
        assert!(rules_in(clean).is_empty(), "{:?}", rules_in(clean));
        let ast = full_moon::parse("local pv = require 'privatium'\npv.get('/x/:id', function() return pv.render('show', {}) end)").unwrap();
        let (_, facts) = walk(&ast, &env());
        assert_eq!(facts.routes, vec!["/x/:id"]);
        assert!(facts.rendered.contains("show"));
    }

    #[test]
    fn the_mount_path_fix_is_an_edit_over_the_literal() {
        let code = "local pv = require 'privatium'\npv.get('/', function() return pv.redirect('/a/meds/edit') end)";
        let ast = full_moon::parse(code).unwrap();
        let (pending, _) = walk(&ast, &env());
        let (start, end, replacement) = pending[0].edit.clone().unwrap();
        assert_eq!(&code[start..end], "'/a/meds/edit'");
        assert_eq!(replacement, "url('/edit')");
    }

    #[test]
    fn compiled_literals_decode() {
        let ast = full_moon::parse("local x = \"a\\\"b\\\\c\\n\\009d\"").unwrap();
        let Some(Stmt::LocalAssignment(local)) = ast.nodes().stmts().next() else {
            panic!("a local");
        };
        let Some(Expression::String(token)) = local.expressions().iter().next() else {
            panic!("a string");
        };
        assert_eq!(decode_literal(token).unwrap(), "a\"b\\c\n\td");
    }
}
