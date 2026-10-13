# Plan: build `vgc-engine-data` without a local dex dump

Status: **plan only, nothing implemented.** Written 2026-10-12.

## Problem

`crates/vgc-engine-data/build.rs` reads six `@pkmn/dex` JSON files from `$VGC_DEX_DIR`,
falling back to `$HOME/Dev/localdex/data`: `moves`, `abilities`, `items`, `pokedex`,
`typechart` and `learnsets.json`. None of them is in the repository. So:

- A fresh clone does not build unless the user finds and points at that dump.
- `cargo publish` cannot work: it verifies by building the packaged crate, and neither that
  build nor docs.rs (no network) nor any user's `cargo add` has the dump.
- Downstream builds (Python wheels, the calc-toy wasm build) inherit the same dependency.

CI works around this today (`.github/workflows/ci.yml`): it downloads the
[`localdex`](https://www.npmjs.com/package/localdex) 0.1.1 npm tarball, whose `data/`
directory is the same dump, and checks the six files against pinned SHA-256s. The files were
byte-identical to the local dump on 2026-10-12. That fixes CI only, not crates.io or users.

## Facts the options depend on

| Item | Size |
|---|---|
| The six JSON files as dumped (`localdex` 0.1.1, `@pkmn/dex` 0.10.7) | 4.6 MB (0.54 MB gzipped) |
| The same, reduced to the fields `build.rs` deserializes, with learnset method codes dropped | 1.9 MB (0.22 MB gzipped) |
| Generated `data_tables.rs` (`OUT_DIR`) | 2.4 MB (0.25 MB gzipped) |

- **Licence.** `@pkmn/dex` is MIT (Copyright (c) 2020-2026 pkmn contributors) and repackages
  Pokémon Showdown data (MIT, Copyright (c) 2011-2026 Guangcong Luo and other contributors).
  Redistributing the data in any form, including generated Rust, needs both notices. They
  are already in `NOTICE`.
- `build.rs` uses learnset keys only, never the method codes (`collect_learnset`).
- `build.rs` already emits `rerun-if-changed` for each input file and
  `rerun-if-env-changed=VGC_DEX_DIR`.
- The Champions corrections (`MEGA_FORME_FIXES`, `EXTRA_ABILITIES`) are applied in
  `build.rs` after reading, so they work the same whatever the source of the JSON.

## Options

### (a) Vendor trimmed JSON in the crate

Commit the six files under `crates/vgc-engine-data/data/`, trimmed to the fields
`build.rs` reads, with a small script (`scripts/trim-dex.mjs`) that regenerates them from
`@pkmn/dex` at a pinned version. `build.rs` defaults to that directory; `$VGC_DEX_DIR`
stays as an override.

- Size: about 1.9 MB in git, about 0.2 MB in the `.crate` file. Well under the crates.io
  10 MB limit.
- Licence: add `NOTICE` (or a `LICENSE-THIRD-PARTY` copy) inside the crate directory and
  list it in `include`, so the packaged crate carries the MIT notices.
- Maintenance: a dex update means rerunning one script and committing a reviewable JSON diff
  (a new move or species shows up as data, not as generated code).
- Risk: the trim script must keep every field `build.rs` reads. A missing required field
  fails the build, but fields marked `#[serde(default)]` would silently default, so step 4
  below (byte-identical `data_tables.rs`) is the real check.

### (b) Fetch or generate at build time

`build.rs` downloads the `localdex` or `@pkmn/dex` tarball, or runs Node to dump it, when
`$VGC_DEX_DIR` is unset.

- Size: nothing in git.
- Licence: same notices needed for binaries; nothing vendored in source.
- Maintenance: no data diffs to review, but upstream changes arrive invisibly unless pinned
  by checksum.
- **Breaks docs.rs and offline builds.** docs.rs builds have no network,
  and many users build offline or in sandboxes. Requiring Node for a Rust crate is a worse
  dependency than the one being removed. Build scripts that hit the network are also widely
  disliked by Rust users.

### (c) Commit the generated Rust

Commit `data_tables.rs` (today written to `OUT_DIR`) and `include!` it directly. Keep
`build.rs` as an opt-in regenerator (behind an env var or `cargo xtask`).

- Size: about 2.4 MB of Rust in git; similar compressed size to (a).
- Licence: same notices; generated code from MIT data is still a derivative.
- Maintenance: diffs are large generated Rust, hard to review. Every change to the codegen in
  `build.rs` forces a full regeneration commit, and a stale committed file can silently
  disagree with `build.rs`. Needs a CI check that regenerates and diffs.
- Benefit: no build script at build time, so builds are faster.

## Recommendation: (a), vendored trimmed JSON

- Like (c), it builds from crates.io, offline and on docs.rs with no Node or network. Unlike
  (c), it keeps today's build logic unchanged and needs no stale-file check.
- Updates stay reviewable as data diffs, and `build.rs` keeps being the one place where
  codegen and the Champions corrections live.
- 0.2 MB packaged is small, and the licence work is already done (`NOTICE`).

Steps when this is picked up:

1. Add `scripts/trim-dex.mjs`: load `@pkmn/dex` at a pinned version, emit the six files with
   only the fields `build.rs` reads (keep learnset values as empty arrays so `LearnsetMap`
   parses unchanged), sorted keys, one stable format.
2. Commit the output to `crates/vgc-engine-data/data/`, plus `NOTICE` in the crate and an
   `include` list in its `Cargo.toml`.
3. Change `dex_dir()` to default to `CARGO_MANIFEST_DIR/data`, keeping `$VGC_DEX_DIR` as an
   override. Drop the `$HOME/Dev/localdex/data` fallback.
4. Check the generated `data_tables.rs` is byte-identical before and after (same tables from
   trimmed input), then run `cargo test --workspace --exclude vgc-engine-py`.
5. Remove the download step from CI.
