# Python wheel releases

Consumers (mimikyu) pin the wheel version listed here. Wheels are built locally into `dist/` (not committed):

```bash
cd crates/vgc-engine-py
CARGO_BUILD_JOBS=4 maturin build --release --features ps-rng -i PYTHON -o ../../dist
```

| Version | Commit | Features | Wheels | Installed into |
|---|---|---|---|---|
| 0.2.0 | 3251ded (phase 2: PS session, requests, `observe(side)`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site), mimikyu analytics venv |
| 0.3.0 | 78b4cd9 (phase 4: `Battle.from_position`, `position_from_board`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site), mimikyu analytics venv |
| 0.3.1 | 8294d87 (round 11: Receiver, Spirit Shackle, spread-drain order, Imposter, Protean) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site), mimikyu analytics venv |
| 0.4.0 | 3492745 (`position_from_replay`, `vgc recon`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site), mimikyu analytics venv |
| 0.4.1 | 6b8461d (default prior `smogon:2026-09/gen9championsvgc2026regmc-1500`, pinnable prior ids) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site), mimikyu analytics venv |
| 0.4.2 | 426ac13 (explicit recharge/lock `legal_choices`, `prior_sample_seed`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site), mimikyu analytics venv |

## Install rule for mimikyu

Before installing a wheel into an environment mimikyu is using, check whether a long-running mimikyu evaluation job is still using the current wheel. If one is, build into `dist/` only and switch after it finishes. A mid-run version change forced a full evaluation restart on 2026-10-12.

## Linux x86_64 build host

Built natively on Linux (Ubuntu 24.04) from a `git archive` of the commit, with a user-local rustup and `maturin` in a dedicated build venv. `VGC_DEX_DIR` must point at the `@pkmn/dex` JSON dump there, because `build.rs` falls back to `$HOME/Dev/localdex/data`. The source tree keeps a `REF` file with the commit; wheels go to a separate dist directory. The target environment has no pip, so install with `uv pip install --python <env>/bin/python <wheel>`.

| Version | Commit | Wheel | Installed into |
|---|---|---|---|
| 0.4.2 | 66c150b | cp311 manylinux_2_34 x86_64 | rollout env (Py 3.11) |
