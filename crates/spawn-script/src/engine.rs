//! The [`ScriptEngine`]: owns the Lua VM, loaded scripts, per-entity state
//! tables, native bindings, and the lifecycle invocation paths.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mlua::{Function, Lua, RegistryKey, Table, Value};
use spawn_core::Transform3D;
use spawn_ecs::{Entity, World};

use crate::config::{ScriptConfig, ScriptId};
use crate::entity_api::{make_entity, WorldCell};
use crate::error::{ScriptError, ScriptResult};
use crate::sandbox::{self, BudgetCounter, BUDGET_SENTINEL};
use crate::value::{self, ScriptValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptStatus {
    Loaded,
    Failed,
}

type BindingFn = dyn Fn(&mut ScriptContext, &[ScriptValue]) -> ScriptResult<ScriptValue>;

struct LoadedScript {
    name: String,
    env: RegistryKey,
    on_init: Option<RegistryKey>,
    on_update: Option<RegistryKey>,
    on_destroy: Option<RegistryKey>,
    status: ScriptStatus,
}

/// Controlled access to the ECS `World` granted to a native binding for the
/// duration of one lifecycle call. The only Phase 1 surface is `Transform3D`
/// access; nothing escapes the borrow.
pub struct ScriptContext<'a> {
    cell: Option<&'a WorldCell<'a>>,
}

impl ScriptContext<'_> {
    /// Reads `entity`'s `Transform3D`. `None` if the entity/component is absent
    /// or the call has no world bound (e.g. during `load_script`).
    pub fn get_transform(&self, entity: Entity) -> Option<Transform3D> {
        self.cell?.get_transform(entity)
    }

    /// Writes `entity`'s `Transform3D`. `false` if the entity/component is absent
    /// or the call has no world bound.
    pub fn set_transform(&mut self, entity: Entity, t: Transform3D) -> bool {
        match self.cell {
            Some(cell) => cell.set_transform(entity, t),
            None => false,
        }
    }
}

/// A sandboxed Lua 5.4 scripting runtime.
///
/// Owns exactly one Lua VM for its lifetime and is `!Send + !Sync` (the VM is
/// not shared across threads in Phase 1; the engine lives on the main/ECS
/// thread). Not `Clone`.
pub struct ScriptEngine {
    lua: Lua,
    config: ScriptConfig,
    budget: BudgetCounter,
    scripts: HashMap<u64, LoadedScript>,
    next_id: u64,
    bindings: Vec<(String, Rc<BindingFn>)>,
    state_tables: HashMap<(u64, Entity), RegistryKey>,
    active: HashSet<(u64, Entity)>,
}

impl ScriptEngine {
    /// Creates the VM, applies the memory limit, installs the budget hook, and
    /// prepares the sandbox. Returns [`ScriptError::Init`] on VM failure.
    pub fn new(config: ScriptConfig) -> ScriptResult<Self> {
        let lua = Lua::new();
        if config.memory_limit != 0 {
            lua.set_memory_limit(config.memory_limit)
                .map_err(|e| ScriptError::Init {
                    message: e.to_string(),
                })?;
        }
        let budget: BudgetCounter = Rc::new(Cell::new(config.instruction_budget));
        sandbox::install_hook(&lua, Rc::clone(&budget), config.instruction_budget).map_err(
            |e| ScriptError::Init {
                message: e.to_string(),
            },
        )?;

        Ok(Self {
            lua,
            config,
            budget,
            scripts: HashMap::new(),
            next_id: 1,
            bindings: Vec::new(),
            state_tables: HashMap::new(),
            active: HashSet::new(),
        })
    }

    pub fn config(&self) -> ScriptConfig {
        self.config
    }

    fn reset_budget(&self) {
        self.budget.set(self.config.instruction_budget);
    }

    fn classify(&self, name: &str, err: &mlua::Error) -> ScriptError {
        let text = err.to_string();
        if text.contains(BUDGET_SENTINEL) {
            ScriptError::BudgetExceeded {
                script: name.to_owned(),
            }
        } else if matches!(err, mlua::Error::MemoryError(_)) || text.contains("not enough memory") {
            ScriptError::MemoryExceeded {
                script: name.to_owned(),
            }
        } else {
            ScriptError::Runtime {
                script: name.to_owned(),
                traceback: text,
            }
        }
    }

