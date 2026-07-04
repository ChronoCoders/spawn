# spawn-physics

`spawn-physics` wraps Rapier (`rapier2d` and `rapier3d`) behind a deterministic, fixed-step simulation API expressed entirely in spawn-core math types. It exists to give the engine a rigid-body substrate: bodies, colliders, joints, ray/shape queries, and collision events, whose behavior is bit-for-bit reproducible across platforms given identical inputs and tick ordering. That reproducibility is the load-bearing property: later phases (spawn-net rollback netcode in particular) replay the same simulation and require it to land on identical state. The crate also bridges into the ECS through component types and a set of transform-sync passes.

## Design Decisions

**Rapier as the solver, never as the interface.** Rapier and its `nalgebra` dependency are an implementation detail. No `nalgebra` or Rapier type appears in any public signature; a private `convert` module per dimension translates between spawn-core (`Vec2`/`Vec3`/`Quat`/`Transform2D`/`Transform3D`) and `nalgebra` (`Isometry`, `Vector`, `Point`, `UnitQuaternion`). This keeps callers insulated from the backend and leaves room to swap or upgrade Rapier without a public break. It also forces every conversion through one audited path rather than scattering ad-hoc casts across the crate.

**Determinism over throughput.** Rapier's `enhanced-determinism` feature is enabled on both dimensional backends. This pins the portable software-FMA path and a deterministic island ordering, at the cost of single-threaded solving. The trade is deliberate: lockstep and rollback netcode cannot tolerate platform-dependent floating-point divergence, and a fast-but-divergent simulation is useless to them. Parallel island solving and any determinism-preserving multithreading are a later concern with their own design pass.

**Fixed-step only.** Simulation advances in whole `fixed_timestep` ticks. `step()` runs exactly one tick; `step_accumulate()` drains an internal time accumulator in whole-tick units and retains the sub-tick remainder. Variable-dt stepping is intentionally absent because it destroys reproducibility. Render-side interpolation between ticks, the usual remedy for visual stutter, is out of scope here and belongs to spawn-render; this crate exposes the leftover accumulator so a caller can compute an interpolation alpha, but does no smoothing itself.

**Mirrored 2D and 3D modules rather than a generic abstraction.** `physics3d` and `physics2d` are two parallel modules with identical API shapes, differing only where dimensionality forces it (`Vec3` vs `Vec2`, `Quat` vs scalar angle, three-axis vs single-axis rotation locks, and so on). A single generic-over-dimension API was rejected: the angular-quantity asymmetry (a 3D vector versus a 2D scalar) and the rotation-lock asymmetry make a unified generic surface leak awkward type parameters into every caller. Duplication that mirrors exactly, test-for-test, is the cheaper cost.

**Generational handles, never raw indices or references.** Bodies, colliders, and joints are addressed by opaque newtype handles wrapping Rapier's generational indices. A handle to a removed entity never aliases a later one, and a stale handle resolves to `None` or `Err(InvalidHandle)` rather than panicking or returning garbage. This is what lets despawn/free be safe and what lets the ECS layer hand handles around without lifetime entanglement.

**No panics in the live path.** Every fallible operation returns `Option` or `PhysicsResult`. There is no `unwrap`/`expect`/`panic!` outside tests. Invalid configuration, stale handles, and degenerate shapes are all ordinary error returns. A physics tick must never bring down the process.

## Architecture

The crate root holds the dimension-independent types, reused by both modules: the three handle newtypes, `BodyType`, `CollisionGroups`, `QueryFilter`, and `CollisionEvent`. A small `math` module provides `BVec2`/`BVec3` boolean vectors (`#[repr(C)]`) for rotation/translation locks; these live here rather than in spawn-core for this phase. The `error` module defines `PhysicsError` (non-exhaustive) and the `PhysicsResult` alias, with `std::error::Error`/`Display` impls and a conversion into `spawn_core::SpawnError`.

`physics3d` and `physics2d` each break down the same way:

- **world**: `PhysicsWorld` and `PhysicsConfig`. The world owns all Rapier state (body/collider/joint sets, island manager, broad and narrow phases, the physics and query pipelines, integration parameters), the time accumulator, and the reusable event buffers. It is not `Clone`. This is the only stateful object; everything else is a value type. Its surface covers stepping (`step`, `step_accumulate`, `accumulator`), lifecycle (`add`/`remove` for bodies, colliders, and the two joint kinds), state access (`body_transform`/`set_body_transform`, `body_velocity`/`set_body_velocity`, force/impulse/torque application), and queries (`ray_cast`, `intersections_with_shape`).
- **body**: descriptor and value types for rigid bodies: `RigidBodyDesc` with its consuming builder methods, `Velocity`, `MassProperties` (density- or mass-driven), and `LockFlags`.
- **collider**: `Shape` (non-exhaustive: ball, cuboid, capsule, convex hull) and `ColliderDesc` with builders. Convex hulls are cooked at attach time; degenerate point sets are rejected.
- **joint**: `FixedJoint` and `RevoluteJoint` value types.
- **query**: `Ray` and `RayHit`.
- **convert**: private spawn-core ↔ `nalgebra` translation. Nothing from this module escapes the module boundary.

