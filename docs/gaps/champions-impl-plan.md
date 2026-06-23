# Pokémon Champions (VGC Reg M-B) — citation catalog & impl plan

> ⚠️ **Source is authoritative — these status docs rot. Verify before trusting any status below.**
> Counts: abilities `grep -rhoE 'ability_id::[A-Z_0-9]+' crates/vgc-engine-core/src/|sort -u|wc -l`; items `item_id`; moves `move_id` (same pattern).
> Last reconciled: 2026-06-23.

> **HISTORICAL / LARGELY SUPERSEDED (reconciled 2026-06-23).** The headline of
> this plan — the **Champions format/regulation system**, the **Mega Evolution
> mechanic**, and **all 6 new Champions abilities** — has **shipped**. This file
> is retained as a citation catalog and for the handful of still-open rule/move
> overrides. See **"Reconciliation 2026-06-23"** immediately below for the
> shipped/gap split with `file:line` evidence; treat the per-section status
> prose further down as **historical** unless it agrees with that summary.

## Reconciliation 2026-06-23 (grep-verified against source)

Live counts: **220 abilities / 163 items / 237 moves** (`ability_id`/`item_id`/`move_id`, `sort -u`).

**SHIPPED — Foundation:**
- **Format / regulation system** — `format_rules.rs:178` `REG_M_B` ruleset (Reg M-B doubles, level-50, Species/Item clause, Stat-Points budget replacing EVs, 208-entry roster allow-list); `rules_for` matches `"regmb"|"regm"|"regb"|"regbm"|"champions"` at `format_rules.rs:208`. The feared "no mod/regulation system" is built.
- **Mega Evolution mechanic** — `Choice::MegaEvolve` (`choice.rs:66`), `Side::mega_used` permit (`side.rs:161`), `do_mega_evolve` consuming the permit + `set_forme(.., true)` (`battle.rs:1382-1421`), `mega_stone_for` held-stone lookup (the `canMegaEvo` analog, `battle.rs:801,1407`), and the **same-turn post-mega-speed ordering subtlety solved**: `Choice::MegaEvolve` is sorted alongside `Move`/`Terastallize` and forme resolves before action order (`order.rs:136,470,503`; `battle.rs:989`). `MEGA_STONES` data table + per-forme resolved ability (`battle.rs:29387`, `damage.rs:3053`).
- **White Herb custom handler** — `item::try_consume_white_herb` (`item.rs:1363`), wired at multiple stat-drop sites.

**SHIPPED — all 6 new abilities (the catalog's verification target):**
- **Spicy Spray** — `ability.rs:1582` (`ability_id::SPICYSPRAY`, burn-attacker-on-hit).
- **Eelevate** — `pokemon.rs:1870` (`ability_id::EELEVATE` shares the Levitate Ground-immunity path; KO-boost wired).
- **Dragonize** — `damage.rs:826` (`ability_id::DRAGONIZE => Some(14)`, Normal→Dragon -ate).
- **Fire Mane** — `damage.rs:951` (`ability_id::FIREMANE`, ×1.5 holder Fire moves).
- **Mega Sol** — `damage.rs:1613` (`ability_id::MEGASOL`, holder computes as if Sun is up).
- **Piercing Drill** — `battle.rs:4229` (`ability_id::PIERCINGDRILL`, Unseen-Fist-style Protect bypass).

All 6 slugs are present in `/tmp/gt/abilities.txt` (FIREMANE, EELEVATE, DRAGONIZE, MEGASOL, SPICYSPRAY, PIERCINGDRILL).

**STILL OPEN (grep-confirmed genuine gaps):**
- **Paralysis full-para 12.5%** — still gen-9 standard 25% (`battle.rs:2243` `self.rng.range(4) == 0`); the Champions `range(8)` override is **not** wired.
- **Global PP cap 20** — no overlay clamp found; **not shipped**.
- **Healer 50% Champions variant** — still 30% (`ability.rs:1042` `rng.percent_1_100() <= 30`); the mod's 50% (`randomChance(1,2)`) variant is **not** encoded.
- **Move stat/effect overrides** (makeitrain SpA-2, direclaw triple-status, ironhead 20% flinch, etc.) — not individually reconciled here; verify per-slug before trusting the "Move overrides" section below.

---

> **This is a citation catalog, not a progress tracker.** The per-entry PS
> `file:line` refs, hook pointers, complexity, and deps below are stable and
> trustworthy. Any "shipped / missing" *counts* go stale the moment a PR lands —
> **do not trust the status snapshot; regenerate it with an audit pass** (grep
> the engine against the slug lists here).
> Last audit: 2026-06-19. PS source is the **`champions` mod** at
> `data/mods/champions/` (smogon/pokemon-showdown, HEAD `4880d36`, 2026-06-18),
> plus base `data/{pokedex,abilities,moves,items}.ts` entries the mod re-enables
> (flagged `isNonstandard:"Future"`). Re-clone for fresh line numbers:
> `git clone --depth 50 https://github.com/smogon/pokemon-showdown /tmp/ps-latest-research`.

## What this is

**Pokémon Champions** is TPC's standalone competitive battler (Switch Apr 2026,
mobile Jun 17 2026). **Regulation M-B** is its season-2 / 2026-Worlds format.
PS models it as a **Dex mod** (`[Gen 9 Champions] VGC 2026 Reg M-B`), wired in
`config/formats.ts` → `data/mods/champions/`.