    /// Compiles and executes `source` in a fresh sandboxed environment, caching
    /// any `on_init`/`on_update`/`on_destroy` references. The top-level body runs
    /// under the instruction budget.
    pub fn load_script(&mut self, name: &str, source: &str) -> ScriptResult<ScriptId> {
        let loaded = self.build_loaded(name, source)?;
        let id = self.next_id;
        self.next_id += 1;
        self.scripts.insert(id, loaded);
        Ok(ScriptId::new(id))
    }

    /// Compiles `source` into a fresh sandboxed environment, runs its top-level
    /// body under the budget, and caches lifecycle refs. Returns an owned
    /// [`LoadedScript`] (registry keys only), so no `self.lua` borrow escapes.
    fn build_loaded(&mut self, name: &str, source: &str) -> ScriptResult<LoadedScript> {
        let env = sandbox::build_environment(&self.lua).map_err(|e| ScriptError::Load {
            script: name.to_owned(),
            line: None,
            message: e.to_string(),
        })?;

        self.reset_budget();
        let cell: Option<&WorldCell> = None;
        let run = self.lua.scope(|scope| {
            self.inject_bindings(scope, &env, cell)?;
            let chunk = self
                .lua
                .load(source)
                .set_name(name)
                .set_environment(env.clone());
            chunk.exec()
        });
        if let Err(e) = run {
            return Err(self.classify_load(name, &e));
        }

        let resolve = |key: &str| -> ScriptResult<Option<RegistryKey>> {
            match env.get::<_, Value>(key) {
                Ok(Value::Function(f)) => {
                    let rk =
                        self.lua
                            .create_registry_value(f)
                            .map_err(|_| ScriptError::Conversion {
                                context: "cache fn ref",
                            })?;
                    Ok(Some(rk))
                }
                _ => Ok(None),
            }
        };
        let on_init = resolve("on_init")?;
        let on_update = resolve("on_update")?;
        let on_destroy = resolve("on_destroy")?;
        let env_key = self
            .lua
            .create_registry_value(env)
            .map_err(|_| ScriptError::Conversion {
                context: "cache env",
            })?;
        Ok(LoadedScript {
            name: name.to_owned(),
            env: env_key,
            on_init,
            on_update,
            on_destroy,
            status: ScriptStatus::Loaded,
        })
    }

    fn classify_load(&self, name: &str, err: &mlua::Error) -> ScriptError {
        let text = err.to_string();
        if text.contains(BUDGET_SENTINEL) {
            return ScriptError::BudgetExceeded {
                script: name.to_owned(),
            };
        }
        if let mlua::Error::SyntaxError { message, .. } = err {
            return ScriptError::Load {
                script: name.to_owned(),
                line: None,
                message: message.clone(),
            };
        }
        ScriptError::Load {
            script: name.to_owned(),
            line: None,
            message: text,
        }
    }

    /// Drops the script's environment and cached refs. Does not call
    /// `on_destroy` (that is the runner system's per-entity job).
    pub fn unload(&mut self, id: ScriptId) -> ScriptResult<()> {
        let script = self
            .scripts
            .remove(&id.index())
            .ok_or(ScriptError::UnknownScript)?;
        self.expire(script);
        Ok(())
    }

    fn expire(&self, script: LoadedScript) {
        let _ = self.lua.remove_registry_value(script.env);
        for key in [script.on_init, script.on_update, script.on_destroy]
            .into_iter()
            .flatten()
        {
            let _ = self.lua.remove_registry_value(key);
        }
    }

    /// Hot-reloads `source` under the existing `name`, replacing the environment
    /// and cached refs in place so the [`ScriptId`] stays valid. On
    /// compile/runtime error the old script remains active and the error is
    /// returned. Per-entity `state` tables are preserved (not reset).
    pub fn reload(&mut self, id: ScriptId, source: &str) -> ScriptResult<()> {
        let name = self
            .scripts
            .get(&id.index())
            .map(|s| s.name.clone())
            .ok_or(ScriptError::UnknownScript)?;
        let fresh = self.build_loaded(&name, source)?;
        if let Some(old) = self.scripts.insert(id.index(), fresh) {
            self.expire(old);
        }
        Ok(())
    }

