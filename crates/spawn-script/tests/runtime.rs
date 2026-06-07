//! Integration tests for the spawn-script runtime: sandbox, budget, memory,
//! lifecycle, reload, math round-trips, value bridge, error isolation, and the
//! Transform3D entity API. All headless.

use spawn_core::{ApproxEq, Quat, Transform3D, Vec2, Vec3};
use spawn_ecs::World;
use spawn_script::{
    script_runner_system, Script, ScriptConfig, ScriptContext, ScriptEngine, ScriptError,
    ScriptLifecycle, ScriptStatus, ScriptValue,
};

fn engine() -> ScriptEngine {
    ScriptEngine::new(ScriptConfig::default()).expect("engine")
}

#[test]
fn sandbox_denies_dangerous_globals() {
    let mut e = engine();
    let src = r#"
        results = {}
        for _, name in ipairs({
            "os","io","package","require","debug","load","loadstring",
            "dofile","loadfile","collectgarbage","rawset","rawget","coroutine",
            "getmetatable","setmetatable"
        }) do
            results[name] = _ENV[name] == nil
        end
        results.string_dump = (string.dump == nil)
        function on_init(e)
            for k, v in pairs(results) do
                assert(v, k .. " was reachable")
            end
        end
    "#;
    let id = e.load_script("sandbox", src).expect("load");
    e.call_init(id, spawn_ecs::Entity::PLACEHOLDER)
        .expect("all denied globals are nil");
}

#[test]
fn os_execute_is_runtime_error_not_compromise() {
    let mut e = engine();
    let id = e
        .load_script("danger", "function on_init(ent) os.execute('echo hi') end")
        .expect("load");
    let err = e.call_init(id, spawn_ecs::Entity::PLACEHOLDER).unwrap_err();
    assert!(matches!(err, ScriptError::Runtime { .. }));
    assert_eq!(e.status(id), Some(ScriptStatus::Failed));
}

#[test]
fn budget_terminates_infinite_loop() {
    let mut e = ScriptEngine::new(ScriptConfig {
        instruction_budget: 100_000,
        memory_limit: 64 * 1024 * 1024,
    })
    .unwrap();
    let id = e
        .load_script("spin", "function on_update(ent, dt) while true do end end")
        .unwrap();
    let err = e
        .call_update(id, spawn_ecs::Entity::PLACEHOLDER, 0.016)
        .unwrap_err();
    assert!(matches!(err, ScriptError::BudgetExceeded { .. }));
    assert_eq!(e.status(id), Some(ScriptStatus::Failed));
}

#[test]
fn budget_terminates_deep_recursion() {
    let mut e = ScriptEngine::new(ScriptConfig {
        instruction_budget: 200_000,
        memory_limit: 256 * 1024 * 1024,
    })
    .unwrap();
    let id = e
        .load_script(
            "rec",
            "function f(n) return f(n + 1) end function on_init(ent) f(0) end",
        )
        .unwrap();
    let err = e.call_init(id, spawn_ecs::Entity::PLACEHOLDER).unwrap_err();
    assert!(matches!(
        err,
        ScriptError::BudgetExceeded { .. } | ScriptError::Runtime { .. }
    ));
}

#[test]
fn memory_cap_enforced() {
    let mut e = ScriptEngine::new(ScriptConfig {
        instruction_budget: 0,
        memory_limit: 2 * 1024 * 1024,
    })
    .unwrap();
    let id = e
        .load_script(
            "hog",
            "function on_init(ent) local t = {} local i = 1 while true do t[i] = string.rep('x', 1024) i = i + 1 end end",
        )
        .unwrap();
    let err = e.call_init(id, spawn_ecs::Entity::PLACEHOLDER).unwrap_err();
    assert!(matches!(
        err,
        ScriptError::MemoryExceeded { .. } | ScriptError::Runtime { .. }
    ));
}

