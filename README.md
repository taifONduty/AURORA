# AURORA

**Auditable Unified Research Orchestration and Reasoning Architecture**

AURORA is an open-source research harness for long-form, evidence-grounded research.

The project is built around a simple idea: research should be treated as a process that can be inspected, resumed, tested, and improved, rather than as a single prompt followed by a generated answer.

AURORA will provide the runtime around language models and research tools. The goal is to make it easier to manage the parts of research that usually get hidden inside prompts or temporary model context: planning, source collection, evidence tracking, verification, iteration, and final synthesis.

The project is being written in **Rust**.

## Motivation

Language models are already capable of searching, reading, reasoning over sources, and producing useful research reports. The difficult part is making that process reliable over longer tasks.

A serious research workflow needs to keep track of more than the final answer. It should be possible to understand:

- what was investigated,
- which sources were used,
- what evidence supports a conclusion,
- where sources disagree,
- what remains unresolved,
- why the system decided to continue or stop,
- and how the final result was produced.

AURORA is intended to provide the infrastructure for that process.

## Goals

AURORA aims to support:

- long-running research tasks,
- multiple model providers,
- different search and retrieval tools,
- structured research state,
- source and evidence tracking,
- claim verification,
- iterative research and replanning,
- reproducible research runs,
- cost and resource tracking,
- and clear inspection of the work that led to a result.

The project will remain model-agnostic wherever possible. Models and external tools should be replaceable without changing the basic research workflow.

## Why Rust?

A research harness is a long-running execution system as much as it is an application around language models.

AURORA is written in Rust to keep the runtime predictable and reliable while handling concurrent work, external tools, persistent state, failures, and long-lived research sessions.

Rust also makes it possible to keep the core system lightweight and distribute it as a native binary without requiring a separate runtime.

## Current Status

AURORA is in Phase 1C. The repository contains a deterministic core runtime slice with a scripted model, one read-only fixture tool, durably acknowledged JSONL events, incremental live projection, pure state reconstruction, and executable acceptance tests. It is not yet a research agent.

Real model providers, network tools, research workflows, and a user-facing command line remain deliberately deferred. The API and version-1 storage format should still be treated as pre-release contracts.

## Development

AURORA uses the Rust 2024 edition and requires a compatible stable toolchain.

```bash
git clone https://github.com/<owner>/aurora.git
cd aurora

cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

See [`docs/runtime-contract.md`](docs/runtime-contract.md) for the implemented behavioral contract and [`docs/runtime-acceptance.md`](docs/runtime-acceptance.md) for its acceptance scenarios.

## Roadmap

The immediate work is focused on the foundations:

- define the research lifecycle,
- build the core runtime,
- establish model and tool interfaces,
- persist research state,
- track sources and evidence,
- support iterative research,
- and build evaluation into the project from the beginning.

More detailed design documents will be added as those parts stabilize.

## Project Status

> Experimental. APIs and behavior will change frequently during early development.

AURORA is currently research software and is not intended for production use.

## License

License information will be added before the first public release.

---

**AURORA**<br>
*Auditable Unified Research Orchestration and Reasoning Architecture*