    pub fn is_loaded(&self, id: ScriptId) -> bool {
        self.scripts.contains_key(&id.index())
    }

    /// Returns the current status of `id`, or `None` if the id is unknown
    /// (never loaded, or already unloaded).
    pub fn status(&self, id: ScriptId) -> Option<ScriptStatus> {
        self.scripts.get(&id.index()).map(|s| s.status)
    }

    /// Exposes a Rust function to all scripts under global `name`. Arguments
    /// arrive as a [`ScriptValue`] slice; the return is one [`ScriptValue`]
    /// ([`ScriptValue::Nil`] for "no return"). Must be registered before scripts
    /// that use it are loaded. A duplicate `name` returns
    /// [`ScriptError::InvalidArgument`].
    pub fn register_fn<F>(&mut self, name: &str, f: F) -> ScriptResult<()>
    where
        F: Fn(&mut ScriptContext, &[ScriptValue]) -> ScriptResult<ScriptValue> + 'static,
    {
        if self.bindings.iter().any(|(n, _)| n == name) {
            return Err(ScriptError::InvalidArgument {
                context: "duplicate binding name",
            });
        }
        self.bindings.push((name.to_owned(), Rc::new(f)));
        Ok(())
    }

    fn inject_bindings<'lua, 'scope>(
        &'lua self,
        scope: &mlua::Scope<'lua, 'scope>,
        env: &Table<'lua>,
        cell: Option<&'scope WorldCell<'scope>>,
    ) -> mlua::Result<()>
    where
        'lua: 'scope,
    {
        for (name, f) in &self.bindings {
            let f = Rc::clone(f);
            let func = scope.create_function(move |lua, args: mlua::Variadic<Value>| {
                let mut converted = Vec::with_capacity(args.len());
                for a in args.iter() {
                    converted
                        .push(value::from_lua(a).map_err(|e| mlua::Error::runtime(e.to_string()))?);
                }
                let mut ctx = ScriptContext { cell };
                let result =
                    f(&mut ctx, &converted).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                value::to_lua(lua, &result).map_err(|e| mlua::Error::runtime(e.to_string()))
            })?;
            env.set(name.as_str(), func)?;
        }
        Ok(())
    }

    /// Invokes cached `on_init(entity)`; no-op if undefined.
    pub fn call_init(&mut self, id: ScriptId, entity: Entity) -> ScriptResult<()> {
        self.call_lifecycle(id, entity, None, LifecycleKind::Init)
    }

    /// Invokes cached `on_update(entity, dt)`; no-op if undefined.
    pub fn call_update(&mut self, id: ScriptId, entity: Entity, dt: f32) -> ScriptResult<()> {
        self.call_lifecycle(id, entity, Some(dt), LifecycleKind::Update)
    }

    /// Invokes cached `on_destroy(entity)`; no-op if undefined.
    pub fn call_destroy(&mut self, id: ScriptId, entity: Entity) -> ScriptResult<()> {
        self.call_lifecycle(id, entity, None, LifecycleKind::Destroy)
    }

    fn call_lifecycle(
        &mut self,
        id: ScriptId,
        entity: Entity,
        dt: Option<f32>,
        kind: LifecycleKind,
    ) -> ScriptResult<()> {
        self.call_with_world(id, entity, dt, kind, None)
    }