The format reintroduces **Mega Evolution** as the sole battle gimmick — **no
Terastallization** (`scripts.ts` `canTerastallize` returns `null`). This is
**Phase 3** work per `docs/PLAN.md` (line 48 already lists the Champions overlay);
it should not start until the Phase 2 gate (≥80% turn agreement) is met.

This is a **mod = data overlay** per `docs/AGENTS.md`: the megas, stones, and
forme links arrive through `build.rs` data generation + a Champions overlay, **not**
hand-written Rust species. Only *effect dispatch* (new abilities, move tweaks,
the mega action, the rule overrides) is engine code. One mechanic per PR; each PR
adds a synthetic golden; cite PS `file:line` + Bulbapedia in the commit.

Behavioral oracle for validation: **18,730 reg-MB replays** already scraping in
`~/Dev/mimikyu/data/replays/gen9championsvgc2026regmb/` (PS protocol logs,
doubles). Use these the same way as the PsGen5 corpus signal.

## Engine readiness map (the good news — verified 2026-06-19)

| Needed for | Exists today? | Where |
| --- | --- | --- |
| Per-turn transform action | **Yes** — mirror it | `choice.rs:24-50` `Choice::Terastallize` variant |
| Once-per-side gate | **Yes** — mirror it | `side.rs:119` `tera_used: bool` |
| Generic mid-battle species swap (stats/type/weight) | **Yes** | `battle.rs:346-374` `set_forme(side, slot, new_species_id, recompute_stats)` |
| Stat recompute from base/IV/EV/nature | **Yes** (auto via `set_forme`) | `pokemon.rs:1409-1444` `compute_stats` |
| Speed sort within priority bracket | **Yes** (sorts once at turn start) | `order.rs:99-165` `effective_speed`, `action_order` |
| Ability dispatch (switch-in/event hooks) | **Yes**, match-arm style | `ability.rs:285+` (`on_switch_in`, `on_switch_out`) |
| Move secondary effects | **Yes**, slug lookup tables | `battle.rs:6491+` (`status_secondary`, `flinch_chance`, …) |
| Format / regulation overlay (PP, para, level overrides) | **No** — must build | `format.rs:10-14` (Doubles/Singles only); `build.rs:292+` filter |

**Verdict:** the Mega *mechanic* is **LOW–MEDIUM**, not the feared rewrite. The
real new infrastructure is (a) the **format-overlay / regulation system** for the
data and rule deltas, and (b) the **one correctness subtlety below**.

### The one correctness subtlety — same-turn mega speed