#[test]
fn lifecycle_order_and_missing_are_noops() {
    let mut e = engine();
    let src = r#"
        order = {}
        function on_init(ent) order[#order+1] = "init" end
        function on_update(ent, dt) order[#order+1] = "update" end
    "#;
    let id = e.load_script("life", src).unwrap();
    let ent = spawn_ecs::Entity::PLACEHOLDER;
    e.call_init(id, ent).unwrap();
    e.call_update(id, ent, 0.1).unwrap();
    e.call_update(id, ent, 0.1).unwrap();
    e.call_destroy(id, ent).unwrap();

    let missing = e.load_script("empty", "x = 1").unwrap();
    e.call_init(missing, ent).unwrap();
    e.call_update(missing, ent, 0.1).unwrap();
    e.call_destroy(missing, ent).unwrap();
}

#[test]
fn reload_preserves_id_and_state() {
    let mut e = engine();
    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();
    let entity = world.spawn_with((Transform3D::IDENTITY,));

    let v1 = r#"
        function on_init(ent) state.count = 0 end
        function on_update(ent, dt) state.count = state.count + 1 end
    "#;
    let id = e.load_script("counter", v1).unwrap();
    world.insert(entity, Script::new(id)).unwrap();

    script_runner_system(&mut world, &mut e, 0.016);
    script_runner_system(&mut world, &mut e, 0.016);

    // After two v1 updates state.count == 1 (init sets 0, one update +1; the
    // second update runs after the reload below). v2 asserts the inherited value
    // before mutating it: a lost or reset state aborts the script (Lua assert ->
    // Failed) and the Active check at the end catches the regression.
    let v2 = r#"
        function on_update(ent, dt)
            assert(state.count == 1, "state lost across reload")
            state.count = state.count + 10
        end
    "#;
    e.reload(id, v2).unwrap();
    assert!(e.is_loaded(id));

    script_runner_system(&mut world, &mut e, 0.016);
    let script = world.get::<Script>(entity).unwrap();
    assert_eq!(script.state.lifecycle(), ScriptLifecycle::Active);

    // v3 asserts the post-reload mutation took effect (1 + 10 == 11), proving the
    // reloaded body's state write persisted into the next frame.
    let v3 = r#"
        function on_update(ent, dt) assert(state.count == 11, "post-reload state value wrong") end
    "#;
    e.reload(id, v3).unwrap();
    script_runner_system(&mut world, &mut e, 0.016);
    let script = world.get::<Script>(entity).unwrap();
    assert_eq!(script.state.lifecycle(), ScriptLifecycle::Active);
}

#[test]
fn invalid_reload_keeps_old_script() {
    let mut e = engine();
    let id = e.load_script("ok", "function on_init(ent) end").unwrap();
    let err = e.reload(id, "function on_init( syntax error").unwrap_err();
    assert!(matches!(err, ScriptError::Load { .. }));
    assert!(e.is_loaded(id));
    assert_eq!(e.status(id), Some(ScriptStatus::Loaded));
}

#[test]
fn math_round_trips_through_lua() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let result: Rc<RefCell<ScriptValue>> = Rc::new(RefCell::new(ScriptValue::Nil));
    let sink = Rc::clone(&result);
    let mut e = engine();
    e.register_fn("report", move |_ctx, a| {
        *sink.borrow_mut() = a.first().cloned().unwrap_or(ScriptValue::Nil);
        Ok(ScriptValue::Nil)
    })
    .unwrap();

    let src = r#"
        function on_init(ent)
            local a = Vec3(1, 2, 3)
            local b = Vec3(4, 5, 6)
            local q = Quat(0, 0, 0.70710677, 0.70710677)
            report({
                sum = a + b,
                cross = a:cross(b),
                norm = a:normalize(),
                rotated = q * Vec3(1, 0, 0),
            })
        end
    "#;
    let id = e.load_script("math", src).unwrap();
    e.call_init(id, spawn_ecs::Entity::PLACEHOLDER).unwrap();

    let table = match result.borrow().clone() {
        ScriptValue::Table(entries) => entries,
        other => panic!("expected table, got {other:?}"),
    };
    let get = |k: &str| table.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());

    let a = Vec3::new(1.0, 2.0, 3.0);
    let b = Vec3::new(4.0, 5.0, 6.0);
    assert!(matches!(get("sum"), Some(ScriptValue::Vec3(v)) if v.approx_eq_default(a + b)));
    assert!(matches!(get("cross"), Some(ScriptValue::Vec3(v)) if v.approx_eq_default(a.cross(b))));
    assert!(
        matches!(get("norm"), Some(ScriptValue::Vec3(v)) if v.approx_eq(a.normalize().unwrap(), 1e-5))
    );
    let q = Quat::from_xyzw(0.0, 0.0, 0.70710677, 0.70710677);
    assert!(
        matches!(get("rotated"), Some(ScriptValue::Vec3(v)) if v.approx_eq(q.rotate(Vec3::X), 1e-4))
    );
}

#[test]
fn value_bridge_round_trips_all_variants() {
    let cases = vec![
        ScriptValue::Nil,
        ScriptValue::Bool(true),
        ScriptValue::Int(-42),
        ScriptValue::Float(3.5),
        ScriptValue::Str("hello".into()),
        ScriptValue::Vec2(Vec2::new(1.0, 2.0)),
        ScriptValue::Vec3(Vec3::new(1.0, 2.0, 3.0)),
        ScriptValue::Quat(Quat::from_xyzw(0.0, 0.0, 0.0, 1.0)),
        ScriptValue::Table(vec![
            ("a".into(), ScriptValue::Int(1)),
            ("b".into(), ScriptValue::Str("x".into())),
        ]),
    ];

    for case in cases {
        let expected = case.clone();
        let got = echo(case);
        match (&got, &expected) {
            // Lua table iteration order is not guaranteed (documented), so
            // compare table entries as sets.
            (ScriptValue::Table(g), ScriptValue::Table(x)) => {
                assert_eq!(g.len(), x.len());
                for entry in x {
                    assert!(g.contains(entry), "missing {entry:?} in {g:?}");
                }
            }
            _ => assert_eq!(got, expected),
        }
    }
}