    /// The world-bound lifecycle path used by the runner system. `world` is
    /// `None` only for the bare `call_*` convenience wrappers (entity-API calls
    /// then no-op).
    pub(crate) fn call_with_world(
        &mut self,
        id: ScriptId,
        entity: Entity,
        dt: Option<f32>,
        kind: LifecycleKind,
        world: Option<&mut World>,
    ) -> ScriptResult<()> {
        let name = match self.scripts.get(&id.index()) {
            Some(s) if s.status == ScriptStatus::Failed => {
                return Err(ScriptError::ScriptFailed {
                    script: s.name.clone(),
                })
            }
            Some(s) => s.name.clone(),
            None => return Err(ScriptError::UnknownScript),
        };

        if self.lifecycle_key(&id, kind).is_none() {
            return Ok(());
        }

        // `&mut self` phase: ensure the per-entity state table exists, then take
        // only immutable `self.lua` borrows for the call itself.
        self.ensure_state_table(id, entity)?;
        self.reset_budget();

        let result = self.run_call(id, entity, dt, kind, world);

        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                let err = self.classify(&name, &e);
                if let Some(s) = self.scripts.get_mut(&id.index()) {
                    s.status = ScriptStatus::Failed;
                }
                Err(err)
            }
        }
    }

    /// Immutable-borrow phase of a lifecycle call: fetches cached func/env/state
    /// from the registry and runs the script body inside the `mlua` scope. The
    /// caller has already ensured the state table exists and reset the budget.
    fn run_call(
        &self,
        id: ScriptId,
        entity: Entity,
        dt: Option<f32>,
        kind: LifecycleKind,
        world: Option<&mut World>,
    ) -> mlua::Result<()> {
        let func_key = match self.lifecycle_key(&id, kind) {
            Some(k) => k,
            None => return Ok(()),
        };
        let func: Function = self.lua.registry_value(func_key)?;
        let env: Table = self.lua.registry_value(&self.scripts[&id.index()].env)?;
        let state_table: Table = self
            .lua
            .registry_value(&self.state_tables[&(id.index(), entity)])?;
        env.set("state", state_table)?;

        let lua = &self.lua;
        match world {
            Some(world) => {
                let cell = WorldCell::new(world);
                lua.scope(|scope| {
                    self.inject_bindings(scope, &env, Some(&cell))?;
                    let ent = make_entity(lua, scope, Some(&cell), entity)?;
                    Self::dispatch(&func, ent, dt)
                })
            }
            None => lua.scope(|scope| {
                self.inject_bindings(scope, &env, None)?;
                let ent = make_entity(lua, scope, None, entity)?;
                Self::dispatch(&func, ent, dt)
            }),
        }
    }

    fn dispatch(func: &Function, entity: Value, dt: Option<f32>) -> mlua::Result<()> {
        match dt {
            Some(dt) => func.call::<_, ()>((entity, dt)),
            None => func.call::<_, ()>(entity),
        }
    }

    fn lifecycle_key(&self, id: &ScriptId, kind: LifecycleKind) -> Option<&RegistryKey> {
        let s = self.scripts.get(&id.index())?;
        match kind {
            LifecycleKind::Init => s.on_init.as_ref(),
            LifecycleKind::Update => s.on_update.as_ref(),
            LifecycleKind::Destroy => s.on_destroy.as_ref(),
        }
    }

    /// Ensures the persistent per-entity `state` table exists, creating it on
    /// first use. Keyed by `(script, entity)` so it survives across frames and
    /// across hot-reload (which replaces the environment, not the state map).
    fn ensure_state_table(&mut self, id: ScriptId, entity: Entity) -> ScriptResult<()> {
        let key = (id.index(), entity);
        if self.state_tables.contains_key(&key) {
            return Ok(());
        }
        let table = self
            .lua
            .create_table()
            .map_err(|_| ScriptError::Conversion {
                context: "create state table",
            })?;
        let rk = self
            .lua
            .create_registry_value(table)
            .map_err(|_| ScriptError::Conversion {
                context: "cache state table",
            })?;
        self.state_tables.insert(key, rk);
        Ok(())
    }

    /// Drops a per-entity state table, used by the runner system after
    /// `on_destroy`.
    pub(crate) fn drop_state(&mut self, id: ScriptId, entity: Entity) {
        if let Some(rk) = self.state_tables.remove(&(id.index(), entity)) {
            let _ = self.lua.remove_registry_value(rk);
        }
    }

    pub(crate) fn mark_active(&mut self, id: ScriptId, entity: Entity) {
        self.active.insert((id.index(), entity));
    }

    pub(crate) fn clear_active(&mut self, id: ScriptId, entity: Entity) {
        self.active.remove(&(id.index(), entity));
    }

    /// Snapshot of `(ScriptId, Entity)` pairs that completed `on_init` and have
    /// not yet been destroyed. Used by the runner to detect entities whose
    /// `Script` was removed or despawned this frame.
    pub(crate) fn active_pairs(&self) -> Vec<(ScriptId, Entity)> {
        self.active
            .iter()
            .map(|&(id, entity)| (ScriptId::new(id), entity))
            .collect()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum LifecycleKind {
    Init,
    Update,
    Destroy,
}
