# Threshold-aware canonical hash — design

**Status:** design, no code change.
**Branch:** `design/threshold-aware-canonical-hash` (based on `perf/solver-damage-3bucket`).
**Layers on:** PR-J (`perf/solver-tt-canonical-hash-audit`) — `CanonicalPokemonView` + `CanonicalSideConditionsView`.
**Motivation:** `docs/perf/2v2_baseline_2026_06_29.md` measures 2,112 deduped outcomes per cell vs. an estimated true distinct-state count of ~100. The gap is HP-exact hashing — sixteen damage rolls leaving a defender at HP ∈ {73..88} produce sixteen TT keys when the downstream future is identical for every site the engine actually consults.

The fix: hash HP into **buckets that are unions of the open intervals between every engine-consulted HP threshold**. Two states sharing a bucket take the same branch at every `if hp <op> threshold` site → lossless within engine semantics.

---

## §1. Catalog of HP-threshold consultation sites

Searched with `rg -n 'current_hp|\.hp\s*[<>=!]|hp\s*/|maxhp' crates/vgc-engine-core/src --type rust` plus targeted slug grep. Sites that *write* HP (heal/recoil/chip/sub-cost) are excluded — they don't read a threshold; their input is the resulting bucket of a different mon's read site.

### §1.A — Discrete-boundary thresholds

| # | Threshold | Predicate | Mechanic | Site | Conditional on |
|---|---|---|---|---|---|
| A1 | `hp == 0` | KO / faint flip | every faint check | `pokemon.rs:1002` (`is_alive`); `pokemon.rs:1777`; `battle.rs:6033-6039` | universal |
| A2 | `hp == maxhp` | Focus Sash guard | survive lethal hit | `item.rs:175-187` (`current == max && incoming >= current`) | item == Focus Sash |
| A3 | `hp == maxhp` | Sturdy guard | survive lethal hit | (Sturdy ability — same `hp == max` gate as Focus Sash; see `item.rs:198-201` comment block); ability check elsewhere | ability == Sturdy |
| A4 | `hp == maxhp` | Multiscale / Shadow Shield ×0.5 | damage mod | `damage.rs:2280-2282` (`defender.current_hp >= def_stats.hp`) | def_ab ∈ {Multiscale, Shadow Shield} |
| A5 | `hp == maxhp` | Tera Shell SE-downgrade | damage mod | `damage.rs:2225-2238` (`defender.current_hp >= def_stats.hp`) | def species = Terapagos-Terastal *and* ability = Tera Shell. Reg M-B: Terapagos illegal (not in 208-species allowlist; verified `format_rules.rs`). **Practically dead in our format.** |
| A6 | `sub_hp == 0` | Substitute active vs broken | every "behind sub?" check | `pokemon.rs:1908` (`substitute_hp()`); many call sites (`battle.rs:5375`, `:10759`, `:11067`, `:12499`) | volatile `Substitute` present |

### §1.B — Fractional thresholds

