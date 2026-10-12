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
