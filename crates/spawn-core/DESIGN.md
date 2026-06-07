# spawn-core

spawn-core is the foundation crate of the Spawn engine: it defines the math types, geometric primitives, error types, and a small set of shared utility traits that every other workspace crate builds on. It depends on nothing but `std` and is depended on by everything, so its types are deliberately small, `Copy`, and laid out for direct transfer to the GPU and across FFI boundaries. Keeping this layer narrow and dependency-free prevents the rest of the engine from coupling to a third-party math library and gives the whole codebase one canonical set of conventions for coordinates, precision, and float comparison.

## Design Decisions

A hand-written math library was chosen over an off-the-shelf crate such as `glam`. The motivation is control: the engine needs a fixed memory layout it can hand directly to wgpu, a single sanctioned float-comparison path, and freedom to evolve the API on the engine's terms rather than a dependency's release cadence. The cost — reimplementing well-understood linear algebra — is bounded and one-time, and it removes a transitive dependency from the root of the dependency graph.

Precision is `f32` only. GPU pipelines operate in single precision, so storing or computing geometry in `f64` would force conversions at every upload and waste bandwidth. No `f64` variants exist, and adding them is a separate decision for a later phase rather than a default that doubles the type surface.

Every math and primitive type is `#[repr(C)]` and `Copy`. `#[repr(C)]` fixes field order and layout so a `Vec3` or `Mat4` can be memcpy'd into a buffer or passed over FFI without a conversion shim; `Copy` keeps the types value-like and avoids accidental moves in hot inner loops. All types are stack-allocated — there is no heap allocation anywhere in the crate, which keeps construction trivial and predictable.

Float equality is split deliberately. Derived `PartialEq` is bitwise-exact and exists mainly so types can sit in derived structures and exact-constant tests; meaningful approximate comparison goes exclusively through the `ApproxEq` trait with an explicit epsilon (default `1e-6`). This keeps the dangerous `==`-on-floats pattern out of downstream code by giving it a named, intentional alternative.

Fallibility is encoded in return types, never in panics. Operations that can fail — normalizing a near-zero vector, inverting a singular matrix, building a projection from degenerate inputs — return `Option` or `SpawnResult` instead of panicking or returning silent garbage. There is no `unwrap`, `expect`, or `panic!` in non-test code. The one concession is index-by-`usize` access on vectors, which carries slice-panic semantics and is documented as such; hot paths address fields directly instead.

Coordinate conventions are fixed here and are normative for the entire engine: right-handed, column-major matrices, column vectors (`M * v`), angles in radians. Establishing these once at the bottom of the stack avoids the per-crate ambiguity that otherwise produces subtle transform bugs.

`Rect` and `AABB2` share an identical field layout but are kept as distinct types on purpose. `Rect` is UI/screen space with min-inclusive, max-exclusive containment; `AABB2` is collision space with fully inclusive bounds. Collapsing them into one type would force every caller to remember which containment rule applies; separate types with explicit `From` conversions make the intent visible at the call site.

Quaternions are excluded from the `Lerp` trait. Componentwise linear interpolation of quaternions silently denormalizes and takes wrong paths, so quaternion blending is only reachable through the explicit `slerp`/`nlerp` methods — removing a correctness trap by construction.

## Architecture

The crate is organized into four areas, each a module under `src/`, with `lib.rs` re-exporting every public type at the crate root (`spawn_core::Vec3`) while leaving the module paths public for explicit addressing.

