use spawn_core::{Transform3D, Vec3};
use spawn_ecs::{
    serialize_entity, Component, EcsError, Entity, EntityMap, MapEntities, SerializeComponent,
    World,
};
use spawn_serialize::{Serialize, SerializeError, SerializeResult, Stream};

#[derive(Clone, Default, PartialEq, Debug)]
struct Aa(u32);
impl Component for Aa {}
impl Serialize for Aa {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        let mut v = u64::from(self.0);
        s.serialize_bits(&mut v, 32)?;
        self.0 = v as u32;
        Ok(())
    }
}
impl SerializeComponent for Aa {
    fn type_name() -> &'static str {
        "Aa"
    }
}

#[derive(Clone, Default, PartialEq, Debug)]
struct Bb(u32);
impl Component for Bb {}
impl Serialize for Bb {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        let mut v = u64::from(self.0);
        s.serialize_bits(&mut v, 32)?;
        self.0 = v as u32;
        Ok(())
    }
}
impl SerializeComponent for Bb {
    fn type_name() -> &'static str {
        "Bb"
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Link {
    target: Entity,
    value: u32,
}
impl Default for Link {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            value: 0,
        }
    }
}
impl Component for Link {}
impl Serialize for Link {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        serialize_entity(s, &mut self.target)?;
        let mut v = u64::from(self.value);
        s.serialize_bits(&mut v, 32)?;
        self.value = v as u32;
        Ok(())
    }
}
impl SerializeComponent for Link {
    fn type_name() -> &'static str {
        "Link"
    }
}
impl MapEntities for Link {
    fn map_entities(&mut self, map: &EntityMap) {
        if let Some(target) = map.get(self.target) {
            self.target = target;
        }
    }
}

struct Plain;
impl Component for Plain {}

fn t(x: f32) -> Transform3D {
    Transform3D::from_translation(Vec3::new(x, x * 2.0, x * 3.0))
}

#[test]
fn world_round_trips_values_and_component_sets() {
    let mut world = World::new();
    world.register_serializable::<Transform3D>();
    world.register_serializable::<Aa>();
    let e0 = world.spawn_with((t(1.0), Aa(11)));
    let _e1 = world.spawn_with((t(2.0),));
    let _e2 = world.spawn_with((Aa(33),));

    let bytes = world.serialize_world().unwrap();
    let map = world.deserialize_world(&bytes).unwrap();

    assert_eq!(map.len(), 3);
    assert_eq!(world.query::<&Transform3D>().iter().count(), 2);
    assert_eq!(world.query::<&Aa>().iter().count(), 2);

    let n0 = map.get(e0).unwrap();
    assert_eq!(world.get::<Transform3D>(n0).copied(), Some(t(1.0)));
    assert_eq!(world.get::<Aa>(n0), Some(&Aa(11)));
}

#[test]
fn transform_round_trips_exactly() {
    let mut world = World::new();
    world.register_serializable::<Transform3D>();
    let e = world.spawn_with((t(-3.5),));
    let bytes = world.serialize_world().unwrap();
    let map = world.deserialize_world(&bytes).unwrap();
    let n = map.get(e).unwrap();
    assert_eq!(world.get::<Transform3D>(n).copied(), Some(t(-3.5)));
}

#[test]
fn serialize_is_byte_deterministic() {
    let build = || {
        let mut world = World::new();
        world.register_serializable::<Transform3D>();
        world.register_serializable::<Aa>();
        world.spawn_with((t(1.0), Aa(5)));
        world.spawn_with((Aa(9),));
        world
    };
    let a = build();
    assert_eq!(a.serialize_world().unwrap(), a.serialize_world().unwrap());
    let b = build();
    assert_eq!(a.serialize_world().unwrap(), b.serialize_world().unwrap());
}

#[test]
fn entity_references_remap_through_a_chain() {
    let mut world = World::new();
    world.register_serializable::<Transform3D>();
    world.register_serializable_mapped::<Link>();
    let a = world.spawn_with((t(0.0),));
    let b = world.spawn_with((t(0.0),));
    world
        .insert(
            a,
            Link {
                target: b,
                value: 7,
            },
        )
        .unwrap();
    world
        .insert(
            b,
            Link {
                target: a,
                value: 8,
            },
        )
        .unwrap();

    let bytes = world.serialize_world().unwrap();
    let map = world.deserialize_world(&bytes).unwrap();
    let na = map.get(a).unwrap();
    let nb = map.get(b).unwrap();

    let la = world.get::<Link>(na).unwrap();
    assert_eq!(la.target, nb);
    assert_eq!(la.value, 7);
    let lb = world.get::<Link>(nb).unwrap();
    assert_eq!(lb.target, na);
    assert_eq!(lb.value, 8);
}

#[test]
fn non_serializable_components_are_skipped() {
    let mut world = World::new();
    world.register_serializable::<Aa>();
    world.register::<Plain>();
    let e = world.spawn_with((Aa(1), Plain));
    let bytes = world.serialize_world().unwrap();
    let map = world.deserialize_world(&bytes).unwrap();
    let n = map.get(e).unwrap();
    assert_eq!(world.get::<Aa>(n), Some(&Aa(1)));
    assert!(world.get::<Plain>(n).is_none());
}

#[test]
fn unknown_wire_id_is_rejected() {
    let mut source = World::new();
    source.register_serializable::<Aa>();
    source.register_serializable::<Bb>();
    source.spawn_with((Bb(1),));
    let bytes = source.serialize_world().unwrap();

    let mut target = World::new();
    target.register_serializable::<Aa>();
    let err = target.deserialize_world(&bytes).unwrap_err();
    assert!(matches!(err, EcsError::UnknownWireId { .. }));
}

#[test]
fn swapped_registration_order_decodes_to_the_wrong_component() {
    let mut source = World::new();
    source.register_serializable::<Aa>();
    source.register_serializable::<Bb>();
    let e = source.spawn_with((Aa(42),));
    let bytes = source.serialize_world().unwrap();

    let mut target = World::new();
    target.register_serializable::<Bb>();
    target.register_serializable::<Aa>();
    let map = target.deserialize_world(&bytes).unwrap();
    let n = map.get(e).unwrap();
    assert_eq!(target.get::<Bb>(n), Some(&Bb(42)));
    assert!(target.get::<Aa>(n).is_none());
}

#[test]
fn truncated_buffer_errors_without_panicking() {
    let mut world = World::new();
    world.register_serializable::<Transform3D>();
    world.spawn_with((t(1.0),));
    let bytes = world.serialize_world().unwrap();
    let truncated = &bytes[..bytes.len() / 2];
    let err = world.deserialize_world(truncated).unwrap_err();
    assert!(matches!(
        err,
        EcsError::Serialize(SerializeError::EndOfStream)
    ));
}
