# Target regressions — pre-fix baseline (2026-10-07)

**Status:** pre-fix baseline only. Measured on worktree HEAD `14fb664` (no crates/tools/Cargo
source diff to current main `6076d6d`, per manager check). This is **not** a final main
verification, not a post-fix run, and not gate acceptance. Phase gates in [`PLAN.md`](PLAN.md)
are unchanged and still UNMET; no replacement thresholds are adopted. The product is incomplete.

## How it was run

Cole approved six local Cargo test runs and two targeted source reads. The manager ran each
once, in `.claude/worktrees/native-{ripen,keen-eye,minds-eye}-20261007`, with separate
`/tmp/vgc-approved-{ripen,keen-eye,minds-eye}-20261007T180351Z` targets:

```
cargo test --offline -j4 -p vgc-engine-core --test ripen_healing_berries
cargo test --offline -j4 -p vgc-engine-core --test keen_eye_accuracy
cargo test --offline -j4 -p vgc-engine-core --test minds_eye_accuracy
```

Each was repeated with `--features chance,ps-rng`. All six compiled and exited 101 on
assertions. Receipt: `approved-target-checks-20261007T180351Z-reviewed.json` (manager evidence
directory; not read here).

| Test file | default (pass/fail) | `chance,ps-rng` (pass/fail) |
|---|---|---|
| `ripen_healing_berries` | 8 / 5 | 8 / 6 |
| `keen_eye_accuracy` | 4 / 5 | 4 / 6 |
| `minds_eye_accuracy` | 4 / 4 | 5 / 5 |

Ignored: 0 in all runs. The tests are untracked drafts; none is final-validated.

## 1. Ripen — `item.rs`

**Confirmed (real defect):** berry healing ignores an effective Ripen multiplier.
- Sitrus, odd max HP 147: 66 vs expected 102 (also 66 vs 102 under `chance`).
- Oran 60 vs 70; Figy 78 vs 126 (first berry in the loop); Bug Bite 86 vs 122.

**Fixture-limited:** the Enigma test fails at its item-consumed precondition (item 126 vs empty
65535), before any multiplier assertion. Source already has an Enigma heal path, without the
multiplier. Don't assume a pure Ripen defect or a missing Enigma implementation.

**Weak control:** Heal Block passes its HP check but doesn't check item retention.

**Green controls:** Leppa, suppression, trigger thresholds, Unnerve, ordinary healing/caps. This
is not a broad accuracy claim.

**Next (local Opus worker, owns `item.rs`):** double the already-truncated berry recovery for an
effective Ripen holder. Keep thresholds, effective suppression and caps. Cover ordinary
triggers, Bug Bite/eaten berries, and Enigma once its fixture proves a surviving super-effective
hit and correct trigger. Strengthen Heal Block against the pinned `onTryEatItem` gate.

**Source (approved read):** PS `a5df8274e85b0889bf2a9b3422a08b39732374fc`
`dist/data/items.js` — Sitrus 5743-5764, Oran 4398-4419, Figy 2057-2081, Aguav 178-202,
Iapapa 2921-2945, Mago 3708-3732, Wiki 7722-7746, Enigma 1857-1879. Handlers go through
`onTryEatItem` → `TryHeal` before eating; Sitrus/Enigma heal ¼ max HP, Oran 10, Figy family ⅓.
Earlier: Ripen `data/abilities.ts:3842-3849` doubles Berry `TryHeal`; `sim/battle.ts:2261-2271`
truncates the base before the event.

## 2. Keen Eye — `accuracy.rs`

**Confirmed (real defect):** Hydro Pump vs −2 evasion gives threshold `Some(100)`, expected
`Some(80)`. The `chance` evasion-only test also fails. The target-evasion term only checks the
move's `ignores_defensive_boosts`, not an effective Keen Eye.

**Fixture-limited:** four threshold/control reds use Supersonic probes that return `None` where a
number is expected, including a non-Keen-Eye control. These are invalid/unsupported probes, not
proof of an ability bug. Fix the probes first. Supersonic is out of scope for this fix.

**Green controls:** learnability fixture, suppressed ability vs +evasion, Aerial Ace (no draw),
forced numeric Hydro Pump miss.

**Next (local Opus worker, owns `accuracy.rs`):** use proven implemented probes. For an effective
Keen Eye, skip only the target's evasion term (negative evasion too). Keep own accuracy stage,
the numeric roll, other modifiers and suppression. No global sure-hit.

**Source (earlier verified):** `data/abilities.ts:2260-2277` (`onModifyMove`),
`sim/battle-actions.ts:690-743` (`ignoreEvasion` skips the target term; own accuracy remains).

## 3. Mind's Eye — `ability.rs`

**Confirmed (real defect):** no guard against accuracy drops. Measured with foe Mud-Slap, foe
Sand Attack, ally Sand Attack (−1 vs 0), an unrelated-state control (only the accuracy stage
differs), and the `chance` branch (−1 vs 0).

**Green controls:** synthetic non-Mind-Eye ability, other stat drops, Mold Breaker, Neutralizing
Gas, `chance` control.

**Next (local Opus worker, owns `ability.rs`):** add an effective Mind's Eye blocker for stat
index 5 (accuracy). Preserve other stats, suppression and source semantics. PS `onTryBoost`
blocks negative accuracy unless `target === source`; it does not ignore own accuracy.

**Caller read (conditional, completed):** `battle.rs` 13395-13425, 14715-14745, 17920-17950,
19315-19345. Status and secondary ally-target call sites already call
`blocks_opposing_stat_drop_for`, so `battle.rs` ownership is not expanded. Revisit only if
corrected, source-aware regressions prove a need.

**Source (earlier verified):** `data/abilities.ts:2619-2642`.

## Boundaries (unchanged)

- The six runs and two reads were a one-time approval, not blanket execution permission.
- The three workers are idle. No mechanic/source changes or commits exist yet; their untracked
  tests and inputs are drafts.
- Proposed standard Gen 9 doubles cases are not captured, run or hashed. No Champions
  roster/fidelity claim.
- Still held: hashes, captures, golden installation, and the older Download / Rest / Avalanche /
  Tera / Gulp / spread / page items.
- Acceptance still needs normally permitted post-fix focused and full validation, real pinned PS
  oracle goldens, step-allocation checks, and final integrated-HEAD checks.
- One mechanic per commit. Implementation edits stay local, by the verified personal-Max
  `claude-opus-5-5` route.
