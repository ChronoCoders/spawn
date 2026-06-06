//! End-to-end tests covering spec §12 mandatory coverage.

use spawn_ecs::{Component, EcsError, Entity, Query, Schedule, Stage, World};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Position {
    x: i64,
    y: i64,
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct Velocity {
    dx: i64,
    dy: i64,
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct Health(i32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tag;

impl Component for Position {}
impl Component for Velocity {}
impl Component for Health {}
impl Component for Tag {}

// ---- layout ----

const _: () = assert!(std::mem::size_of::<Entity>() == 8);

#[test]
fn placeholder_never_returned_by_spawn() {
    let mut world = World::new();
    for _ in 0..100 {
        assert!(!world.spawn().is_placeholder());
    }
}

// ---- generation recycling ----

#[test]
fn recycle_reuses_index_and_rejects_stale() {
    let mut world = World::new();
    let a = world.spawn();
    world.insert(a, Health(5)).unwrap();
    assert_eq!(a.index(), 0);

    world.despawn(a).unwrap();
    assert!(!world.contains(a));
    assert!(world.get::<Health>(a).is_none());
    assert!(matches!(
        world.despawn(a),
        Err(EcsError::EntityNotFound { .. })
    ));

    let b = world.spawn();
    assert_eq!(b.index(), a.index());
    assert_ne!(b.generation(), a.generation());
    assert!(world.contains(b));
    // Stale handle never aliases the recycled entity.
    assert!(world.get::<Health>(a).is_none());
}

#[test]
fn many_recycle_cycles_bump_generation() {
    let mut world = World::new();
    let mut last_gen = None;
    for _ in 0..1000 {
        let e = world.spawn();
        assert_eq!(e.index(), 0);
        if let Some(g) = last_gen {
            assert!(e.generation() > g || e.generation() == 0);
        }
        last_gen = Some(e.generation());
        world.despawn(e).unwrap();
    }
}

// ---- archetype moves ----

#[test]
fn insert_remove_preserve_other_components() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 1, y: 2 }).unwrap();
    world.insert(e, Velocity { dx: 3, dy: 4 }).unwrap();
    world.insert(e, Health(99)).unwrap();

    assert_eq!(*world.get::<Position>(e).unwrap(), Position { x: 1, y: 2 });

    let removed = world.remove::<Velocity>(e).unwrap();
    assert_eq!(removed, Some(Velocity { dx: 3, dy: 4 }));
    assert!(world.get::<Velocity>(e).is_none());
    // Other components survive the move.
    assert_eq!(*world.get::<Position>(e).unwrap(), Position { x: 1, y: 2 });
    assert_eq!(*world.get::<Health>(e).unwrap(), Health(99));

    // Removing an absent component is Ok(None), not an error.
    assert_eq!(world.remove::<Velocity>(e).unwrap(), None);
}

#[test]
fn swap_remove_keeps_moved_entity_intact() {
    let mut world = World::new();
    let a = world.spawn_with(Position { x: 1, y: 1 });
    let b = world.spawn_with(Position { x: 2, y: 2 });
    let c = world.spawn_with(Position { x: 3, y: 3 });

    // Despawn the middle entity; the last entity swaps into its row.
    world.despawn(b).unwrap();
    assert_eq!(*world.get::<Position>(a).unwrap(), Position { x: 1, y: 1 });
    assert_eq!(*world.get::<Position>(c).unwrap(), Position { x: 3, y: 3 });
    assert!(world.get::<Position>(b).is_none());
    assert_eq!(world.entity_count(), 2);
}

#[test]
fn insert_overwrites_existing() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Health(1)).unwrap();
    world.insert(e, Health(2)).unwrap();
    assert_eq!(*world.get::<Health>(e).unwrap(), Health(2));
}

// ---- query correctness ----

#[test]
fn query_with_without_filters() {
    let mut world = World::new();
    world.register::<Tag>();
    let tagged = world.spawn_with((Position { x: 0, y: 0 }, Tag));
    let plain = world.spawn_with(Position { x: 9, y: 9 });

    let with: Vec<Entity> = world
        .query::<&Position>()
        .with::<Tag>()
        .iter_entities()
        .collect();
    assert_eq!(with, vec![tagged]);

    let without: Vec<Entity> = world
        .query::<&Position>()
        .without::<Tag>()
        .iter_entities()
        .collect();
    assert_eq!(without, vec![plain]);
}

