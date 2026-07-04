# spawn-serialize

`spawn-serialize` is the bit-level serialization codec that sits between structured game state and the opaque byte payloads the transport moves. The networking layer needs encodings that are compact (bandwidth is the scarce resource), bit-exact (both peers must agree to the bit), and deterministic across platforms (a snapshot encoded on one machine must decode identically on another). The transport itself is deliberately byte-oriented and carries no opinion about message structure, so the codec is a separate concern with its own crate. The result is a small, dependency-light library of stream primitives: a bit reader and writer, a direction-symmetric serialization trait, and the integer, float-quantization, and geometry helpers that turn world values into bits, usable by the replication layer and by any later consumer (replay capture, asset packing) that needs the same compaction.

## Design Decisions

**One serialization function per type, driving both directions.** A `Stream` trait abstracts over a writer and a reader; a value implements a single `serialize` method that reads its fields on a writer and writes them on a reader, distinguished only by a direction query for the cases where encode and decode are not structurally identical. Maintaining separate read and write functions per type was rejected outright: the two drift apart under maintenance, and a single function makes the wire layout identical by construction. This mirrors the field-table discipline that hand-rolled C network code uses, expressed as a Rust trait rather than a macro.

**Caller-owned buffers, zero allocation on the codec path.** The writer and reader operate over a byte slice the caller supplies and reuses; encoding and decoding allocate nothing. The codec never owns or grows a buffer, so a steady-state encode or decode loop has no heap traffic: the allocation discipline the hot networking path requires is satisfied at the foundation.

**Bit packing, MSB-first, host-endianness-independent.** Values pack at bit granularity rather than byte granularity, because the bandwidth wins come from sub-byte fields (a changed-marker bit, a 2-bit index, a 9-bit quantized component). Bits pack most-significant-first within each byte with bytes ascending, a fixed order that does not depend on host endianness: the same guarantee the transport's header makes, extended to the bit level.

**Quantization and geometry as primitives, not a schema.** The crate ships bounded-integer encoding, zig-zag for signed values, bounded float quantization, and unit-quaternion smallest-three compression as composable helpers over the bit stream. It defines no schema language, no versioning, and no self-describing format: streams carry no type tags, and reader and writer agree by sharing the same serialize function. `serde` was rejected: it is byte-oriented rather than bit-oriented, pulls a dependency the transport-adjacent layer is meant to avoid, and does not produce the quantized, delta-friendly bit layout that drives bandwidth down.

**No derive macro in this phase.** A `#[derive(Serialize)]` would require a proc-macro crate, a new dependency and a new workspace member. The handful of serialization implementations the netcode needs are written by hand, and the trait is shaped so a derive can target it later without changing any call site.

**A standalone crate, not a module of `spawn-core` or `spawn-net`.** The codec is kept out of `spawn-net` because the transport's contract is explicitly serialization-free, and out of `spawn-core` because a bit codec with quantization is a networking-shaped concern rather than a math primitive. Standing alone keeps it reusable by future consumers and keeps the transport's audit surface minimal.

## Architecture

The crate is a flat set of modules with the public types re-exported at the crate root; modules stay public for explicit paths.

- **`bits`**: `BitWriter` and `BitReader` over a caller-owned `&mut [u8]` / `&[u8]`. The writer offers width-bounded integer writes (`1..=64` bits), single bits, and byte-aligned block copies; it clears each slot before writing so a dirtied buffer cannot leak, and a finishing step zero-fills the trailing partial byte. The reader mirrors them. Every operation is bounds-checked and returns `EndOfStream` rather than panicking, and an invalid width is rejected as `InvalidWidth`.
- **`stream`**: the `Stream` trait (`is_writing`, `serialize_bits`, `serialize_bool`), implemented by both `BitWriter` and `BitReader`, and the `Serialize` trait that types implement with one `serialize<S: Stream>` function. `Stream` is object-safe, which lets a consumer drive a type's `serialize` against a type-erased stream where that is convenient.
- **`pack`**, free functions over `Stream`: unsigned integers in an explicit width, signed integers via zig-zag, bounded integers in the minimum bits for their range, and bounded `f32` quantization that clamps on write and round-trips to within one step. Each is a single symmetric function that encodes on a writer and decodes on a reader.
- **`geom`**, geometry helpers over `spawn-core` types: per-axis position quantization with `PositionBounds`, and unit-quaternion smallest-three compression (the largest-magnitude component dropped and reconstructed on read, with a sign fold and a renormalize so a denormalized peer value can never yield a non-finite quaternion).
- **`error`**: `SerializeError` (`#[non_exhaustive]`: `EndOfStream`, `InvalidWidth`, `OutOfRange`), the `SerializeResult<T>` alias, and the conversion into `spawn_core::SpawnError`.

## Constraints

- **Allocation.** Encode and decode operate over caller-owned buffers and allocate nothing. There are no internal `Vec`, `Box`, or `String` on any code path.
- **Safety.** 100% safe Rust, zero `unsafe`. Bit manipulation is plain shifts and masks on `u8`/`u64`; there is no transmute, no raw-pointer access, and no `#[repr]` reliance. Any future `unsafe` would carry a mandatory `SAFETY` comment, but none is required.
- **Panics.** No `unwrap`, `expect`, or `panic!` outside test code. Every fallible operation returns `SerializeResult`. Writing past the buffer, reading past the end, an out-of-range quantization input, and an out-of-range bit width are errors, never panics: a malformed peer payload must never crash the decoder.
- **Determinism.** Quantization is bit-exact and platform-independent: the same value and bounds produce the same bits on every target, because the arithmetic is explicit integer work over a single IEEE-754 round-to-nearest multiply. Both peers depend on this agreement.
- **Dependencies.** `spawn-core` and `std` only. No third-party crates, no `serde`, no async runtime: verifiable through `cargo tree -p spawn-serialize`.
- **Documentation.** Every public item carries a `///` doc comment stating its contract; `#![deny(warnings)]` is in force at the crate root.

## Phase 2 Scope

In scope: the `BitWriter`/`BitReader` bit stream; the `Stream`/`Serialize` direction-symmetric traits; integer, zig-zag, bounded-integer, and quantized-float helpers; bounded position and smallest-three quaternion compression; the `SerializeError` type and its `spawn-core` interop; and unit tests covering bit round-trips at every width, quantization round-trips and determinism, the geometry helpers, and the never-panic bounds behavior.

Deferred, each to a later approved phase: component and world serialization: mapping ECS components onto a stream is the replication layer's bridge, built on these primitives, and this crate carries no dependency on `spawn-ecs`; acked-baseline delta encoding, which is replication logic that uses the codec but belongs with the netcode; a schema language, versioning, or self-describing format; a derive macro for the `Serialize` trait; compression beyond quantization (entropy or range coding, the relative small-or-large value classes used for sub-30-bit deltas); and any threaded or asynchronous use. The line falls exactly at the boundary between turning values into bits and deciding which values to send: this crate does the former and has no opinion about the latter.
