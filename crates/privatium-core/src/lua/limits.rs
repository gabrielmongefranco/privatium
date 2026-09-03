// Project:  Privatium™  |  File: crates/privatium-core/src/lua/limits.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The per-request resource limits of spec/lua-api.md §5, all four from
//           [lua] in config.toml: the instruction count and the wall clock in one debug hook
//           installed before any app code runs, the memory limit in the allocator, and the
//           same deadline handed to SQLite's progress handler for the time a statement
//           spends in Rust where the hook cannot fire. A tripped limit is remembered here,
//           so a handler that catches the error with pcall still fails the request.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mlua::{HookTriggers, Lua, VmState};

use crate::config::LuaConfig;

/// How many VM instructions run between two hook calls while nothing is wrong. The count
/// is therefore accurate to one tick, which against the 50,000,000 default is noise; the
/// hook itself costs a callback per tick, which is why it is not 1.
pub const TICK: u32 = 1000;

/// Which of the limits was exceeded — the `limit` in the `lua.limit_exceeded` audit row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// `lua.max_instructions`.
    Instructions,
    /// `lua.max_memory_mb`.
    Memory,
    /// `lua.max_seconds`.
    Seconds,
}

impl LimitKind {
    /// The audit row's spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instructions => "instructions",
            Self::Memory => "memory",
            Self::Seconds => "seconds",
        }
    }
}

impl fmt::Display for LimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One VM's limit state, armed once per run (a load, a request, or an append dispatch).
///
/// Shared between the hook closure, the connection's progress handler and the host that
/// reads the verdict afterwards, so it lives behind an `Arc`.
#[derive(Debug)]
pub struct Limits {
    max_instructions: u64,
    max_seconds: u64,
    /// Instructions executed since `arm`, in units of [`TICK`].
    executed: AtomicU64,
    deadline: Mutex<Instant>,
    tripped: Mutex<Option<LimitKind>>,
}

impl Limits {
    /// Fresh and disarmed: the deadline is now, so [`arm`](Self::arm) has to run first.
    #[must_use]
    pub fn new(config: &LuaConfig) -> Self {
        Self {
            max_instructions: config.max_instructions,
            max_seconds: config.max_seconds,
            executed: AtomicU64::new(0),
            deadline: Mutex::new(Instant::now()),
            tripped: Mutex::new(None),
        }
    }

    /// Start a run: reset the count, set the deadline, forget any earlier verdict.
    pub fn arm(&self) {
        self.executed.store(0, Ordering::Relaxed);
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) =
            Instant::now() + Duration::from_secs(self.max_seconds);
        *self.tripped.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The limit that was exceeded during this run, if any.
    #[must_use]
    pub fn tripped(&self) -> Option<LimitKind> {
        *self.tripped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Record a verdict. The first one stands.
    pub fn trip(&self, kind: LimitKind) {
        let mut tripped = self.tripped.lock().unwrap_or_else(PoisonError::into_inner);
        if tripped.is_none() {
            *tripped = Some(kind);
        }
    }

    /// Instructions counted so far, to the nearest tick.
    #[must_use]
    pub fn executed(&self) -> u64 {
        self.executed.load(Ordering::Relaxed)
    }

    /// `lua.max_instructions`.
    #[must_use]
    pub fn max_instructions(&self) -> u64 {
        self.max_instructions
    }

    /// `lua.max_seconds`.
    #[must_use]
    pub fn max_seconds(&self) -> u64 {
        self.max_seconds
    }

    fn deadline(&self) -> Instant {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the wall clock has run out — the connection's progress handler, called by
    /// SQLite every few thousand virtual-machine steps while a statement runs. Returning
    /// `true` interrupts the statement; the verdict is recorded first, so the request fails
    /// even if the handler catches the SQL error.
    pub fn over_time(&self) -> bool {
        if Instant::now() > self.deadline() {
            self.trip(LimitKind::Seconds);
            true
        } else {
            false
        }
    }

    /// The Lua error a tripped limit raises.
    #[must_use]
    pub fn error(kind: LimitKind) -> mlua::Error {
        mlua::Error::runtime(format!(
            "request limit exceeded: {kind} (spec/lua-api.md §5); the request fails whether \
             or not this error is caught"
        ))
    }
}

/// Install the instruction-count and wall-clock hook on a fresh state, before any app code
/// runs (`spec/lua-api.md §5`).
///
/// A global hook: Lua copies a thread's hook into every coroutine it creates, so a loop
/// inside `coroutine.wrap` is counted like any other.
pub fn install(lua: &Lua, limits: Arc<Limits>) -> mlua::Result<()> {
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(TICK),
        move |lua, _| hook(lua, &limits),
    )
}

/// The hook body. Once a limit has tripped, every later call errors again, and the hook is
/// re-armed to fire on *every* instruction: a handler that swallows the error with `pcall`
/// then gets it again at its very next instruction, outside the `pcall`, however the
/// instruction count happened to line up with its loops.
fn hook(lua: &Lua, limits: &Arc<Limits>) -> mlua::Result<VmState> {
    if let Some(kind) = limits.tripped() {
        escalate(lua, limits);
        return Err(Limits::error(kind));
    }
    let executed = limits
        .executed
        .fetch_add(u64::from(TICK), Ordering::Relaxed)
        .saturating_add(u64::from(TICK));
    let kind = if executed > limits.max_instructions {
        Some(LimitKind::Instructions)
    } else if Instant::now() > limits.deadline() {
        Some(LimitKind::Seconds)
    } else {
        None
    };
    match kind {
        Some(kind) => {
            limits.trip(kind);
            escalate(lua, limits);
            Err(Limits::error(kind))
        }
        None => Ok(VmState::Continue),
    }
}

/// Fire on every instruction from now on, on the main thread and on the coroutine that is
/// running, if one is. A failure to re-arm is ignored: the verdict is already recorded and
/// the VM is discarded when the run ends.
fn escalate(lua: &Lua, limits: &Arc<Limits>) {
    let every = HookTriggers::new().every_nth_instruction(1);
    let shared = Arc::clone(limits);
    let _ = lua.set_global_hook(every, move |lua, _| hook(lua, &shared));
    let shared = Arc::clone(limits);
    let _ = lua
        .current_thread()
        .set_hook(every, move |lua, _| hook(lua, &shared));
}

/// Whether an error from the VM is Lua's own out-of-memory, raised when an allocation
/// would pass `lua.max_memory_mb`. It can be wrapped by a callback frame.
#[must_use]
pub fn is_memory_error(error: &mlua::Error) -> bool {
    match error {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } => is_memory_error(cause),
        _ => false,
    }
}
