//! Mandatory sandbox: builds the whitelisted per-script environment, reroutes
//! `print` to engine logging, and installs the instruction-count budget hook.
//!
//! Denied globals are simply never copied into the environment, so `os`, `io`,
//! `package`, `require`, `debug`, `load`, `loadstring`, `dofile`, `loadfile`,
//! `collectgarbage`, `rawget`/`rawset`/`rawequal`/`rawlen`,
//! `getmetatable`/`setmetatable`, and `coroutine` resolve to `nil` inside a
//! script. The environment is per-script, so scripts never reach the real
//! globals.

use std::cell::Cell;
use std::rc::Rc;

use mlua::{HookTriggers, Lua, Result as LuaResult, Table, Value, Variadic};

use crate::math_binding;

/// The VM-hook instruction interval. The budget is enforced to within one
/// interval (documented), chosen coarse so per-instruction overhead is
/// negligible.
pub(crate) const HOOK_INTERVAL: u32 = 1000;

/// Shared remaining-budget counter decremented by the instruction hook. The
/// engine resets it to the configured budget before each call; the hook errors
/// once it reaches zero, surfacing as `ScriptError::BudgetExceeded`.
pub(crate) type BudgetCounter = Rc<Cell<u64>>;

/// Sentinel message raised by the budget hook so the engine can classify the
/// resulting Lua error as `BudgetExceeded` rather than a generic runtime error.
pub(crate) const BUDGET_SENTINEL: &str = "spawn-script:budget-exceeded";

/// Installs the instruction-count hook backed by `counter`. `counter` is shared
/// with the engine, which resets it per call. A `budget` of `0` disables the
/// hook (unlimited).
pub(crate) fn install_hook(lua: &Lua, counter: BudgetCounter, budget: u64) -> LuaResult<()> {
    if budget == 0 {
        return Ok(());
    }
    let interval = u64::from(HOOK_INTERVAL);
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_lua, _debug| {
            let remaining = counter.get();
            if remaining <= interval {
                counter.set(0);
                Err(mlua::Error::runtime(BUDGET_SENTINEL))
            } else {
                counter.set(remaining - interval);
                Ok(())
            }
        },
    );
    Ok(())
}

/// Builds a fresh sandboxed environment table for one script: whitelisted
/// stdlib, safe builtins, rerouted `print`, and the math constructors.
pub(crate) fn build_environment(lua: &Lua) -> LuaResult<Table<'_>> {
    let globals = lua.globals();
    let env = lua.create_table()?;

    for name in [
        "pairs", "ipairs", "next", "select", "type", "tostring", "tonumber", "assert", "error",
        "pcall", "xpcall", "unpack",
    ] {
        let value: Value = globals.get(name)?;
        if value != Value::Nil {
            env.set(name, value)?;
        }
    }

    // Each environment gets a shallow COPY of every stdlib table: Lua tables
    // are reference types, so sharing the global table would let one script
    // mutate (e.g. `string.rep = ...`) the stdlib seen by every other script.
    for lib in ["math", "string", "table"] {
        if let Value::Table(shared) = globals.get::<_, Value>(lib)? {
            let copy = lua.create_table()?;
            for pair in shared.pairs::<Value, Value>() {
                let (k, v) = pair?;
                copy.set(k, v)?;
            }
            if lib == "string" {
                copy.set("dump", Value::Nil)?;
            }
            env.set(lib, copy)?;
        }
    }

    let print = lua.create_function(|lua, args: Variadic<Value>| {
        let mut parts = Vec::with_capacity(args.len());
        for arg in args.iter() {
            let s = lua.coerce_string(arg.clone())?;
            match s {
                Some(s) => parts.push(s.to_str()?.to_owned()),
                None => parts.push(format!("{arg:?}")),
            }
        }
        log_line(&parts.join("\t"));
        Ok(())
    })?;
    env.set("print", print)?;

    math_binding::install(lua, &env)?;

    Ok(env)
}

/// Engine logging seam for script `print`. Phase 1 writes to stdout; spawn-debug
/// integration is a later phase.
fn log_line(line: &str) {
    println!("[script] {line}");
}