In gen 6+, mega evolution resolves at **turn start, before any move**, and the
**mega's new Speed is used for that turn's move ordering**. The engine sorts
`action_order` **once** at turn start and does **not** re-sort on mid-turn forme
change (`order.rs`). So mega resolution must run **before** `action_order` in
`step()`: collect all `Choice::Mega` declarations → `set_forme(.., true)` each in
a deterministic sub-phase → *then* compute action order off post-mega speed. Get
this wrong and you mis-order any turn where a slow mon megas into a fast one.
Cite PS `sim/battle-actions.ts` `runMegaEvo` ordering when implementing.

## Complexity tally (entry count, not status)

| Bucket | Count |
| --- | --- |
| Foundation (overlay + mega mechanic + rule overrides) | 6 |
| New abilities | 6 |
| Ability behavior overrides | 5 |
| New mega species (data overlay entries) | ~34 standard + 10 gated |
| Move stat/effect overrides | ~30 (+9 re-enables) |
| Items (stones + White Herb) | ~77 |
| Deferred / orphan | 2 |

## Suggested shipping order

Tracer bullet first (prove the mechanic end-to-end on an *existing* mega), then
fan out. Each numbered item is one PR unless labelled "batch".

1. **Champions format + data overlay scaffolding** (foundation) — load the mod
   tables in `build.rs`, add the `Format::ChampionsRegMB` / regulation handle.
2. **Mega Evolution mechanic** — tracer bullet using an *existing* base-dex mega
   (e.g. Mega Charizard-Y: stat block already present, vanilla ability). No new
   data; pure mechanic. Proves `Choice::Mega` + gate + sub-phase + `set_forme`.
3. **Mega does not revert on faint** — small follow-up to (2).
4. **Rule overrides** — paralysis 12.5%, global PP cap 20, Level Clause stat calc.
   (batch or 3 small PRs)
5. **New abilities** — Fire Mane, Eelevate, Dragonize, Mega Sol, Spicy Spray,
   Piercing Drill (one PR each; several are clones of shipped abilities).
6. **Ability behavior overrides** — Healer 50%, Unseen Fist ¼-dmg, etc.
7. **New mega species data** — batched by which ability they need (the ability
   PR must land first so the forced mega ability resolves).
8. **Move overrides** — batched by effect family (BP-only bump batch, accuracy
   batch, retype batch, then the handful with real effect changes).
9. **Items** — mega-stone batch (data only) + White Herb custom handler.

---

## Foundation

### Champions format + data overlay scaffolding
- **PS:** `config/formats.ts:284-313` (maps BSS + VGC 2026 Reg M-B → `champions`
  mod); mod dir `data/mods/champions/`.
- **Behavior:** `build.rs` currently filters base dex by `keep_gen9()` and drops
  `isNonstandard:"Future"`. Need a path to *include* the Champions overlay entries
  (megas, stones, the 6 new abilities, Nihil Light) when building the Champions
  table, without polluting the standard gen-9 table.
- **Hook:** `build.rs:292+` (`keep_gen9`, `get_ns`); new `Format`/`Regulation`
  handle in `format.rs:10-14`.
- **Complexity:** medium (it's new infra — the engine has no mod/regulation
  system today). **This unblocks everything else.**
- **Deps:** none.
- **Note:** decide overlay strategy — second generated table vs. runtime
  regulation struct selecting a row set. Keep `step()` allocation-free.

### Mega Evolution — the action
- **PS:** `data/mods/champions/scripts.ts:182` `canMegaEvo` returns
  `item.megaStone?.[species.name]` (held-stone lookup; reuses base PS mega engine,
  no custom trigger). Ordering: `sim/battle-actions.ts` `runMegaEvo`.
- **Behavior:** at move selection a side may declare mega-evolve on one active
  mon holding the matching stone; resolves at turn start before moves, in speed
  order; new mega Speed applies same turn (see subtlety above). Once per side
  per battle.
- **Hook:** add `Choice::Mega { actor_slot, move_slot, target }` to
  `choice.rs:24-50` (mirror `Terastallize`); add `mega_used: bool` to
  `side.rs` (mirror `tera_used`); resolve via `set_forme(side, slot, mega_id,
  true)` (`battle.rs:346`) in a new turn-start sub-phase **before**
  `order.rs::action_order`.
