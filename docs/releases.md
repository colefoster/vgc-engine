# Python wheel releases

Consumers (mimikyu) pin the wheel version listed here. Wheels are built locally into `dist/` (not committed):

```bash
cd crates/vgc-engine-py
CARGO_BUILD_JOBS=4 maturin build --release --features ps-rng -i PYTHON -o ../../dist
```

| Version | Commit | Features | Wheels | Installed into |
|---|---|---|---|---|
| 0.2.0 | 3251ded (phase 2: PS session, requests, `observe(side)`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | `/usr/bin/python3` (user site), `mimikyu/.venv-analytics` |
| 0.3.0 | 78b4cd9 (phase 4: `Battle.from_position`, `position_from_board`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | `/usr/bin/python3` (user site), `mimikyu/.venv-analytics` |
| 0.3.1 | 8294d87 (round 11: Receiver, Spirit Shackle, spread-drain order, Imposter, Protean) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | `/usr/bin/python3` (user site), `mimikyu/.venv-analytics` |
| 0.4.0 | 3492745 (`position_from_replay`, `vgc recon`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | `/usr/bin/python3` (user site), `mimikyu/.venv-analytics` |
| 0.4.1 | 6b8461d (default prior `smogon:2026-09/gen9championsvgc2026regmc-1500`, pinnable prior ids) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | `/usr/bin/python3` (user site), `mimikyu/.venv-analytics` |
| 0.4.2 | 426ac13 (explicit recharge/lock `legal_choices`, `prior_sample_seed`) | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | `/usr/bin/python3` (user site), `mimikyu/.venv-analytics` |

## Install rule for mimikyu

Before installing a wheel into `/usr/bin/python3` or `mimikyu/.venv-analytics`, check for running kcheck jobs (`tmux ls | grep kcheck-`). If any are running, message the kcheck PM session and wait for its OK; otherwise build into `dist/` only and let the PM switch. A mid-run version change forced a full kcheck-mt restart on 2026-10-12.

## colepc (WSL Ubuntu-24.04, x86_64)

Built natively in WSL from a `git archive` of the commit (rustup user-local, `maturin` in `~/.vgc-build-venv`, `VGC_DEX_DIR=~/vgc-engine-deps/localdex/data` because `build.rs` reads `~/Dev/localdex/data` by default). Source in `~/vgc-engine-src` (`REF` file holds the commit), wheels in `~/vgc-engine-dist`. `~/arena-env` has no pip; install with `~/.local/bin/uv pip install --python ~/arena-env/bin/python`.

| Version | Commit | Wheel | Installed into |
|---|---|---|---|
| 0.4.2 | 66c150b | cp311 manylinux_2_34 x86_64 | `~/arena-env` (Py 3.11, ACT-flareon rollouts) |
