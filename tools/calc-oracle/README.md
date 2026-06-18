# calc-oracle

Spec-based correctness signal for the engine. Independent of PS.

For a single-hit scenario:
1. `oracle.js` calls `@smogon/calc` to get the canonical 16-roll damage
   array (and a separate one for crit). Smogon's calc is the
   community-validated source of truth for gen-9 damage.
2. `examples/calc_oracle.rs` runs the engine N times with the same setup
   (defender uses Splash so no counter-damage; defender holds no item so
   no residual heal confounds the HP read), records every observed
   damage value.
3. `compare.py` asserts the engine's observed unique damages are a
   subset of `damage_union = damage ∪ damage_crit`. Anything outside is
   a real bug (engine produced a value the spec says is impossible).

This complements:
- **PsGen5** — bit-exact alignment with PS RNG draws.
- **distribution-test** — statistical agreement with PS (KS test).

Spec-oracle catches bugs in PS that we'd accidentally match.

## Usage

```bash
node tools/calc-oracle/oracle.js \
    tools/calc-oracle/scenario-cc-lifeorb.json > /tmp/calc.json

cargo run --release -q -p vgc-engine-golden --example calc_oracle \
    -- tools/calc-oracle/scenario-cc-lifeorb.json 1> /tmp/eng.json 2>/dev/null

python3 tools/calc-oracle/compare.py /tmp/eng.json /tmp/calc.json
```

## Scenario shape

```json
{
  "name": "...",
  "attacker": {
    "species": "Lucario", "level": 50, "item": "Life Orb",
    "ability": "Inner Focus", "nature": "Adamant",
    "evs": { "atk": 252, "hp": 4, "spe": 252 }
  },
  "defender": {
    "species": "Garchomp", "level": 50,
    "ability": "Sand Veil", "nature": "Impish",
    "evs": { "hp": 252, "def": 252, "spd": 4 }
  },
  "move": "Close Combat",
  "trials": 500
}
```

The defender intentionally holds no item — the harness reads HP delta
after turn 1 and any end-of-turn heal/chip would skew the
measurement. Leftovers / Sitrus / etc. on the defender are NOT
supported (use distribution-test for those interactions instead).