The **ecs** module sits on top. `RigidBody` and `Collider` are authoring components wrapping the respective descriptors, consumed when an entity is registered. `PhysicsBody` is the live link the registration pass writes back, carrying the body handle and an optional collider handle (collider-less bodies are valid). A set of public free functions: `register_physics_bodies`, `sync_transforms_to_physics`, `step`, `sync_physics_to_transforms`, operate on explicit `&mut World` / `&mut PhysicsWorld`, orchestrated by `run_physics_fixed_update`. A `PhysicsSyncState` struct tracks registration across frames. Both dimensions have full mirrors (`RigidBody2D`, `Collider2D`, `PhysicsBody2D`, `run_physics_fixed_update_2d`, `PhysicsSyncState2D`).

## Constraints

- **Allocation:** `step()` performs no heap allocation in steady state beyond what Rapier does internally. Per-tick collision events drain into a world-owned `Vec` that is cleared and reused each step; the returned slice borrows it and is valid only until the next mutation. `step_accumulate` appends events across all its internal ticks into a caller-owned `&mut Vec<CollisionEvent>`, which the caller is responsible for clearing. A reused caller buffer keeps the path allocation-free. No per-event allocation occurs. Error `context` strings are `&'static str`, so error construction never allocates.
- **Safety:** zero `unsafe`. No `unwrap`/`expect`/`panic!` anywhere outside `#[cfg(test)]`. Every failure mode is an `Option` or `PhysicsResult`.
- **Dependencies:** depends on `spawn-core` (math, transforms, error type) and `spawn-ecs` (component trait, world). Rapier backends are optional dependencies pulled in by feature: `rapier3d` via `dim3`, `rapier2d` via `dim2` (`nalgebra` arrives transitively). Both features are in `default`. Building with neither dimension is a `compile_error!` in `lib.rs`. Nothing else is a dependency, and no Rapier or `nalgebra` type may appear in a public signature.
- **Invariants:** spawn-core conventions hold throughout: right-handed, radians, `f32`; default gravity is downward 9.81 on the Y axis in both dimensions. Handles are generational and stale handles never alias live entities. Collision-pair handle ordering within a `CollisionEvent` is stable (lower generational index first) so events compare deterministically. Query and event ordering is unspecified in absolute terms but deterministic for a fixed world state. `enhanced-determinism` is enabled on both Rapier backends. The fixed-update passes run in exactly one order: register, sync-to-physics, step, sync-from-physics. And any other order is a scheduling bug. Physics carries no scale: transform reads always report `Vec3::ONE`/unit scale and writes ignore the scale component, preserving the ECS component's own scale on sync-back.

## Phase 1 Scope

In scope: deterministic fixed-step rigid-body simulation; the `PhysicsWorld` abstraction in both dimensions; body/collider/joint descriptors with opaque handles; ray-cast and single-shot shape-overlap queries; per-step collision-event drain covering both contact and sensor pairs; the ECS components and the four-pass fixed-update orchestration; and a full unit and 2D/3D-parity test suite. Runtime cooking of `ConvexHull` colliders is included.

Deferred, each gated behind its own future design pass:

- **`TriMesh` colliders**: out for this phase. The `Shape` enum is `#[non_exhaustive]`, so adding the variant later is non-breaking; it is deliberately absent now to avoid committing to mesh cooking and the associated query complexity before it is needed.
- **Joints beyond fixed and revolute**: prismatic, spherical, rope, and motor drives are later work. Fixed and revolute cover the immediate cases and exercise both the no-DOF and single-DOF constraint paths.
- **Character controllers**: not a rigid-body primitive; a separate concern.
- **Query-pipeline caching and persistent shape-cast sweeps**: only the single-shot overlap test is provided. Cached/swept queries carry their own performance and correctness design.
- **Tick-to-tick interpolation/extrapolation**: render smoothness lives in spawn-render, which consumes the exposed accumulator alpha.
- **Physics-state serialization/snapshotting for rollback transport**: owned by spawn-net, which builds on the deterministic substrate this phase establishes.
- **Multithreaded island-solving tuning**: incompatible with the single-threaded determinism guarantee chosen here, and revisited only alongside a determinism-preserving parallelism design.

The line falls where it does because the phase's purpose is the deterministic fixed-step foundation everything downstream depends on. Features that either compromise that determinism or belong to a consumer crate's responsibility are pushed out so the substrate lands correct and minimal first.
