use spawn_core::{Transform3D, Vec3};
use spawn_ecs::{propagate_transforms, Children, EcsError, GlobalTransform3D, Parent, World};

fn t(x: f32) -> Transform3D {
    Transform3D::from_translation(Vec3::new(x, 0.0, 0.0))
}

#[test]
fn set_parent_establishes_both_sides_and_reparents() {
    let mut world = World::new();
    let p1 = world.spawn();
    let p2 = world.spawn();
    let c = world.spawn();

    world.set_parent(c, p1).unwrap();
    assert_eq!(world.parent(c), Some(p1));
    assert_eq!(world.children(p1), &[c]);

    world.set_parent(c, p2).unwrap();
    assert_eq!(world.parent(c), Some(p2));
    assert_eq!(world.children(p2), &[c]);
    assert!(world.children(p1).is_empty());
}

#[test]
fn remove_parent_detaches_both_sides_and_no_ops_on_root() {
    let mut world = World::new();
    let p = world.spawn();
    let c = world.spawn();
    world.set_parent(c, p).unwrap();

    world.remove_parent(c).unwrap();
    assert_eq!(world.parent(c), None);
    assert!(world.children(p).is_empty());

    world.remove_parent(c).unwrap();
    assert_eq!(world.parent(c), None);
}

#[test]
fn cycles_are_rejected_and_leave_the_world_unchanged() {
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    let c = world.spawn();
    world.set_parent(b, a).unwrap();
    world.set_parent(c, b).unwrap();

    assert!(matches!(
        world.set_parent(a, a),
        Err(EcsError::HierarchyCycle { .. })
    ));
    assert!(matches!(
        world.set_parent(a, c),
        Err(EcsError::HierarchyCycle { .. })
    ));
    assert_eq!(world.parent(a), None);
    assert_eq!(world.children(c), &[] as &[spawn_ecs::Entity]);
}

#[test]
fn entity_not_found_for_dead_targets() {
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    world.despawn(b).unwrap();
    assert!(matches!(
        world.set_parent(a, b),
        Err(EcsError::EntityNotFound { .. })
    ));
}

#[test]
fn deferred_set_parent_applies_at_the_boundary() {
    let mut world = World::new();
    let p = world.spawn();
    let c = world.spawn();
    {
        let mut commands = world.commands();
        commands.set_parent(c, p);
    }
    assert_eq!(world.parent(c), None);
    world.apply_commands();
    assert_eq!(world.parent(c), Some(p));
    assert_eq!(world.children(p), &[c]);
}

#[test]
fn deferred_cycle_is_a_no_op_at_apply() {
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    world.set_parent(b, a).unwrap();
    {
        let mut commands = world.commands();
        commands.set_parent(a, b);
    }
    world.apply_commands();
    assert_eq!(world.parent(a), None);
}

#[test]
fn despawn_recursive_frees_the_subtree_and_nothing_else() {
    let mut world = World::new();
    let root = world.spawn();
    let a = world.spawn();
    let b = world.spawn();
    let leaf = world.spawn();
    let outside = world.spawn();
    world.set_parent(a, root).unwrap();
    world.set_parent(b, root).unwrap();
    world.set_parent(leaf, a).unwrap();

    let before = world.entity_count();
    world.despawn_recursive(root).unwrap();
    assert_eq!(world.entity_count(), before - 4);
    for e in [root, a, b, leaf] {
        assert!(!world.contains(e));
    }
    assert!(world.contains(outside));
}

#[test]
fn despawn_recursive_detaches_from_grandparent() {
    let mut world = World::new();
    let grand = world.spawn();
    let parent = world.spawn();
    let child = world.spawn();
    world.set_parent(parent, grand).unwrap();
    world.set_parent(child, parent).unwrap();

    world.despawn_recursive(parent).unwrap();
    assert!(world.contains(grand));
    assert!(world.children(grand).is_empty());
}

