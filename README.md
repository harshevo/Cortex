# Cortex

Cortex is a Rust coding harness with a native desktop UI and a command-line agent.
It supports provider-backed chats, project files, tool approvals, code search,
verification, skills, session history, split chats, and collaboration between chats.

## Run

```sh
cargo run -p cortex-gui
cargo run -p cortex-cli -- chat --dir /path/to/project
```

Choose a provider and model in the desktop Settings, or use the CLI provider flags.
Provider credentials and desktop settings are kept under `~/.cortex`; project chat
logs are kept under `.cortex/trajectories`. No neural learner or brain service is started.

```sh
cargo run -p cortex-cli -- --help
cargo run -p cortex-cli -- run --help
```

## Workspace

| Crate | Purpose |
| --- | --- |
| cortex-core | Agent loop, tools, approvals, context, logs, and verification interfaces |
| cortex-providers | Provider adapters and authentication |
| cortex-index | Code index and dependency graph |
| cortex-retrieval | Optional semantic code search |
| cortex-router | Project maps and task context routing |
| cortex-tools | File, shell, search, browser, and GitHub tools |
| cortex-skills | Written procedures for the agent |
| cortex-verify | Project check discovery and execution |
| cortex-eval | Harness evaluation runner and reports |
| cortex-cli | Command-line interface and shared harness setup |
| cortex-gui | Native desktop UI |

## Optional semantic search

Code search by meaning uses a separate embedding service. Ordinary chats and
file/search tools work without it.

```sh
uv sync --project ml
uv run --project ml python -m cortex_ml.embedding.server
cargo run -p cortex-cli -- search --help
```

The service uses `google/embeddinggemma-2`; model access may require Hugging Face
authentication. See `ml/README.md`.

## Verification

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p cortex-cli -p cortex-gui
```

`scripts/package.sh` packages macOS/Linux builds; `scripts/package.ps1` packages
Windows builds. Packages include the optional embedding runtime. Run packaging
on the destination operating system and architecture.