- **Complexity:** medium (the sub-phase ordering is the only hard part).
- **Deps:** scaffolding; mega-stone item data.
- **Batch with:** ship the mechanic alone on an existing mega first (tracer).

### Mega Evolution — no revert on faint
- **PS:** `data/mods/champions/scripts.ts:57` overrides `formeChange` — "Don't
  revert Mega Evolutions after fainting."
- **Behavior:** standard PS reverts a fainted mega's forme for team-preview/HP
  bookkeeping; Champions keeps it megaed. Likely a no-op for us depending on how
  faint clears `species_id` — **grep to confirm** our faint path doesn't reset
  forme.
- **Hook:** faint handling in `battle.rs`.
- **Complexity:** trivial (probably already correct).
- **Deps:** mega mechanic.

### Rule override — paralysis full-para 12.5%
- **PS:** `data/mods/champions/conditions.ts:5` — `randomChance(1, 8)` (12.5%)
  vs PS gen-9 standard 25%.
- **Behavior:** paralyzed mon fails to move 1/8 of the time, not 1/4.
- **Hook:** hard-coded `self.rng.range(4) == 0` at `battle.rs:1411-1427` → needs
  a regulation-driven divisor (`range(8)` under Champions).
- **Complexity:** small (but requires the regulation handle to thread a flag).
- **Deps:** scaffolding.
- **Note:** mod `conditions.ts` also tweaks sleep (`startTime = sample([2,3,3])`)
  and freeze (3-turn timer + ¼ thaw) — capture as sub-items if our sleep/freeze
  RNG diverges from the replays.

### Rule override — global PP cap 20
- **PS:** `data/mods/champions/scripts.ts:4` `init()` caps every move's PP at 20;
  `calculatePP` (line 41) = `(pp/5+1)*4`.
- **Behavior:** no move can exceed 20 max PP regardless of base/ups.
- **Hook:** PP init from `data::MOVES[id].pp` (`pokemon.rs:489`); cap at table
  build for the Champions overlay.
- **Complexity:** trivial (clamp at overlay generation).
- **Deps:** scaffolding.

### Rule override — Level Clause Mod stat calc
- **PS:** `data/mods/champions/scripts.ts:10` `statModify` — level-50 adjust with
  a level-dependent formula when `levelclausemod` active; `rulesets.ts`
  `standardag` = Adjust Level 50 / Species Clause / Item Clause 1 / Min Team 6.
- **Behavior:** all mons L50, 31 IVs, "Stat Points" replace EVs. For the engine
  this is mostly a *team-import* concern — we consume final stats — but confirm
  our `compute_stats` (`pokemon.rs:1409`) matches at L50 for these spreads
  against the replays.
- **Hook:** `pokemon.rs:compute_stats`; team builder / set ingestion.
- **Complexity:** small (likely a verification task, not new code).
- **Deps:** scaffolding.

---

## New abilities

All six are defined in base `data/abilities.ts` as `isNonstandard:"Future"` and
re-enabled by the mod. Dispatch like Intimidate (`ability.rs:285+`): add a match
arm at the relevant hook. Several are clones of already-shipped abilities.

### Fire Mane
- **PS:** `data/abilities.ts:1285` (num 316) — `onModifyAtk`/`onModifySpA`
  `chainModify(1.5)` when `move.type==='Fire'`. → Pyroar-Mega.
- **Behavior:** ×1.5 Atk **and** SpA for the holder's Fire-type moves,
  unconditional (Blaze with no HP gate).
- **Hook:** damage power/attack-modify path — engine has **no general
  `on_modify_atk` hook yet** (`battle.rs:3291` scans power multipliers); add one
  arm there or a new `apply_atk_mult`. Reuse the same hook for future abilities.
- **Complexity:** small (needs the modify-attack hook stood up once).
- **Deps:** scaffolding.
- **Bulbapedia:** Fire Mane (Ability).

