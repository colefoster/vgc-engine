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
| 0.5.0 | a2d60bf niche doubles pass: opt-in `ally_targets` (default off; on = larger doubles action space), Mean Look/Block/Jaw Lock/Fairy Lock traps, Parting Shot ally/no-change fixes, Pollen Puff ally Telepathy/Heal Block, Quick Guard adjusted priority + Triage/Gale Wings, Utility Umbrella holder orientation, Weakness Policy real type, bincode round-trip fix. Battle JSON now always carries `fairy_lock_turns`. | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | **not installed** (dist only; install per the rule below) |
| 0.6.0 | 27ea4dc61 Showdown parity rounds 2-5, `save_load` gate; same-seed results change, older snapshots don't load | `protocol` (default), `ps-rng` | cp39, cp312 macOS arm64 | system Python (user site) |

## Release workflow (CI wheels)

`.github/workflows/release.yml` builds wheels with `--features ps-rng` for CPython 3.9-3.13 on Linux x86_64/aarch64 (manylinux), macOS x86_64/arm64 and Windows x64, plus an sdist.

- **Trigger:** push a `v*` tag (`git tag v0.5.0 && git push origin v0.5.0`). The wheels are attached to a GitHub Release for that tag.
- **PyPI is off.** Tag pushes never publish. The `pypi` job runs only from a manual dispatch (Actions > Release wheels > Run workflow) with `publish_pypi` ticked, which defaults to off.
- **Trusted publishing setup (once, before the first PyPI run):**
  1. On pypi.org, add a pending trusted publisher: project `vgc-engine`, owner `colefoster`, repository `vgc-engine`, workflow `release.yml`, environment `pypi`.
  2. In the GitHub repo settings, create the environment `pypi` (optionally with a required reviewer).
  3. No API token is stored; the job uses OIDC (`id-token: write`).
- A manual dispatch from a branch builds wheels as workflow artifacts without making a release.

## Install rule for mimikyu

Before installing a wheel into an environment mimikyu is using, check whether a long-running mimikyu evaluation job is still using the current wheel. If one is, build into `dist/` only and switch after it finishes. A mid-run version change forced a full evaluation restart on 2026-10-12.

## Linux x86_64 build host

Built natively on Linux (Ubuntu 24.04) from a `git archive` of the commit, with a user-local rustup and `maturin` in a dedicated build venv. `VGC_DEX_DIR` must point at the `@pkmn/dex` JSON dump there, because `build.rs` falls back to `$HOME/Dev/localdex/data`. The source tree keeps a `REF` file with the commit; wheels go to a separate dist directory. The target environment has no pip, so install with `uv pip install --python <env>/bin/python <wheel>`.

| Version | Commit | Wheel | Installed into |
|---|---|---|---|
| 0.4.2 | 66c150b | cp311 manylinux_2_34 x86_64 | rollout env (Py 3.11) |
| 0.6.0 | 27ea4dc61 | cp311 manylinux_2_34 x86_64 | colepc `arena-env` and `mimikyu-train-env`; unraid wheel dir |
