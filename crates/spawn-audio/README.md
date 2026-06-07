# spawn-audio

spawn-audio is the engine's sound subsystem: a frame-pumped `AudioEngine` wrapping a kira-backed mixer, asset-driven loading of WAV and OGG Vorbis clips, lightweight playback handles for runtime control, and single-listener spatial positioning with distance attenuation and stereo panning.

See [DESIGN.md](DESIGN.md) for architecture, design decisions, constraints, and Phase 1 scope.
