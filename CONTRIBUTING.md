# Contributing to Spawn

Spawn is a production-grade game engine written in Rust. It is not a learning project, not a community-driven experiment, and not optimized for contribution volume. It is optimized for correctness.

We are selective. This document exists to save your time and ours.

---

## Who we're looking for

You have shipped production code in systems languages: C, C++, or Rust. Not tutorials. Not side projects that never ran in production. Code that had to work.

You have real experience in at least one of these domains:
- Game engine architecture (ECS, rendering, physics, audio)
- Low-level graphics (wgpu, Vulkan, Metal, DX12)
- Real-time networking and transport protocols
- Scripting VM integration
- Compiler or build tooling

You can read a technical specification, ask the right questions before writing a line of code, and defend every decision you made afterward.

---

## Who we're not looking for

If any of these describe you, this is not the right project right now:

- You are looking for a first Rust project
- You want to learn game engine architecture by contributing to one
- Your contribution process is: find something to change, write the code, open a PR
- You cannot explain why every line of your implementation exists

AI-generated code is not accepted. No exceptions.

---

## The process

Every contribution follows the same workflow, no exceptions:

1. **Open an issue first.** Describe what you want to change and why. No code yet.
2. **Wait for discussion.** We will tell you if it fits the roadmap and how to scope it.
3. **Write a spec.** A written description of exactly what changes, what does not, and why. Get approval before implementing.
4. **Implement.** One module at a time. One commit per module.
5. **Request review.** Every PR is reviewed against its spec. Deviations require a spec amendment, not a code workaround.

A PR without an approved spec will be closed.

---

## Standards

The same standards that apply to the core codebase apply to every contribution:

- `cargo clippy --all-features --all-targets -D warnings`: clean
- `cargo fmt --check`: clean
- `cargo test --workspace`: all pass
- `cargo deny check`: clean
- No `unwrap()` in production paths
- No `unsafe` without a `SAFETY` comment
- No dead code, no TODOs, no placeholder implementations
- `#![deny(warnings)]` at crate root

These are not guidelines. They are gates.

---

## Where to start

Read the specs in `docs/specs/` before touching any code. Every crate has a Phase 1 specification that defines its design decisions, its public API, and its acceptance criteria. If you do not understand a spec, ask in an issue before proceeding.

There are no "good first issues" in the traditional sense. The right first issue is one where your background gives you something real to contribute.

---

## Contact

If you have questions before opening an issue: altug@bytus.io