### Eelevate
- **PS:** `data/abilities.ts:1137` (num 313) — `onSourceAfterFaint` boosts
  holder's best stat by `length` when its move KOs; plus grants Ground immunity /
  airborne (Levitate). `flags:{breakable:1}`. → Eelektross-Mega.
- **Behavior:** Levitate (Ground immunity + hazard immunity) **+** Beast-Boost-style
  +1 to highest stat on each KO by the holder's move.
- **Hook:** Ground-immunity reuses the Levitate path; KO-boost reuses Beast Boost
  (`getBestStat`). Wire both onto one ability id.
- **Complexity:** small (two shipped patterns combined).
- **Deps:** scaffolding; confirm Levitate + Beast Boost already implemented.
- **Bulbapedia:** Eelevate (Ability).

### Dragonize
- **PS:** `data/abilities.ts:1026` (num 312) — Pixilate clone: Normal→Dragon,
  `chainModify([4915,4096])` (×1.2). → Feraligatr-Mega.
- **Behavior:** holder's Normal moves become Dragon, ×1.2 power (same
  `noModifyType` exclusion list as Pixilate).
- **Hook:** the -ate ability path (Pixilate/Refrigerate/etc.) if shipped — same
  arm, different type.
- **Complexity:** trivial (clone of an -ate ability).
- **Deps:** scaffolding; -ate machinery.
- **Batch with:** any other -ate ability work.
- **Bulbapedia:** Dragonize (Ability).

### Mega Sol
- **PS:** `data/abilities.ts:2548` (num 315) — `onWeatherModifyDamage` delegates
  to `sunnyday` (Fire ×1.5 / Water ×0.5 for the holder regardless of real
  weather). → Meganium-Mega.
- **Behavior:** holder computes damage as if Sun is up (does **not** set weather
  for anyone else).
- **Hook:** weather-damage modify in `damage.rs`; gate the sun multiplier on
  holder-has-ability instead of global weather.
- **Complexity:** small.
- **Deps:** scaffolding; sun damage multiplier must exist.
- **Bulbapedia:** Mega Sol (Ability).

### Spicy Spray
- **PS:** `data/abilities.ts:4456` (num 318) — `onDamagingHit`
  `trySetStatus('brn')` on the attacker; Fire-types noted immune. → Scovillain-Mega.
- **Behavior:** burns the attacker on being hit by a damaging move (no % gate);
  like Flame Body at 100%.
- **Hook:** `on_damaging_hit` contact/hit path (mirror Flame Body / Static).
- **Complexity:** trivial (clone of Flame Body at 100%).
- **Deps:** scaffolding.
- **Bulbapedia:** Spicy Spray (Ability).

### Piercing Drill
- **PS:** `data/abilities.ts:3272` (num 311) — Unseen Fist clone: contact moves
  set `bypassProtect`; the bypass deals ×0.25 damage (`scripts.ts modifyDamage`).
  → Excadrill-Mega.
- **Behavior:** holder's contact moves ignore Protect; on a bypass, damage ×0.25.
- **Hook:** Unseen Fist path + the mod's ¼-damage `modifyDamage` rule (see
  override below). If Unseen Fist isn't shipped, this builds it.
- **Complexity:** medium (Protect-bypass + conditional ¼ damage).
- **Deps:** scaffolding; Protect-bypass plumbing.
- **Bulbapedia:** Piercing Drill (Ability).

---

## Ability behavior overrides (mod re-implements existing abilities)

These diverge from the gen-9 versions already in the engine — encode the
Champions variant under the regulation, leave standard gen-9 untouched.

### Healer — 50% cure (not 30%)
- **PS:** mod `abilities.ts` — `randomChance(1,2)`.
- **Divergence:** `docs/gaps/abilities-impl-plan.md` lists Healer at 30%.
- **Hook:** Healer residual in `ability.rs`; regulation-gated chance.
- **Complexity:** trivial.

### Unseen Fist — also deals ¼ damage on bypass
- **PS:** mod `abilities.ts` + `scripts.ts modifyDamage` (×0.25 on Protect bypass).
- **Behavior:** the standard Unseen Fist *plus* the ¼-damage rule shared with
  Piercing Drill.