`math` holds the linear algebra. `Vec2`, `Vec3`, and `Vec4` provide the usual arithmetic operator set, associated unit/axis constants, and methods for dot products, length, normalization (`normalize` returning `Option`, plus a `normalize_or_zero` convenience), interpolation, componentwise min/max/clamp, and dimension changes via `extend`/`truncate`. `Vec3` adds `cross`, projection, and reflection. `Mat3` and `Mat4` are column-major matrix types carrying construction helpers (from columns, rows, diagonal, rotation, scale, translation, and quaternion), transpose, determinant, `Option`-returning `inverse`, and point/vector transform methods; `Mat4` additionally supplies the camera and projection builders (`look_at_rh`, `perspective_rh`, `orthographic_rh`) targeting wgpu's `[0,1]` depth range. `Quat` is the rotation type with Hamilton-product multiplication, axis-angle and Euler construction, conjugate/inverse, and shortest-path `slerp`/`nlerp`. The module root also exposes scalar helpers — `lerp`, `inverse_lerp`, `remap`, `wrap_angle` — and the shared `EPSILON` constant.

`primitives` builds higher-level geometry on the math types. `Color` is a linear-space, unclamped (HDR-capable) RGBA value with sRGB conversion in both directions. `Rect`, `AABB2`, and `AABB3` are the bounding-region types described above, covering containment, intersection, union, expansion, and — for `AABB3` — surface area and volume needed by the physics BVH. `Transform2D` and `Transform3D` are TRS (translation/rotation/scale) transforms with matrix conversion, point/vector application, `Option`-returning inversion, and parent-child composition exposed both as a method and via `Mul`.

`error` defines `SpawnError`, a `#[non_exhaustive]` enum of coarse failure categories carrying `&'static str` context, plus the `SpawnResult<T>` alias used throughout the workspace. It implements `Error`, `Display`, and `From<std::io::Error>`.

`traits` holds the two cross-cutting traits: `ApproxEq` (the sanctioned approximate-comparison path, implemented for every numeric type in the crate) and `Lerp` (linear interpolation for scalars, vectors, and color).

## Constraints

- No heap allocation anywhere in the crate. Every type is stack-allocated and `Copy`. Error construction in particular must not allocate, which is why `SpawnError` context is `&'static str` rather than `String`.
- No `unsafe`. The crate is entirely safe Rust with no exceptions.
- No `unwrap`, `expect`, or `panic!` in non-test code. Fallible operations return `Option` or `SpawnResult`. The sole documented exception is slice-style index access on vector types.
- Dependencies: `std` only. No external crates, including no serde and no SIMD intrinsic crates. This crate sits at the root of the dependency graph and must not pull anything else into it.
- Precision is `f32` exclusively; `f64` appears nowhere.
- Every math and primitive type is `#[repr(C)]` with a fixed, asserted size and alignment so it transfers to GPU buffers and across FFI without conversion. `Vec4` is intentionally not 16-byte aligned — std140 padding belongs to spawn-render, not here.
- Approximate float comparison goes through `ApproxEq` only; derived `PartialEq` is reserved for bitwise-exact uses and exact-constant tests.
- `SpawnError` is `#[non_exhaustive]` and is neither `Clone` nor `PartialEq` (it wraps `io::Error`).
- `Rect` containment is min-inclusive/max-exclusive; `AABB2`/`AABB3` containment and overlap are inclusive on all bounds. These conventions are part of the type contract.

## Phase 1 Scope

In scope: the math types, the geometric primitive types, the error types, the `ApproxEq` and `Lerp` utility traits, and exhaustive unit tests. Test coverage is part of the deliverable, not an afterthought — every required method has at least one test, all float assertions route through `ApproxEq` rather than `==`, layout is locked down with compile-time size/offset assertions on every `#[repr(C)]` type, and the harder paths (matrix inverse round-trips, projection depth mapping, quaternion shortest-path behavior, near-zero normalization, sRGB round-trips) carry dedicated checks.

Explicitly deferred to later, separately approved phases: SIMD intrinsics, `f64` type variants, serde serialization, ECS types, allocator utilities, and logging. Each of these expands either the dependency surface or the type surface in ways that would compromise the "small, dependency-free root" property this crate is built to preserve, so none of them enter on default. The line is drawn at the minimal set of value types the rest of the workspace cannot be written without — anything that can be layered above this crate stays above it.