#[test]
fn plain_despawn_leaves_a_stale_child_pruned_by_propagation() {
    let mut world = World::new();
    let p = world.spawn_with((t(0.0),));
    let c = world.spawn_with((t(1.0),));
    world.set_parent(c, p).unwrap();

    world.despawn(c).unwrap();
    assert_eq!(world.children(p).len(), 1);

    propagate_transforms(&mut world);
    assert!(world.children(p).is_empty());
}

#[test]
fn propagation_composes_a_chain_and_roots_equal_local() {
    let mut world = World::new();
    let a = world.spawn_with((t(1.0),));
    let b = world.spawn_with((t(2.0),));
    let c = world.spawn_with((t(3.0),));
    world.set_parent(b, a).unwrap();
    world.set_parent(c, b).unwrap();

    propagate_transforms(&mut world);

    assert_eq!(
        world.get::<GlobalTransform3D>(a).map(|g| *g.get()),
        Some(t(1.0))
    );
    let expected_b = t(1.0).mul(t(2.0));
    assert_eq!(
        world.get::<GlobalTransform3D>(b).map(|g| *g.get()),
        Some(expected_b)
    );
    let expected_c = expected_b.mul(t(3.0));
    assert_eq!(
        world.get::<GlobalTransform3D>(c).map(|g| *g.get()),
        Some(expected_c)
    );
}

#[test]
fn propagation_skips_a_transformless_node_without_breaking_siblings() {
    let mut world = World::new();
    let root = world.spawn_with((t(1.0),));
    let no_transform = world.spawn();
    let sibling = world.spawn_with((t(5.0),));
    world.set_parent(no_transform, root).unwrap();
    world.set_parent(sibling, root).unwrap();
    let under = world.spawn_with((t(9.0),));
    world.set_parent(under, no_transform).unwrap();

    propagate_transforms(&mut world);
    assert!(world.get::<GlobalTransform3D>(no_transform).is_none());
    assert!(world.get::<GlobalTransform3D>(under).is_none());
    assert_eq!(
        world.get::<GlobalTransform3D>(sibling).map(|g| *g.get()),
        Some(t(1.0).mul(t(5.0)))
    );
}

#[test]
fn propagation_is_deterministic_and_idempotent() {
    let build = || {
        let mut world = World::new();
        let mut roots = Vec::new();
        for r in 0..4 {
            let root = world.spawn_with((t(r as f32),));
            roots.push(root);
            let mut prev = root;
            for d in 0..3 {
                let child = world.spawn_with((t(d as f32 + 0.5),));
                world.set_parent(child, prev).unwrap();
                prev = child;
            }
        }
        world
    };

    let mut world = build();
    propagate_transforms(&mut world);
    let snapshot: Vec<Transform3D> = world
        .query::<&GlobalTransform3D>()
        .iter()
        .map(|g| *g.get())
        .collect();

    propagate_transforms(&mut world);
    let again: Vec<Transform3D> = world
        .query::<&GlobalTransform3D>()
        .iter()
        .map(|g| *g.get())
        .collect();
    assert_eq!(snapshot, again);

    let mut other = build();
    propagate_transforms(&mut other);
    let other_snapshot: Vec<Transform3D> = other
        .query::<&GlobalTransform3D>()
        .iter()
        .map(|g| *g.get())
        .collect();
    assert_eq!(snapshot, other_snapshot);
}

#[test]
fn hierarchy_survives_serialization_round_trip() {
    let mut world = World::new();
    world.register_serializable::<Transform3D>();
    world.register_serializable_mapped::<Parent>();
    world.register_serializable_mapped::<Children>();

    let root = world.spawn_with((t(1.0),));
    let child = world.spawn_with((t(2.0),));
    world.set_parent(child, root).unwrap();

    let bytes = world.serialize_world().unwrap();
    let map = world.deserialize_world(&bytes).unwrap();
    let nroot = map.get(root).unwrap();
    let nchild = map.get(child).unwrap();

    assert_eq!(world.parent(nchild), Some(nroot));
    assert_eq!(world.children(nroot), &[nchild]);

    propagate_transforms(&mut world);
    let expected = t(1.0).mul(t(2.0));
    assert_eq!(
        world.get::<GlobalTransform3D>(nchild).map(|g| *g.get()),
        Some(expected)
    );
}