- **Hook:** damage modify path; pairs with Piercing Drill.
- **Complexity:** small.

### Anger Shell / Berserk — multi-hit trigger timing
- **PS:** mod `abilities.ts` — `onDamage` sets `checkedX = !(Move && !multihit)`.
- **Behavior:** trigger-check timing tweak for multi-hit moves.
- **Hook:** Anger Shell / Berserk threshold check in `ability.rs`.
- **Complexity:** small.

### Disguise — full `onEffectiveness` re-impl
- **PS:** mod `abilities.ts` (substitute / infiltrate handling).
- **Behavior:** verify against our Disguise (PR-shipped) under the mod; may already
  match.
- **Hook:** `ability.rs` Disguise block.
- **Complexity:** small (likely verification).

### Natural Cure / Regenerator — `onSwitchOut` re-impl
- **PS:** mod `abilities.ts` (Regenerator heals `baseMaxhp/3`).
- **Behavior:** confirm our switch-out heal/cure matches the mod exactly.
- **Hook:** `ability.rs:660` (`on_switch_out`).
- **Complexity:** trivial (verification).

---

## New mega species (data-overlay entries)

Stat blocks live in base `data/pokedex.ts` (`isNonstandard:"Future"`, forced
`abilities[0]`, `requiredItem`). These are **overlay data**, not Rust — but each
mega's forced ability must already be implemented, so **gate the mega batch on
its ability PR**. Standard gen-6/7 megas (Charizard, Metagross, etc.) already
have base-dex stat blocks and need no new entry.

**Standard-legal new megas (~34)** — `(types | HP/Atk/Def/SpA/SpD/Spe | ability)`:

- Raichu-Mega-X — Electric | 60/135/95/90/95/110 | Electric Surge
- Raichu-Mega-Y — Electric | 60/100/55/160/80/130 | No Guard
- Clefable-Mega — Fairy/Flying | 95/80/93/135/110/70 | Magic Bounce
- Victreebel-Mega — Grass/Poison | 80/125/85/135/95/70 | Innards Out
- Starmie-Mega — Water/Psychic | 60/100/105/130/105/120 | Huge Power
- Meganium-Mega — Grass/Fairy | 80/92/115/143/115/80 | **Mega Sol**
- Feraligatr-Mega — Water/Dragon | 85/160/125/89/93/78 | **Dragonize**
- Skarmory-Mega — Steel/Flying | 65/140/110/40/100/110 | Stalwart
- Staraptor-Mega — Fighting/Flying | 85/140/100/60/90/110 | Contrary
- Emboar-Mega — Fire/Fighting | 110/148/75/110/110/75 | Mold Breaker
- Excadrill-Mega — Ground/Steel | 110/165/100/65/65/103 | **Piercing Drill**
- Scolipede-Mega — Bug/Poison | 60/140/149/75/99/62 | Shell Armor
- Scrafty-Mega — Dark/Fighting | 65/130/135/55/135/68 | Intimidate
- Eelektross-Mega — Electric | 85/145/80/135/90/80 | **Eelevate**
- Chandelure-Mega — Ghost/Fire | 60/75/110/175/110/90 | Infiltrator
- Golurk-Mega — Ground/Ghost | 89/159/105/70/105/55 | Unseen Fist
- Chesnaught-Mega — Grass/Fighting | 88/137/172/74/115/44 | Bulletproof
- Delphox-Mega — Fire/Psychic | 75/69/72/159/125/134 | Levitate
- Greninja-Mega — Water/Dark | 72/125/77/133/81/142 | Protean
- Pyroar-Mega — Fire/Normal | 86/88/92/129/86/126 | **Fire Mane**
- Floette-Mega — Fairy | 74/85/87/155/148/102 | Fairy Aura (base Floette-Eternal)
- Meowstic-M-Mega / -F-Mega — Psychic | 74/48/76/143/101/124 | Trace
- Malamar-Mega — Dark/Psychic | 86/102/88/98/120/88 | Contrary
- Barbaracle-Mega — Rock/Fighting | 72/140/130/64/106/88 | Tough Claws
- Dragalge-Mega — Poison/Dragon | 65/85/105/132/163/44 | Regenerator
- Hawlucha-Mega — Fighting/Flying | 78/137/100/74/93/118 | No Guard
- Crabominable-Mega — Fighting/Ice | 97/157/122/62/107/33 | Iron Fist
- Golisopod-Mega — Bug/Steel | 75/150/175/70/120/40 | Emergency Exit
- Drampa-Mega — Normal/Dragon | 78/85/110/160/116/36 | Berserk
- Falinks-Mega — Fighting | 65/135/135/70/65/100 | Defiant
- Scovillain-Mega — Grass/Fire | 65/138/85/138/85/75 | **Spicy Spray**
- Chimecho-Mega — Psychic/Steel | 75/50/110/135/120/65 | Levitate
- Glimmora-Mega — Rock/Poison | 83/90/105/150/96/101 | Adaptability