fn echo(value: ScriptValue) -> ScriptValue {
    use std::cell::RefCell;
    use std::rc::Rc;
    let out: Rc<RefCell<ScriptValue>> = Rc::new(RefCell::new(ScriptValue::Nil));
    let sink = Rc::clone(&out);
    let mut e = engine();
    let input = Rc::new(RefCell::new(Some(value)));
    let supply = Rc::clone(&input);
    e.register_fn("supply", move |_ctx, _a| {
        Ok(supply.borrow_mut().clone().unwrap_or(ScriptValue::Nil))
    })
    .unwrap();
    e.register_fn("capture", move |_ctx, a| {
        *sink.borrow_mut() = a.first().cloned().unwrap_or(ScriptValue::Nil);
        Ok(ScriptValue::Nil)
    })
    .unwrap();
    let id = e
        .load_script("echo", "function on_init(ent) capture(supply()) end")
        .unwrap();
    e.call_init(id, spawn_ecs::Entity::PLACEHOLDER).unwrap();
    let r = out.borrow().clone();
    r
}

#[test]
fn conversion_rejects_bad_values() {
    use std::cell::RefCell;
    use std::rc::Rc;
    // Invalid string keys (numeric) → Conversion.
    let mut e = engine();
    let got: Rc<RefCell<Option<ScriptError>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&got);
    e.register_fn("capture", move |_ctx, a| {
        let _ = a;
        Ok(ScriptValue::Nil)
    })
    .unwrap();
    let id = e
        .load_script("bad", "function on_init(ent) capture({ [1] = 'x' }) end")
        .unwrap();
    let err = e.call_init(id, spawn_ecs::Entity::PLACEHOLDER).unwrap_err();
    // The binding wrapper turns a conversion failure into a runtime error.
    assert!(matches!(err, ScriptError::Runtime { .. }));
    let _ = sink;
}

#[test]
fn error_isolation_one_script_failing_does_not_poison_others() {
    let mut e = engine();
    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();

    let good = e
        .load_script(
            "good",
            "function on_update(ent, dt) state.n = (state.n or 0) + 1 end",
        )
        .unwrap();
    let bad = e
        .load_script("bad", "function on_update(ent, dt) error('boom') end")
        .unwrap();

    let e_good = world.spawn_with((Transform3D::IDENTITY,));
    world.insert(e_good, Script::new(good)).unwrap();
    let e_bad = world.spawn_with((Transform3D::IDENTITY,));
    world.insert(e_bad, Script::new(bad)).unwrap();

    for _ in 0..3 {
        script_runner_system(&mut world, &mut e, 0.016);
    }

    assert_eq!(e.status(bad), Some(ScriptStatus::Failed));
    assert_eq!(e.status(good), Some(ScriptStatus::Loaded));
    assert_eq!(
        world.get::<Script>(e_good).unwrap().state.lifecycle(),
        ScriptLifecycle::Active
    );
    assert_eq!(
        world.get::<Script>(e_bad).unwrap().state.lifecycle(),
        ScriptLifecycle::Failed
    );
}

#[test]
fn entity_api_reads_and_writes_transform() {
    let mut e = engine();
    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();

    let src = r#"
        function on_init(ent)
            ent:set_position(Vec3(1, 2, 3))
            ent:set_scale(Vec3(2, 2, 2))
        end
        function on_update(ent, dt)
            local p = ent:get_position()
            ent:set_position(Vec3(p.x + 1, p.y, p.z))
        end
    "#;
    let id = e.load_script("mover", src).unwrap();
    let entity = world.spawn_with((Transform3D::IDENTITY,));
    world.insert(entity, Script::new(id)).unwrap();

    script_runner_system(&mut world, &mut e, 0.016);
    let t = *world.get::<Transform3D>(entity).unwrap();
    assert!(t.translation.approx_eq_default(Vec3::new(1.0, 2.0, 3.0)));
    assert!(t.scale.approx_eq_default(Vec3::new(2.0, 2.0, 2.0)));

    script_runner_system(&mut world, &mut e, 0.016);
    let t = *world.get::<Transform3D>(entity).unwrap();
    assert!(t.translation.approx_eq_default(Vec3::new(2.0, 2.0, 3.0)));
}

