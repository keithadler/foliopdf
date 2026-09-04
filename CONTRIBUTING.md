# Contributing

Thanks for helping. foliopdf is small enough to hold in your head; please
keep it that way.

## Setup

```bash
git clone https://github.com/keithadler/foliopdf
cd foliopdf
cargo test --workspace          # core + CLI + integration tests
cargo bench -p foliopdf         # throughput numbers
./scripts/build-wasm.sh         # needs wasm-pack; produces ./pkg
node examples/node/smoke.mjs    # exercises the wasm package
```

Run your own PDFs through the parser and writer before opening a PR that
touches `parser.rs`, `document.rs` or `writer.rs`:

```bash
cargo run --release -p foliopdf --example corpus -- ~/Documents
```

It loads every PDF it finds, re-saves it compressed, reloads it, and
reports failures. Nothing is written back.

## Ground rules

- **No `unsafe`.** The crate is `#![forbid(unsafe_code)]`.
- **No I/O in the core crate.** Bytes in, bytes out. The CLI and WASM
  crates own files and the browser.
- **Lenient reader, strict writer.** Accept what real files do; write what
  the spec says. Every leniency should be a one-line comment citing what
  producer needed it.
- **Errors, never panics, on hostile input.** If you add a parser path,
  add a test that feeds it garbage.
- **Document public items.** `#![warn(missing_docs)]` is on. Explain what a
  thing is for, not what its signature already says.
- **Format and lint.** `cargo fmt --all` and
  `cargo clippy --workspace --all-targets -- -D warnings` must pass; CI
  enforces both plus the 1.75 minimum Rust version.

## Adding a batch step

1. Add a variant to `batch::Step` with `#[serde]` attributes matching the
   existing kebab-case `op` tags.
2. Handle it in `batch::process`.
3. Validate anything serde cannot in `Preset::validate`.
4. Add the TypeScript shape to the `TS_TYPES` block in
   `crates/foliopdf-wasm/src/lib.rs` and a line to `docs/presets.md`.
5. Add a round-trip test in `batch::tests`.

## Releases

Tag `vX.Y.Z` on `main`. The release workflow builds CLI binaries for
Linux, macOS and Windows, publishes the npm package (`foliopdf`) and the
crates (`foliopdf`, `foliopdf-cli`). Update `CHANGELOG.md` first.

## Licence

By contributing you agree your work is released under the MIT licence.
