# spawn-audio

spawn-audio is the engine's sound subsystem: a frame-pumped `AudioEngine` wrapping a kira-backed mixer, asset-driven loading of WAV and OGG Vorbis clips, lightweight playback handles for runtime control, and single-listener spatial positioning with distance attenuation and stereo panning. It exists to give game code and the editor a stable, kira-free public surface for triggering and steering sound, while keeping every platform-audio and decoding concern behind a backend boundary that can fall back to silence when no device is available.

## Design Decisions

**kira stays fully encapsulated.** kira is the mandated mixer and decoder, but none of its types (`AudioManager`, `StaticSoundData`, `Tween`, voice handles, `Decibels`, and so on) appear in any public signature, field, or re-export. They are confined to the `backend` module. Every public type is owned by spawn-audio. This keeps callers insulated from kira's API churn and leaves the engine free to swap or augment the playback layer in a later phase without a breaking change rippling through game code.

**Decoding routes through kira, not a direct symphonia dependency.** kira already drives symphonia internally, so consuming it directly would create a second decode path and a duplicate codec configuration to keep in sync. A single decode path through kira removes that drift. The symphonia feature is narrowed to the `wav` and `vorbis` codecs only: `mp3`, `flac`, and `aac` are deliberately excluded from the dependency tree.

**cpal is transitive, never named.** kira drives cpal under the hood; spawn-audio references no cpal type. cpal is documented as a backend dependency for traceability only. Device enumeration would promote it to a direct, separately-specified dependency, which is out of scope here.

**Device-init failure is recoverable, not fatal.** When the OS audio device cannot be opened, engine construction still succeeds and substitutes a `NullBackend`. The choice is between making every consumer handle a hard error at startup versus keeping the engine uniformly usable; the latter wins. The null path tracks voice and bus state exactly as a real device would, so handle lifecycle and bus volumes behave identically whether or not sound is audible. The fallback is surfaced through `backend_kind()` and logged at warn level, never silent.

**Linear amplitude is the public volume unit.** All public volume values are linear amplitude `f32` in `0.0..=1.0`, clamped rather than rejected. Decibels are an internal convenience confined to the `db_to_amplitude` / `amplitude_to_db` helpers, which live in spawn-audio rather than spawn-core because dB is an audio-domain concept. A single, clamped, linear unit at the boundary keeps caller-side math unambiguous.

**Control methods live on the engine, keyed by handle.** `SoundHandle` is a `Copy` generational index, a slot index plus generation, carrying no backend pointer. Stop, pause, resume, volume, pitch, and position operations are methods on `AudioEngine` that take a handle. This keeps the handle small and kira-free, centralizes all mutation behind the command queue, and lets a stale handle resolve to `InvalidHandle` instead of aliasing a recycled voice.

**No separate emitter type in this phase.** A spatial voice's emitter position rides on `PlaybackParams` at play time and is updated through the handle. Adding an `Emitter` struct would introduce another lifecycle to manage for no current benefit; the voice itself is the emitter.

**Bus routing is exactly one level deep.** Every named bus routes straight to the implicit master bus, and master routes to the device. No bus-to-bus chaining exists. Effective voice gain is `voice_volume * bus_volume * master_volume`, clamped at each stage. A flat routing graph covers the realistic Phase 1 mixing needs without the complexity of an arbitrary routing tree.

## Architecture

The crate is split by concern, with all kira contact isolated under `backend/`.