| # | Threshold | Predicate (integer-safe) | Mechanic | Site | Conditional on |
|---|---|---|---|---|---|
| B1 | `hp ≤ ½ max` | `2·hp ≤ max` | Defeatist ×0.5 atk/spa | `damage.rs:1921-1922` | ability == Defeatist |
| B2 | `hp ≤ ½ max` | `2·hp ≤ max` | Sitrus Berry eat | `item.rs:260` | item == Sitrus Berry |
| B3 | `hp ≤ ½ max` | `2·hp ≤ max` | Oran Berry eat | `item.rs:305` | item == Oran Berry |
| B4 | `hp ≤ ½ max` | `2·hp ≤ max` | Berserk / Anger Shell **crossing** detect | `ability.rs:1442-1469` (Berserk); `ability.rs:1516-1530` (Anger Shell) | ability + already-not-fired (state of the mon, not of the bucket) |
| B5 | `hp ≤ ⅓ max` | `3·hp ≤ max` | Overgrow / Blaze / Torrent / Swarm ×1.5 same-type atk/spa | `battle.rs:3308-3310` | ability ∈ {OVERGROW, BLAZE, TORRENT, SWARM} |
| B6 | `hp ≤ ¼ max` | `4·hp ≤ max` | Custap Berry priority bump | `order.rs:374-381` (`m.current_hp * 4 <= m.stats.hp`) | item == Custap Berry, berry-eatable |
| B7 | `hp ≤ ¼ max` | `4·hp ≤ max` | Substitute *failure* gate (can't sub from ≤25%) | `battle.rs:10597-10602` (`a.current_hp <= cost` where `cost = max/4`) | universal (Sub is universal TM) |
| B8 | `hp ≤ ¼ max` | `4·hp ≤ max` | Pinch stat berries — Liechi / Ganlon / Petaya / Apicot / Salac | `item.rs:~273-298` (pinch_entry block) | item ∈ {LIECHIBERRY, GANLONBERRY, PETAYABERRY, APICOTBERRY, SALACBERRY} |
| B9 | `hp ≤ ¼ max` | `4·hp ≤ max` | Figy / Wiki / Mago / Aguav / Iapapa heal berries | `item.rs:~326-340` | item in pinch-heal-berry set |
| B10 | `hp ≤ ½ max` | `2·hp ≤ max` | Belly Drum cost gate | `battle.rs:12098, 12107` (`a.current_hp <= cost`, cost = max/2) | move = Belly Drum / Fillet Away |
| B11 | `hp ≤ 33/100 max` | `100·hp ≤ 33·max` | Clangorous Soul cost gate | `battle.rs:12100, 12107` (cost = `max * 33 / 100`) | move = Clangorous Soul |

**Gluttony note** — Gluttony shifts B8/B9 from ≤25% to ≤50%; `item.rs:272` ("Pinch stat berries — fire at ≤25% HP (Gluttony ≤50%, deferred)") confirms **Gluttony is not yet implemented**. When it lands, B8/B9 bucket boundaries shift to B2/B3 for Gluttony holders. Track as a §6 risk: any threshold-aware bucketing PR must coordinate with the Gluttony PR.

### §1.C — Continuous HP-fraction reads (NOT collapsible to discrete buckets)

| # | Mechanic | Site | Shape | Engine status |
|---|---|---|---|---|
| C1 | Eruption / Water Spout / Dragon Energy BP | `damage.rs:898-910` | `bp = max(1, 150 · hp / max)` — continuous attacker HP | **implemented** |
| C2 | Endeavor try-immunity gate | `battle.rs:3772` (`attacker.current_hp >= defender.current_hp`) | comparison of two continuous HPs | **implemented** |
| C3 | Pain Split | `battle.rs:12136-12158` | averages two continuous HPs, writes both | **implemented** |
| C4 | Super Fang / Ruination | `battle.rs:3745-3753` (fixed-damage list); damage = `max(1, hp/2)` of TARGET | continuous target HP determines damage dealt | **implemented** |
| C5 | Reversal / Flail BP | — | 5 BP bands per Bulbapedia | **NOT implemented** (grep `rg -n 'REVERSAL|FLAIL' crates/vgc-engine-core/src` returns only a doc-comment at `damage.rs:2482`) |
| C6 | Crush Grip / Wring Out BP | — | `floor(120 · hp / max)` | **NOT implemented** (no `CRUSHGRIP` slug match outside a test team) |
| C7 | Final Gambit | `battle.rs:3745-3753` (fixed-damage list); damage = attacker `current_hp`, user faints | continuous attacker HP | **implemented** |
| C8 | Counter / Mirror Coat / Metal Burst | `battle.rs:3745-3753`; uses `last_phys_damage` etc., not current HP | NOT an HP-threshold consult — keys off `last_damage_taken` which PR-J already drops | n/a |

**Continuous-HP move set (implemented, must be handled by §4):** Eruption, Water Spout, Dragon Energy, Endeavor, Pain Split, Super Fang, Ruination, Final Gambit.

Reversal / Flail / Crush Grip / Wring Out **are absent from the engine today** — flag the design but no buckets needed until they ship.

---

## §2. Side conditions / counters / volatiles

Audit of non-HP integer state the hash currently emits via PR-J's `CanonicalSideConditionsView` and `CanonicalPokemonView`.

| Field | Consulted shape | Bucket-safe collapse? | Evidence |
|---|---|---|---|
| `tailwind_turns` | Decrement-to-0; effect active iff `> 0` | **NO** — speed-mod is the same for any non-zero value, but turn count IS visible (mon plans for cliff). Keep exact. | `side.rs` / `battle.rs:9270+` residual decrement loop |
| `reflect_turns`, `light_screen_turns`, `aurora_veil_turns` | Same — active iff `> 0`, but cliff turn is plan-relevant | Keep exact. | `battle.rs:10585-10590` |
| `safeguard_turns`, `mist_turns` | Same | Keep exact. | |
| `trick_room_turns`, `gravity_turns`, `magic_room_turns`, `wonder_room_turns` | Same | Keep exact. | `canonical_hash.rs:67-70` |
| `stealth_rock` (bool) | `true` / `false` | Already 1 bit. | `canonical_hash.rs` |
| `toxic_spikes_layers` (0/1/2) | discrete 3 states | Already collapsed. | |
| `spikes_layers` (0/1/2/3) | discrete 4 states | Already collapsed. | |
| `sticky_web` (bool) | `true`/`false` | Already 1 bit. | |
| `tera_used`, `mega_used` | bool | Already 1 bit. **Reg M-B: Tera banned**, so `tera_used` is dead. Could drop. | format note in memory `project_regmb_format_scope` |
| `sleep_turns` | counter; mon wakes at 0; rolled-distribution of remaining turns matters for plan | **Keep exact** — different counts give different wake probabilities. | `pokemon.rs:1883-1892` |
| `confusion_turns` | counter; clears at 0; remaining = future-snap-out probability | **Keep exact.** | `pokemon.rs:1956` |
| `encore_turns`, `disable_turns`, `taunt_turns`, `heal_block_turns` | each a residual counter, ticks down | **Keep exact** — countdown horizon affects plan. | `pokemon.rs:1452, 1496, 1675, 1836` |
| `toxic_counter` | linear damage ramp (n/16 of max HP) | **Keep exact** — different counters produce different future chip. | `pokemon.rs:1932` |
| `locked_move_slot` | discrete (255 = none, else slot 0..3) | already discrete | `pokemon.rs:1377` |
| `slow_start_active_turns`, `truant_loafing`, `cud_chew_counter` | discrete small counters | Keep exact. | `canonical_hash.rs` (PR-J) |
| `*_this_turn` side-condition fields (Wide Guard / Quick Guard / Mat Block / Crafty Shield / Round) | PR-J already drops them | already done | PR-J `canonical_hash.rs` |
| `pp` | exact slot-by-slot PP | exact reads (Pressure / Leppa / PP-out trigger 0) — Leppa fires at PP==0; otherwise just non-zero/zero matters for selection legality. | `pp_system_exists` memory; can theoretically collapse `>0` PP into one bucket per slot at the cost of distinguishing 1 PP left vs 8 PP left. **Defer to §7 PR-K3.** |

**Conclusion:** the bucketable wins outside HP are slim. The largest non-HP win available is collapsing `pp[i]` from `0..max_pp` to `{0, ≥1}` per move slot — but only safe if Leppa-on-bench is the only PP-aware item, since holders that *plan* PP-stall logic care about ≥-than-N counts. Recommend leaving §2 untouched in the first PR and revisiting only after measuring §1's headline gain.

---

## §3. Concrete bucket function

Synthesizing §1.A + §1.B (universal — applies to every Pokemon regardless of moveset; §1.C handled by §4):

```rust
/// Maps `current_hp` into a bucket index identical across HP values
/// sharing downstream branching behavior. Universal coarse bucketing
/// (independent of moveset / item / ability). Pokemon owning continuous
/// HP-fraction moves (§4) use the fine bucket instead.
fn hp_bucket_coarse(hp: u16, max: u16) -> u8 {
    if hp == 0 { return 0; }                 // KO (A1)
    if hp == max { return 7; }               // Full HP (A2, A3, A4, A5)
    // Predicates use integer-safe forms identical to engine code.
    // Each boundary check is the predicate AS WRITTEN in §1.B.
    let hp = hp as u32;
    let max = max as u32;
    if 4 * hp <= max { return 1; }           // ≤ ¼  (B6, B7, B8, B9)
    if 3 * hp <= max { return 2; }           // (¼ , ⅓]   (B5)
    if 100 * hp <= 33 * max { return 3; }    // (⅓ , 33/100]  (B11) — narrow, but the cost gate is sharp
    if 2 * hp <= max { return 4; }           // (33/100 , ½]  (B1, B2, B3, B4, B10)
    // hp > ½ max and hp < max
    5                                        // (½ , <max)
}
```

**Bucket inventory (8 indices, 0..7):**

| Bucket | Range | Justifying §1 entries |
|---|---|---|
| 0 | `hp == 0` | A1 |
| 1 | `0 < hp ∧ 4·hp ≤ max` (i.e. `(0, ¼]`) | B6 Custap, B7 Sub-cost, B8 pinch stat berries, B9 pinch heal berries |
| 2 | `4·hp > max ∧ 3·hp ≤ max` (`(¼, ⅓]`) | B5 Overgrow/Blaze/Torrent/Swarm |
| 3 | `3·hp > max ∧ 100·hp ≤ 33·max` (`(⅓, 33%]`) | B11 Clangorous Soul cost — boundary at 33% is one integer apart from 33/100 — required for Clangorous Soul holders only, but cheap to keep universal |
| 4 | `100·hp > 33·max ∧ 2·hp ≤ max` (`(33%, ½]`) | B1 Defeatist, B2 Sitrus, B3 Oran, B4 Berserk/Anger Shell crossing, B10 Belly Drum/Fillet Away |
| 5 | `2·hp > max ∧ hp < max` (`(½, max)`) | (no direct trigger; the "safe middle" — every threshold is below or at full) |
| — | bucket 6 unused in coarse form; reserve slot for Multiscale-half if a future PR splits the (½, max) range. | |
| 7 | `hp == max` | A2 Focus Sash, A3 Sturdy, A4 Multiscale, A5 Tera Shell |

**Boundary precision is load-bearing.** Every check uses the integer-safe form the engine code already uses (e.g. `2·hp ≤ max` for ≤50%, NOT `hp ≤ max/2` — those differ when `max` is odd: `max=99`, `hp=49` gives `2·49=98 ≤ 99` true vs. `hp ≤ max/2 = 49` true; both agree here, but `max=49, hp=24` gives `48 ≤ 49` true vs. `24 ≤ 24` true — fine here too. The canonical form is what PS uses and what the engine sites already match, so we use the same form to guarantee co-trigger).

**Substitute HP** (A6): separate field `Pokemon::substitute_hp() → u16`. Engine reads only `sub_hp > 0`. Collapse to a single bit. Implementation: `Some(sub_hp).filter(|h| *h > 0)` → `Option<()>` (effectively bool); OR if PR-K cares about damage absorption rolls (each sub takes a damage roll and breaks/doesn't break — 16 possible sub_hp values after one attack), keep exact for now and revisit.

**Item-consumed flag** (`Pokemon::consumed_item`): already binary, already in PR-J's view.

---

## §4. Continuous-HP move handling

§1.C inventory (implemented): {Eruption, Water Spout, Dragon Energy, Endeavor, Pain Split, Super Fang, Ruination, Final Gambit}.

Three strategies (per the prompt):

### Strategy A — Per-Pokemon classification (RECOMMENDED)

At hash time, compute `has_continuous_hp_move(mon)` by checking `mon.moves` against a fixed set:

```rust
const CONTINUOUS_HP_MOVE_IDS: &[u16] = &[
    move_id::ERUPTION, move_id::WATERSPOUT, move_id::DRAGONENERGY,
    move_id::ENDEAVOR, move_id::PAINSPLIT,
    move_id::SUPERFANG, move_id::RUINATION,
    move_id::FINALGAMBIT,
    // C5/C6 when they ship: REVERSAL, FLAIL, CRUSHGRIP, WRINGOUT.
];
```

If the mon owns any such move, hash its HP **exactly** (current u16). Otherwise hash via `hp_bucket_coarse`.

Cost: 4 moves × 8-entry lookup = trivially fast, and importantly:
- Granularity only widens for the (rare) mons that actually own these moves.
- Eruption/Water Spout cases (Torkoal-Sun, Wash Rotom) are common enough to matter; Super Fang (Raticate, Magneton via Frisk-style) is rare; Final Gambit common only on Mienshao.

**Threat:** the moveset gate is on the *attacker* (Eruption) but the BP determines the *defender's* post-hit HP — so the defender's bucket needs to be fine in cells where the attacker has Eruption. Easiest correct rule: **if any active mon on either side carries a §1.C move, use exact HP for ALL active mons that turn**. Inactive bench mons can still use coarse — they aren't HP-fraction-sourcing or HP-fraction-sinking until they swap in.

### Strategy B — Universal finest

Always use a hash fine enough that Eruption/Water Spout/Dragon Energy compute identical BP across the bucket. BP = `floor(150 · hp / max)` → 151 distinct values (BP=0..150 → 151 classes). The bucket count is `min(max+1, 151)` per mon. At max_hp=200 (typical), that's 151 buckets — basically no compression vs. the 200-value exact HP. Strategy B is a non-starter.

### Strategy C — Conditional fall-through

Only bucket finely when the move is *queued this turn*. This requires the hash to look at the pending action queue, which is per-step transient. PR-J already excluded the pending queue. Adding it back recouples step-transient state to the TT key — bug magnet. **Reject.**

**Recommendation: Strategy A**, gated on `any_active_has_continuous_hp_move(battle)`. This keeps the common case (no Eruption / Endeavor / Pain Split / Super Fang / Final Gambit on either side) on the 8-bucket fast path, and falls back to per-u16 hashing on the rare turn where it matters.

---

## §5. Expected dedup win

### Anchor data (from `docs/perf/2v2_baseline_2026_06_29.md` §1, lossless multi-ply)

- raw_combos / cell ≈ **3,072**
- deduped outcomes / cell ≈ **2,112** (midgame 2HKO; same for switch-heavy)
- pre-canonical-hash collapse ratio: ~1.45×

### Estimate after coarse bucketing

Each attack draws a 16-value damage roll. In a 2HKO cell (the load-bearing scenario the user cares about), a typical attack leaves the defender at HP somewhere in a 16-wide band, NONE of which crosses a §3 bucket boundary in most rolls. Expected behaviour:

- **Per attack, per defender:** 16 damage rolls → typically 1 bucket (no boundary crossed) or 2 buckets (one boundary in range).
- Per cell, 4 attacks distributed across 2 defenders: each defender accumulates the post-damage HP of up to 2 attacks. After bucketing, each defender's post-cell HP is in 1-3 buckets, not 16-256.
- Net per-cell outcome count after bucketing ≈ (defender1_buckets × defender2_buckets × accuracy_outcomes × secondary_outcomes × crit_outcomes).
  - Defender buckets per cell: ~1-3 each.
  - Accuracy per attack: 2 buckets (PR-B). Across 4 attacks: 2^4 = 16, but many cells have only 2 attacks landing.
  - Crit per attack: 2 buckets. 2^4 = 16.
  - Secondary: 2 buckets. 2^4 = 16.
- Rough cell-level upper bound: `3 · 3 · 2^4 · 2^4 · 2^4` = `9 · 4096` = 36k — that's a *worst-case ceiling*, not a typical, because the orthogonal axes co-dedup heavily (crit and roll alone collapse from 32 raw → ~2-3 buckets per attack after threshold-bucketing, since crit changes the bucket only when the crit damage crosses the bucket border).

### Empirical compression ratio (more honest)

Today: 16 damage rolls × 4 attacks × 2 accuracy × 2 crit × 2 secondary = 1024 per-attack combos collapsed to ~3072 raw / cell at the 4-attack joint — the canonical-hash dedup factor is already ~1.45×. Bucketing replaces the 16-roll axis with ~2 buckets per attack, dropping the raw combos by ~8× per attack. Net per-cell deduped outcomes:

```
2112 / (8^k)  where k = number of attacks whose damage axis collapses
```

With k=2 (both attackers hit a defender in a no-threshold-crossing band): `2112 / 64 ≈ 33`.
With k=4: `2112 / 4096 ≈ 1` (floor of 1 — the cell collapses to a single outcome).

In practice some cells have a boundary crossing (the attack's damage range straddles 50% etc., bucket count = 2-3). Mixing the cases:

- Pure no-boundary 4-attack cell: ~1-5 outcomes
- One boundary-crossing attack: ~5-20 outcomes
- Both attacks boundary-crossing on both defenders: ~30-100 outcomes
- Weighted typical: **~50-150 outcomes / cell**

**Cole's intuition of "~100 unique states per cell" is well-supported.** Order of magnitude is right; mid of the range I estimate. Cell-level wall reduction: 28 ms → ~28 ms × (100/2112) = **~1.3 ms**, i.e. ~20× cell-level wall-time win. At 32k cells, depth-1 lossless solve drops from ~15 minutes to **~45 seconds**.

Caveat per `feedback_verify_agent_estimates`: this is an analytic estimate, NOT measurement. Validation plan in §7 (PR-K1 ships with a `measure_2v2 --bucketed` rerun that re-emits the same table from the baseline doc with the new hash; the dedup ratio is the load-bearing number).

---

## §6. Risk register

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Missed §1 consultation site** → wrong solver answer (states collapsed that shouldn't be). | Grep audit shipped in §1. Add a `debug_assert!` in `canonical_hash` (debug build only) that re-runs a tight test: for a corpus of (mon, hp1, hp2) triples in the same bucket, run a no-RNG step and confirm the resulting battles also hash equal. Catch missed sites via fuzzing the bucket function. |
| R2 | **§1.C continuous-HP move misclassified** → wrong solver answer. | The `CONTINUOUS_HP_MOVE_IDS` table is small (8 entries today) and audited against §1.C; CI grep gate added in PR-K2 (a `move_id::ERUPTION` reference outside this table is a failure). |
| R3 | **PR-J interaction.** PR-J `CanonicalPokemonView` serializes `current_hp` exactly (`canonical_hash.rs:78` in PR-J: `s.serialize_field("current_hp", &p.current_hp)?`). | Replace that one line with `s.serialize_field("hp_bucket", &hp_bucket_for(p, classification))?` where `classification` is decided once per `canonical_hash()` call (Strategy A gate at top-of-function: scan active mons' movesets once, pick coarse/fine). Everything else PR-J landed stays put. |
| R4 | **Mega Evolution changes max HP.** | Verified: gen-9 Mega forms (Mega Charizard X/Y, Mega Mawile, Mega Gengar etc.) **do not change the HP stat** (per `~/Dev/vgc-engine/build.rs` MEGA_FORME_FIXES — see `project_mega_roster_complete` memory; the override table only rewrites non-HP stats and types/abilities). So `max_hp` is stable across mega evolve and the bucket boundary doesn't shift mid-state. No special handling required. |
| R5 | **Tera Blast / Tera consultation lurking.** | Reg M-B BANS Tera (`format_rules.rs`); `tera_used` and `terastallized` exist as state but no §1 entry consults HP under Tera condition. Tera Shell (A5) is dead in our format because Terapagos is illegal. **No new threshold from Tera.** |
| R6 | **Gluttony (B8/B9 shift)** lands later and silently breaks bucketing. | §1 explicitly tracks Gluttony as a deferred shift; PR-K1 commit message + `// FIXME(gluttony)` comments at the §1.B B8/B9 boundary check. |
| R7 | **Substitute HP** — currently exact in PR-J's view. Reducing to a bit risks losing the "how many more hits can sub survive" plan signal. | Keep `substitute_hp` exact in PR-K1 (no regression); add as a §3 optional follow-up if measurement shows substantial residual non-HP bloat. |
| R8 | **Berserk / Anger Shell crossing detection** keys on `last_damage_taken` which PR-J dropped. | PR-J's justification holds: `last_damage_taken` is wiped before the next step, so by the time a TT key is computed, the "did I cross half this step?" answer is already baked into `current_hp` (above or below half). The boundary at ½ (bucket 4 vs 5) preserves this. No additional state needed. |
| R9 | **Cell-count estimate optimism.** | §5 is analytic, not measured. PR-K1 includes the measurement rerun as its acceptance criterion. |

---

## §7. Implementation phasing

### PR-K1 — universal coarse bucketing (Strategy A's "no continuous-HP move" path)

- **Files touched:** `crates/vgc-engine-core/src/canonical_hash.rs` (replace `current_hp` field with bucket; add bucket fn). `crates/vgc-solver/examples/measure_2v2.rs` (add a `--bucketed-stats` flag to rerun the baseline table).
- **LoC:** ~60 (bucket fn + classification gate + PR-J view patch + 2 tests).
- **Tests:** unit-test bucket fn on every §3 boundary HP (both sides of the integer-safe predicate); regression test in `canonical_hash::tests` showing that two battles with HP in the same bucket produce equal hashes; rerun the §1.A `active_hp_change_diverges` test to confirm a *cross-bucket* HP change still diverges.
- **Acceptance:** `measure_2v2 --bucketed-stats` reports outcomes/cell ≤ 250 on the midgame 2HKO scenario from `docs/perf/2v2_baseline_2026_06_29.md`.

### PR-K2 — per-Pokemon §1.C classification

- **Files touched:** `crates/vgc-engine-core/src/canonical_hash.rs` (add `has_continuous_hp_move`); maybe `crates/vgc-engine-core/src/data.rs` (export `CONTINUOUS_HP_MOVE_IDS`). CI grep gate (`tools/audit-continuous-hp-moves/audit.sh`, mirroring `tools/audit-residual-index/`).
- **LoC:** ~40 + 30 audit script.
- **Tests:** an Eruption-holder fixture confirms HP changes within a bucket still produce distinct hashes; a no-§1.C fixture confirms in-bucket HP changes collide.
- **Acceptance:** golden goldens for `corpus_zero_divergences` still pass; an added small synthetic golden where Torkoal + Eruption pre-state HP differs by 1 produces different solver Nash values when bucketed vs unbucketed (proves Strategy A is engaging).

### PR-K3 (optional) — §2 counter / PP bucketing

- Defer until measurement shows post-K1 non-HP state bloat is meaningful. Likely deferred indefinitely.

---

## §8. Open questions

1. **Substitute HP fine-grain.** A sub at 25 HP vs 30 HP behaves identically to the *holder* (sub absorbs damage either way until 0), but the *opponent* may choose a stronger/weaker move based on "can I break this sub?". For solver TT purposes, the opponent's action enumeration is independent of the TT key — only the resolved transition's value depends on sub_hp. Recommendation: collapse to `{0, >0}` in K3 if measurement justifies; keep exact for K1/K2. Need confirmation from Cole.
2. **Sleep / confusion / toxic counter — bucket or keep exact?** Each is a probability cliff (sleep_turns=1 wakes guaranteed turn-end; =3 wakes with low probability). I recommend keeping exact (§2). Worth confirming Cole doesn't want them in scope.
3. **PP counter collapse.** Leppa Berry triggers at PP==0; otherwise only `{0, ≥1}` matters mechanically. But move *legality* is "PP > 0", and once-per-game PP-stall lines (rare in VGC) need exact PP. Recommendation: defer to K3 with measurement.
4. **Gluttony coordination.** When Gluttony PR lands, B8/B9 boundaries shift from ¼ to ½. Bucket fn needs an item-conditional branch. Confirm whether to add the conditional now (so the Gluttony PR is a 1-line data change) or defer.
5. **PR-K1 should it also drop `tera_used` from the hash (Reg M-B)?** Saves zero solve time but a clean prune. Probably yes; confirm.

---

## Appendix — grep transcripts for §1 citations

All citations above were produced from the following searches against `crates/vgc-engine-core/src/**.rs`:

```
rg -n 'current_hp|\.hp\s*[<>=!]|hp\s*/' --type rust
rg -n 'current_hp\s*[<>=!]|current_hp\s*\*|stats\.hp\s*[/<>*]|maxhp' --type rust
rg -n 'pinch|berserk|emergency|wimp|defeatist|multiscale|sturdy|focus.*sash|sitrus|berry|reversal|flail|wring|crush|spout|eruption|endeavor|painsplit|pain_split|anger.*shell|tera.*shell' --type rust -i
rg -n 'REVERSAL|FLAIL|CRUSHGRIP|WRINGOUT|HEATCRASH|LOWKICK|ENDEAVOR|SUPERFANG|NIGHTSHADE|SEISMICTOSS|DRAGONRAGE|FINALGAMBIT|PAINSPLIT|SUBSTITUTE|BELLYDRUM|FILLETAWAY|CLANGOROUSSOUL|WIMPOUT|EMERGENCY|BERSERK|ANGERSHELL|TERASHELL|MULTISCALE|DEFEATIST' damage.rs
rg -n 'sub_hp|substitute_hp' --type rust
rg -n 'sleep_turns|confusion_turns|toxic_counter|encore_turns|disable_turns|taunt_turns|magnetrise|heal_block|choice_lock|locked_move' --type rust
```

Reversal / Flail / Crush Grip / Wring Out / Wimp Out / Emergency Exit / Gluttony all confirmed UNIMPLEMENTED today; they appear only in doc-comments, test team JSON, or deferred-feature notes.
