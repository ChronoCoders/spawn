//! The once-per-frame script-runner system.

use std::collections::HashSet;

use spawn_ecs::{Entity, World};

use crate::component::{Script, ScriptLifecycle};
use crate::config::ScriptId;
use crate::engine::{LifecycleKind, ScriptEngine, ScriptStatus};

/// Drives script lifecycles for all entities carrying a [`Script`], once per
/// frame on the main thread.
///
/// Order: `on_init` for `Pending` entities (→ `Active`/`Failed`), then
/// `on_update` for `Active` entities (errors → `Failed`), then `on_destroy`
/// exactly once for entities whose `Script` was removed or despawned this frame
/// while `Active`. One entity's script error never aborts the loop.
pub fn script_runner_system(world: &mut World, engine: &mut ScriptEngine, dt: f32) {
    let snapshot: Vec<(Entity, ScriptId, ScriptLifecycle)> = world
        .query::<(Entity, &Script)>()
        .iter()
        .map(|(entity, script)| (entity, script.script, script.state.lifecycle()))
        .collect();

    let mut present: HashSet<(u64, Entity)> = HashSet::with_capacity(snapshot.len());
    for &(entity, id, _) in &snapshot {
        present.insert((id.index(), entity));
    }

    for (entity, id, lifecycle) in snapshot {
        match lifecycle {
            ScriptLifecycle::Pending => {
                let result =
                    engine.call_with_world(id, entity, None, LifecycleKind::Init, Some(world));
                let next = match result {
                    Ok(()) if engine.status(id) == Some(ScriptStatus::Failed) => {
                        ScriptLifecycle::Failed
                    }
                    Ok(()) => {
                        engine.mark_active(id, entity);
                        ScriptLifecycle::Active
                    }
                    Err(_) => ScriptLifecycle::Failed,
                };
                set_lifecycle(world, entity, next);
            }
            ScriptLifecycle::Active => {
                let result = engine.call_with_world(
                    id,
                    entity,
                    Some(dt),
                    LifecycleKind::Update,
                    Some(world),
                );
                if result.is_err() {
                    set_lifecycle(world, entity, ScriptLifecycle::Failed);
                }
            }
            ScriptLifecycle::Failed => {}
        }
    }

    for (id, entity) in engine.active_pairs() {
        if present.contains(&(id.index(), entity)) {
            continue;
        }
        let _ = engine.call_with_world(id, entity, None, LifecycleKind::Destroy, Some(world));
        engine.clear_active(id, entity);
        engine.drop_state(id, entity);
    }
}

fn set_lifecycle(world: &mut World, entity: Entity, lifecycle: ScriptLifecycle) {
    if let Some(script) = world.get_mut::<Script>(entity) {
        script.state.set_lifecycle(lifecycle);
    }
}