- **`engine`**: owns `AudioConfig`, `BusSpec`, `BackendKind`, and `AudioEngine` itself. `AudioEngine` holds the backend, the pre-sized command queue, and the voice table. Its surface is the frame pump (`update(dt)`), device lifecycle (`new`, `suspend`, `resume`, `backend_kind`), bus volume get/set, listener get/set, playback entry (`play`), and the per-handle control methods. `play` resolves a `Handle<AudioSource>` against an `AssetServer` passed by the caller, the engine holds no server reference, then enqueues a play command and returns a `SoundHandle` synchronously.
- **`source`**: `AudioSource` (decoded clip data held in memory, kira's static sound data kept private) plus `AudioLoader` and its `register` helper. `AudioSource` is `Send + Sync` and cheap to clone via internal ref-counting so it can live in the asset store and be shared across plays. It reports `duration`, `channels`, and `sample_rate`; channel count is sniffed from the RIFF `fmt ` chunk or Vorbis identification header at load, since kira's decoded frames are always stereo and lose the original count.
- **`handle`**: `SoundHandle` (the `Copy` generational token) and `VoiceState` (`Playing` / `Paused` / `Stopped`).
- **`bus`**: `BusId` (a `&'static str` newtype with the reserved `BusId::MASTER` constant) and the one-level routing helpers. Buses are created only at init from config; no runtime bus creation.
- **`spatial`**: `Listener` (one position and orientation per engine), `Attenuation`, and `AttenuationModel` (`Linear` / `Inverse`). The attenuation constructor upholds `min_distance <= max_distance`.
- **`params`**: `PlaybackParams`, `Spatial`, and the pure parameter-math functions: dB/amplitude conversion, attenuation gain, stereo pan from listener-relative azimuth, equal-power pan gains, and amplitude clamping. This module touches neither a device nor kira and is the primary deterministic test target.
- **`error`**: `AudioError` (a `#[non_exhaustive]` enum with `&'static str` context fields) and the `AudioResult<T>` alias. `AudioError` implements `Error`, `Display`, and `From<AudioError> for spawn_core::SpawnError`.
- **`backend/`**: private. `mod.rs` defines the backend trait, kind dispatch, and command queue; `kira.rs` holds every kira call; `null.rs` is the state-tracking no-op device. Nothing here is re-exported.

`lib.rs` carries `#![deny(warnings)]`, declares the modules, and re-exports the public types at the crate root (for example `spawn_audio::AudioEngine`). No kira symbol is re-exported anywhere.

The spatialization flow runs inside `update`: for each active spatial voice the engine computes gain from the attenuation model and the listener-to-emitter distance, computes stereo pan from the listener-relative direction projected onto the listener's right vector (derived from orientation), and applies both to the kira voice. Panning uses an equal-power law clamped to `[-1, 1]` left/right. Doppler is absent by design.

## Constraints

- **Allocation.** `AudioEngine::update` is allocation-free in steady state: the command queue and voice table are pre-sized and reused, so no per-call heap allocation occurs. `play` enqueues a command and returns synchronously; it neither blocks on nor runs on the audio thread. Error construction does not allocate: `context` is `&'static str`. A `play` that would exceed `max_voices` returns `AudioError::VoiceLimit` rather than growing the table.
- **Safety.** No `unsafe`. No `unwrap`, `expect`, or `panic!` outside test code. Fallible operations return `AudioResult<T>`. Backend indexing never panics.
- **Dependencies.** The crate depends only on `spawn-core` (math, `Vec3`/`Quat`, error types, `ApproxEq`) and `spawn-asset` (`AssetServer`, `AssetLoader`, `Handle<T>`, `AssetId`), plus `kira` with features limited to `cpal` and `symphonia` carrying only the `wav` and `vorbis` codecs. cpal is transitive and never named. No direct symphonia dependency.
- **kira containment.** No kira (or cpal) type appears in any public signature, public field, or re-export. All kira usage lives under `src/backend/`. spawn-audio owns every public type.
- **Units and conventions.** Public volume is linear amplitude `f32`, clamped to `0.0..=1.0`. Pitch is a playback-rate multiplier clamped to `0.0..=8.0`, with non-finite values rejected. dB appears only in the two `params` conversion helpers. Math follows spawn-core: right-handed, radians, `f32`, default listener orientation looking down `-Z`.
- **Rustdoc as contract.** Every public item carries `///` documentation; the rustdoc is the API contract.
- **State parity under Null.** Every engine operation under `BackendKind::Null` produces the same observable state as under a real device: voice lifecycle, handle validity, bus volumes.

## Phase 1 Scope

In scope: the once-per-frame `AudioEngine`; a kira-backed mixer with a master bus and named buses routed one level deep; WAV and OGG Vorbis loading through an `AudioLoader` registered with spawn-asset; playback handles supporting stop, pause, resume, volume, and pitch; a single listener with per-voice spatial emitter positioning, distance attenuation, and panning; the `NullBackend` silence fallback; the pure parameter math (attenuation, pan, dB conversion, clamping); and unit tests for all of it, including a steady-state no-allocation check on `update`, loader fixtures, and an ignored on-target device test.

Deferred, each gated behind its own future approval: doppler shift; reverb, filters, and other DSP effects; convolution and occlusion; HRTF/binaural; recording and capture; multiple simultaneous listeners; routing deeper than one level; runtime device hot-swap and enumeration UI; music sequencing and timeline; sample-accurate scheduling; and compressed or additional formats beyond OGG Vorbis (MP3, FLAC, AAC).

The line sits at a complete, testable, single-listener mixing and spatialization path with a hard kira boundary and a guaranteed silent fallback. Everything deferred either expands the routing graph, adds a DSP layer, or introduces device-management surface, each a distinct design problem better specified on its own rather than folded into the foundational playback path.