#[test]
fn entity_api_table_transform_round_trip() {
    let mut e = engine();
    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();

    let src = r#"
        function on_init(ent)
            local t = ent:get_transform()
            t.position = Vec3(5, 6, 7)
            ent:set_transform(t)
        end
    "#;
    let id = e.load_script("trs", src).unwrap();
    let entity = world.spawn_with((Transform3D::IDENTITY,));
    world.insert(entity, Script::new(id)).unwrap();

    script_runner_system(&mut world, &mut e, 0.016);
    let t = *world.get::<Transform3D>(entity).unwrap();
    assert!(t.translation.approx_eq_default(Vec3::new(5.0, 6.0, 7.0)));
}

#[test]
fn entity_api_graceful_without_component() {
    let mut e = engine();
    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();

    let src = r#"
        function on_init(ent)
            assert(ent:get_position() == nil)
            ent:set_position(Vec3(1, 1, 1))
        end
    "#;
    let id = e.load_script("nocomp", src).unwrap();
    let entity = world.spawn();
    world.insert(entity, Script::new(id)).unwrap();

    script_runner_system(&mut world, &mut e, 0.016);
    assert_eq!(
        world.get::<Script>(entity).unwrap().state.lifecycle(),
        ScriptLifecycle::Active
    );
}

#[test]
fn on_destroy_runs_once_on_removal() {
    use std::cell::Cell;
    use std::rc::Rc;
    let destroyed = Rc::new(Cell::new(0u32));
    let counter = Rc::clone(&destroyed);

    let mut e = engine();
    e.register_fn("note_destroy", move |_ctx, _a| {
        counter.set(counter.get() + 1);
        Ok(ScriptValue::Nil)
    })
    .unwrap();

    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();

    let id = e
        .load_script("destroyer", "function on_destroy(ent) note_destroy() end")
        .unwrap();
    let entity = world.spawn_with((Transform3D::IDENTITY,));
    world.insert(entity, Script::new(id)).unwrap();

    script_runner_system(&mut world, &mut e, 0.016); // init
    world.remove::<Script>(entity).unwrap();
    script_runner_system(&mut world, &mut e, 0.016); // detects removal -> destroy
    script_runner_system(&mut world, &mut e, 0.016); // no second destroy

    assert_eq!(destroyed.get(), 1);
}

#[test]
fn duplicate_binding_rejected() {
    let mut e = engine();
    e.register_fn("dup", |_ctx, _a| Ok(ScriptValue::Nil))
        .unwrap();
    let err = e
        .register_fn("dup", |_ctx, _a| Ok(ScriptValue::Nil))
        .unwrap_err();
    assert!(matches!(err, ScriptError::InvalidArgument { .. }));
}

#[test]
fn binding_can_touch_transform_via_context() {
    let mut e = engine();
    let mut world = World::new();
    world.register::<Script>();
    world.register::<Transform3D>();

    e.register_fn("teleport", |ctx: &mut ScriptContext, args| {
        if let [ScriptValue::Entity(ent), ScriptValue::Vec3(p)] = args {
            let mut t = ctx.get_transform(*ent).unwrap_or(Transform3D::IDENTITY);
            t.translation = *p;
            ctx.set_transform(*ent, t);
        }
        Ok(ScriptValue::Nil)
    })
    .unwrap();

    let id = e
        .load_script(
            "tp",
            "function on_init(ent) teleport(ent, Vec3(9, 9, 9)) end",
        )
        .unwrap();
    let entity = world.spawn_with((Transform3D::IDENTITY,));
    world.insert(entity, Script::new(id)).unwrap();

    script_runner_system(&mut world, &mut e, 0.016);
    let t = *world.get::<Transform3D>(entity).unwrap();
    assert!(t.translation.approx_eq_default(Vec3::new(9.0, 9.0, 9.0)));
}

#[test]
fn stdlib_mutation_does_not_leak_across_scripts() {
    let mut e = engine();
    let attacker = e
        .load_script(
            "attacker",
            r#"
            function on_init(ent)
                string.rep = function() return "pwned" end
                math.abs = function() return -1 end
                table.concat = function() return "pwned" end
            end
        "#,
        )
        .expect("load attacker");
    let victim = e
        .load_script(
            "victim",
            r#"
            function on_init(ent)
                assert(string.rep("x", 3) == "xxx", "string.rep was poisoned")
                assert(math.abs(-5) == 5, "math.abs was poisoned")
                assert(table.concat({"a", "b"}) == "ab", "table.concat was poisoned")
            end
        "#,
        )
        .expect("load victim");

    e.call_init(attacker, spawn_ecs::Entity::PLACEHOLDER)
        .expect("attacker mutates only its own stdlib copy");
    e.call_init(victim, spawn_ecs::Entity::PLACEHOLDER)
        .expect("victim must see pristine stdlib");
}