**Gated legendary megas (10)** — stones are **not** in the standard-enabled set
(Ubers/AG only). Defer unless we support those banlists.

- Absol-Mega-Z — Dark/Ghost | 65/154/60/75/60/151 | Magic Bounce
- Garchomp-Mega-Z — Dragon | 108/130/85/141/85/151 | Sand Force
- Lucario-Mega-Z — Fighting/Steel | 70/100/70/164/70/151 | Adaptability
- Heatran-Mega — Fire/Steel | 91/120/106/175/141/67 | Flash Fire / Flame Body
- Darkrai-Mega — Dark | 70/120/130/165/130/85 | Bad Dreams
- Baxcalibur-Mega — Dragon/Ice | 115/175/117/105/101/87 | Thermal Exchange / Ice Body
- Tatsugiri-(Curly/Droopy/Stretchy)-Mega — Dragon/Water | 68/65/90/135/125/92 | Commander / Storm Drain
- Zeraora-Mega — Electric | 88/157/75/147/80/153 | Volt Absorb
- Magearna-Mega (+ -Original) — Steel/Fairy | 80/125/115/170/115/95 | Soul-Heart
- Zygarde-Mega — Dragon/Ground | 216/70/91/216/85/100 | Aura Break

- **Complexity:** trivial each (data), but **blocked on the forced ability**.
- **Batch with:** group by ability dependency; ship the vanilla-ability megas
  (Intimidate, Contrary, Tough Claws, Regenerator, Adaptability, Trace, No Guard,
  Levitate, etc. — all shipped abilities) first as data-only batches; hold the
  6-new-ability megas until those ability PRs land.

---

## Move overrides

Most mod `moves.ts` entries just flip `isNonstandard`. Re-enabled customs
(`null`): burnup, corrosivegas, electrify, kingsshield, **lightofruin**,
powershift, **snaptrap**, stormthrow, trickortreat (data-only). Genuine
stat/effect changes to encode:

### Real effect changes
- **makeitrain** — acc 95, adds self `spa: -2`; spread. (`moves.ts`)
- **direclaw** — +slicing flag; 30% secondary → `sample(['psn','par','slp'])`.
- **freezedry** — secondary removed; keeps super-effective-vs-Water.
- **toxicthread** — Spe −2 (was −1) + poison.
- **saltcure** — residual 1/16 (1/8 on Steel/Water); custom `condition`.
- **snaptrap** — retyped **Steel**.
- **growth** — type set Grass.
- **fakeout / firstimpression** — custom `onDisableMove` (first-active-turn gate
  via `activeMoveActions`); firstimpression BP 100.
- **ironhead** — flinch 20% (was 30%).
- **moonblast** — SpA −1 at 10% (was 30%).
- **encore** — custom `condition` override.

### Mechanical (data-only) batches
- **BP bumps:** anchorshot 90, appleacid 90, beakblast 120, boltbeak 80,
  bonerush 30, dragonhammer 100, firelash 90, fishiousrend 80, gravapple 90,
  infernalparade 65, mountaingale 120, nightdaze 90, psyshieldbash 90,
  snipeshot 85, spiritshackle 90, tripledive 35, tropkick 85, geargrind (BP 60).
