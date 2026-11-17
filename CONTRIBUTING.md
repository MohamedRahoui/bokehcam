# Contributing

bokehcam is in early development, so the most useful contributions right now
are bug reports with the output of `bokehcam doctor` and `bokehcam status`.

Contributions are accepted under the project's licence, GPL-3.0-or-later.

## Setup

```sh
cargo build                       # first build takes a few minutes
scripts/load-module.sh            # virtual camera for this boot
cargo run -p bokehcam-cli -- models fetch
cargo run -p bokehcam-cli -- doctor
```

Build dependencies: a recent stable Rust toolchain and `clang` (for the V4L2
bindings).
The model runtime (ONNX Runtime) is downloaded prebuilt at build time.

## Before sending a change

```sh
cargo fmt --all
cargo clippy --workspace --all-targets     # must be warning-free
cargo test --workspace
```

The tests need no camera or graphics card. For anything that touches the
picture, also look at a real frame and at the timings:

```sh
cargo run --release -p bokehcam-cli -- snapshot /tmp/after.png
cargo run --release -p bokehcam-cli -- run --stats 5
```

## Conventions

- Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first; it says where
  things belong.
- Platform-specific code lives only in `bokehcam-media` (and the Unix socket
  in `bokehcam-engine::control`).
- A new setting needs a field in `config.rs` and an entry in `schema.rs`.
  Front ends pick it up from the schema; do not hard-code controls.
- A new model of a supported kind is a manifest with a pinned URL and
  SHA-256, not code.
- User-facing text says what happened and what to do, without internal names.
- Performance claims come with numbers from `--stats` or `models bench`, and
  processor use measured at the real frame rate.
- `unsafe` is confined to the kernel interface in
  `bokehcam-media/src/linux/loopback.rs`, each use with a `SAFETY` comment.