#[test]
fn tuple_query_and_mut_iteration() {
    let mut world = World::new();
    for i in 0..5 {
        world.spawn_with((Position { x: i, y: 0 }, Velocity { dx: 1, dy: 1 }));
    }
    let mut q = world.query_mut::<(&mut Position, &Velocity)>();
    let mut count = 0;
    for (pos, vel) in q.iter_mut() {
        pos.x += vel.dx;
        count += 1;
    }
    assert_eq!(count, 5);
    let sum: i64 = world.query::<&Position>().iter().map(|p| p.x).sum();
    assert_eq!(sum, (1 + 2 + 3 + 4 + 5));
}

#[test]
fn query_get_random_access_and_count() {
    let mut world = World::new();
    let e = world.spawn_with(Position { x: 7, y: 8 });
    let q = world.query::<&Position>();
    assert_eq!(q.count(), 1);
    assert!(!q.is_empty());
    assert_eq!(q.get(e).unwrap(), &Position { x: 7, y: 8 });
    assert!(q.get(Entity::PLACEHOLDER).is_none());
}

#[test]
fn empty_and_zero_archetype_queries() {
    let world = World::new();
    let q = world.query::<&Position>();
    assert!(q.is_empty());
    assert_eq!(q.count(), 0);
    assert_eq!(q.iter().count(), 0);
}

#[test]
fn unregistered_filter_matches_nothing_or_everything() {
    let mut world = World::new();
    world.spawn_with(Position { x: 0, y: 0 });
    // `With` on an unregistered component matches nothing.
    assert_eq!(world.query::<&Position>().with::<Tag>().count(), 0);
    // `Without` on an unregistered component matches everything.
    assert_eq!(world.query::<&Position>().without::<Tag>().count(), 1);
}

#[test]
fn deterministic_iteration_order() {
    let mut world = World::new();
    let mut expected = Vec::new();
    for i in 0..10 {
        expected.push(world.spawn_with(Position { x: i, y: 0 }));
    }
    let order1: Vec<Entity> = world.query::<&Position>().iter_entities().collect();
    let order2: Vec<Entity> = world.query::<&Position>().iter_entities().collect();
    assert_eq!(order1, order2);
    assert_eq!(order1, expected);
}

#[test]
fn exact_size_iterator() {
    let mut world = World::new();
    for i in 0..4 {
        world.spawn_with(Position { x: i, y: 0 });
    }
    let q = world.query::<&Position>();
    let iter = q.iter();
    assert_eq!(iter.len(), 4);
}

// ---- error paths ----

#[test]
fn stale_entity_structural_ops_error() {
    let mut world = World::new();
    let e = world.spawn();
    world.despawn(e).unwrap();
    assert!(matches!(
        world.insert(e, Health(1)),
        Err(EcsError::EntityNotFound { .. })
    ));
    assert!(matches!(
        world.remove::<Health>(e),
        Err(EcsError::EntityNotFound { .. })
    ));
}

#[test]
fn unregistered_component_in_build_errors() {
    let world = World::new();
    let mut stage = Stage::new("update");
    stage.add_system(|_q: Query<'_, &Position, ()>, _c: &mut spawn_ecs::Commands<'_>| Ok(()));
    // Position is never registered in this world.
    assert!(matches!(
        stage.build(&world),
        Err(EcsError::ComponentNotRegistered { .. })
    ));
}

#[test]
fn run_before_build_errors() {
    let mut world = World::new();
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(|_c: &mut spawn_ecs::Commands<'_>| Ok(()));
    schedule.add_stage(stage);
    assert!(matches!(
        schedule.run(&mut world),
        Err(EcsError::ScheduleNotBuilt)
    ));
}

#[test]
fn panicking_system_surfaces_as_error() {
    let mut world = World::new();
    world.register::<Position>();
    let mut schedule = Schedule::new();
    let mut stage = Stage::new("s");
    stage.add_system(
        |_q: Query<'_, &Position, ()>, _c: &mut spawn_ecs::Commands<'_>| panic!("boom"),
    );
    schedule.add_stage(stage);
    schedule.build(&world).unwrap();
    assert!(matches!(
        schedule.run(&mut world),
        Err(EcsError::SystemPanicked { .. })
    ));
}