- **Accuracy:** crabhammer 95, syrupbomb 90, geargrind 90, clangoroussoul `true`.
- **+slicing flag:** crushclaw, dragonclaw, shadowclaw, metalclaw.
- **PP clamps** (also globally capped 20 by `scripts.ts init()`): banefulbunker 5,
  protect 5, kingsshield 5, obstruct 5, spikyshield 5, sandstorm/snowscape 5,
  shelltrap 10, spinout 10, nightslash 20, …

- **PS:** base `data/moves.ts` + `data/mods/champions/moves.ts`.
- **Hook:** stat fields → overlay data; effect changes → `battle.rs:6491+`
  secondary tables (regulation-gated where they differ from gen-9).
- **Complexity:** data batches trivial; effect changes small.
- **Deps:** scaffolding.

---

## Items

### Mega-stone batch (data only)
- **PS:** mod `items.ts` enables **76 stones** via `isNonstandard:null`; each
  carries `megaStone:{ "<Species>": "<Species>-Mega" }` + `onTakeItem` lock in
  base `items.ts`. New Champions stones include Barbaracite, Chimechite,
  Crabominite, Dragalgite, Drampanite, Eelektrossite, Emboarite, Excadrite,
  Falinksite, Feraligite, Floettite, Glimmoranite, Golurkite, Greninjite,
  Hawluchanite, Malamarite, Meganiumite, Meowsticite, Pyroarite, Raichunite X/Y,
  Scolipite, Scovillainite, Scraftinite, Skarmorite, Staraptite, Starminite,
  Victreebelite, Chesnaughtite, Delphoxite, Clefablite, Chandelurite.
- **Behavior:** held stone enables mega-evolve for its species; cannot be removed
  (Knock Off / Trick locked via `onTakeItem`).
- **Hook:** overlay item table + the `canMegaEvo` lookup in the mega action.
- **Complexity:** trivial (data) — but the `onTakeItem` lock needs the
  Knock-Off/Trick guard wired.
- **Deps:** mega mechanic.

### White Herb — custom handler
- **PS:** mod `items.ts` — custom handler queueing a `WhiteHerb` event at order 99
  (a desync / Parting-Shot fix). The one non-mega custom item.
- **Behavior:** restores lowered stats; timing-fixed vs base.
- **Hook:** White Herb item logic in `item.rs`/`battle.rs`.
- **Complexity:** small.
- **Deps:** scaffolding.

---

## Deferred / orphan

### Nihil Light — orphaned move
- **PS:** base `data/moves.ts:12759` (num 920) — Dragon, Special, 100 BP, 100 acc,
  `allAdjacentFoes`, `ignoreEvasion` + `ignoreDefensive` + `ignoreImmunity:{Dragon}`;
  mod sets pp 5. **No species learns it** in mod or base learnsets (0 hits).
- **Status:** dead data in this snapshot — likely an unfinished signature for a
  Dragon mega (Feraligatr / Dragalge / Drampa?). **Do not block on it**; revisit
  if a later mod commit adds a learnset.

### Gated legendary megas
- See table above — stones not in the standard-enabled set. Defer until/unless we
  model the Ubers/AG banlists.

---

## Open decisions (settle before PR-1)

1. **Overlay strategy** — second generated data table vs. runtime regulation
   struct selecting a row set. Affects `build.rs` and `Battle::new` (must stay
   allocation-free in `step()`).
2. **Mega speed sub-phase placement** — confirm `step()` resolves megas before
   `order.rs::action_order` so post-mega Speed orders the turn (the one
   correctness subtlety).
3. **Stat-spread authority** — PS `champions` mod `pokedex.ts` is the oracle;
   mimikyu's `data/ps_data/pokedex.json` looked **pre-1.1.0** (Pyroar still
   Rivalry/Unnerve/Moxie, not Fire Mane). Refresh mimikyu from the mod before
   trusting its numbers.
4. **Validation** — wire the 18,730 reg-MB replays into the golden harness as a
   Champions corpus signal once the mechanic lands.
