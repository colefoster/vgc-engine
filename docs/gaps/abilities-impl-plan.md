# Abilities — citation catalog

> **This is a citation catalog, not a progress tracker.** The per-entry PS
> `file:line` refs, hook pointers, complexity, and deps below are stable and
> trustworthy. Any "shipped / missing" *counts* go stale the moment a PR lands —
> **do not trust the status snapshot; regenerate it with an audit pass** (grep
> `ability.rs` + status/boost guards in `battle.rs` against the slug list).
> Last audit: 2026-06-19. PS line numbers are from
> `/tmp/pokemon-showdown-research/data/abilities.ts` (gen-9 head).

## Status snapshot (2026-06-19 audit — verified source-grep)

Round 10 shipped almost everything. The short tail below is what is left.

**SHIPPED this round (PRs 318–335):** Own Tempo, Inner Focus, Damp, Pastel
Veil, Sweet Veil, Moody, Color Change, Protean, Libero, Toxic Debris, Wind
Rider, Wind Power, Cute Charm, Cursed Body, Mirror Armor, Disguise, **Magic
Bounce** — **plus the systems they depend on** (PRs 309–317, 334: Toxic Spikes,
runtime type-override, Disable full-apply, wind-move flag + Charge volatile,
gender, Attract, reflectable flag, forme-change, source-threaded `apply_boosts`,
status-move explicit target-slot). These join the
prior-round shipped set (bigpecks, keeneye-acc, vitalspirit, insomnia, limber,
magmaarmor, immunity, waterveil, cloudnine, airlock, healer, cottondown,
poisontouch, slowstart, truant, shadowshield, rattled, + ~28 earlier).

**REMAINING — the short tail:**
- **Wonder Guard** — SHIPPED (PR-336). Only-SE damage gate in the
  move-immunity block; Mold-Breaker-breakable; status / Struggle / indirect
  damage unaffected.
- **Aroma Veil** — SHIPPED for every reachable volatile (PR-337): immunity
  (holder + ally aura) wired at all THREE live application sites — Attract
  (move + Cute Charm), Encore (move), Disable (Cursed Body). Taunt / Torment /
  Heal Block have no setter in the engine yet, so their Aroma Veil immunity is
  **vacuous-pending** — wire it when those volatiles first gain an applier.
- **Pressure** — SHIPPED (PR-338): the engine tracks PP, so Pressure now
  deducts +1 PP per foe target holding it (spread moves sum per Pressure
  target). The switch-in `-ability` message is cosmetic and intentionally
  skipped. Hooked at every PP-decrement site in `battle.rs`.
- **Frisk** — deferred no-op (information-only; no battle effect to model).

**Note:** Wind Rider / Wind Power are otherwise complete; only their
**Tailwind-set triggers** are missing (the wind-move-hit paths are done — see
`ability.rs:1081`).

## Original complexity tally (entry count, not status)

| Complexity | Count |
| --- | --- |
| Trivial | 11 |
| Small | 14 |
| Medium | 10 |
| Hard | 8 |
| Deferred (gen-9 N/A or non-effect) | 1 (Frisk; Pressure shipped PR-338) |
| **Total entries** | **45** |

## Suggested shipping order

Cheap-and-impactful first; blocked entries last. Each numbered item is a single PR unless
labelled "batch".

1. **Stat-drop guard batch** — Big Pecks + Keen Eye (Acc-drop only) + Vital Spirit /
   Insomnia (sleep immune) + Inner Focus (flinch immune) + Own Tempo (confusion immune)
   + Limber / Magma Armor / Immunity / Water Veil / Oblivious (status immunities).
   All are flat slug arms in the existing stat-drop / status-set guard block. One PR per
   2-3 slugs.
2. **Cloud Nine / Air Lock** — single weather-suppression flag consulted by damage and
   residual phases. Cheap and high-leverage for corpus accuracy.
3. **Soundproof** — sound-immune slug in the move-immunity block (sound table already
   exists post PR-51).
4. **Bulletproof** — same shape as Soundproof against `flags.bullet` table.
5. **Damp** — single guard in pre-move resolve against an explode-move list.
6. **Friend Guard** — ally-side ×0.75 damage mod; doubles-only but Clefairy is a staple.
7. **Telepathy** — ally-damaging immunity check; small.
8. **Unaware** — boost-ignoring branch in stat-stage application during damage; medium
   but very high-leverage (shows up in Clodsire / Quagsire lines).
9. **Liquid Ooze** — drain heal inversion; small, niche but mechanically obvious.
10. **Defeatist + Slow Start + Truant** — straightforward stat halves / turn-skip
    flags; one PR each, all gated on simple state.
11. **Cotton Down** — `on_damaging_hit` arm with -1 Spe to all foes.
12. **Wind Rider + Wind Power** — needs wind-move tag table; ship together.
13. **Toxic Debris** — depends on Toxic Spikes (systems.md); blocked until hazard wired.
14. **Pickpocket / Symbiosis** — item-transfer plumbing; medium.
15. **Healer** — residual partner-cure; small.
16. **Color Change / Protean / Libero** — runtime type mutation; medium-hard.
17. **Cursed Body** — needs Disable volatile; blocked.
18. **Cute Charm** — needs Attract volatile; blocked.
19. **Mirror Armor** — stat-drop reflection; needs source plumbing; medium.
20. **Magic Bounce** — needs Magic Coat predicate (systems.md); blocked.
21. **Disguise** — Mimikyu signature; needs forme-change plumbing + damage-substitution
    path; medium-hard but high-leverage.
22. **Wonder Guard** — only-SE damage gate; single arm in damage path. Trivial but
    Shedinja-only — low priority.
23. **Poison Heal** — needs interplay with status-damage residual; small.
24. **Moody** — RNG-heavy residual; medium.
25. **Shadow Shield Mold-Breaker bypass** — patch on existing Multiscale arm (PR-240).
    Trivial.
26. **Rattled Intimidate trigger** — patch on PR-262. Trivial.
27. **Poison Touch** — 30% on-contact poison; small (mirror of Static).
28. **Sweet Veil** — partners sleep-immune; small but needs ally-side broadcast.
29. **Aromatherapy Veil (Aroma Veil)** — partners Taunt/Disable/Encore/Heal-Block-immune;
    blocked on those volatiles.
30. **Pastel Veil** — partners poison-immune; small.
31. **Pressure** — SHIPPED (PR-338); engine tracks PP, +1 per foe target.
32. **Frisk** — deferred (information-only, no battle effect).

---

## Trivial

### Big Pecks

- **slug:** `bigpecks`
- **PS:** `data/abilities.ts:435` (`onTryBoost` blocks Def drops from a foe)
- **Behavior:** Def cannot be lowered by an opposing source.
- **Hook:** `ability.rs::on_try_boost` (existing stat-drop guard block ~line 84)
- **Complexity:** trivial
- **Deps:** none — pattern-match arm
- **Batch with:** Keen Eye, Hyper Cutter (already shipped)

### Keen Eye (Acc-drop branch only)

- **slug:** `keeneye`
- **PS:** `data/abilities.ts:2215` (`onTryBoost` blocks Acc drops + ignores target Eva)
- **Behavior:** Acc cannot be lowered by foes. Acc-drop branch is trivial; Eva-ignore
  branch is small (needs accuracy-calc hook).
- **Hook:** `ability.rs::on_try_boost` + later `damage.rs` accuracy block for Eva-ignore.
- **Complexity:** trivial (Acc-drop) + small (Eva-ignore)
- **Deps:** none for Acc-drop
- **Batch with:** Big Pecks

### Vital Spirit (sleep-immune branch)

- **slug:** `vitalspirit`
- **PS:** `data/abilities.ts:5272` (`onUpdate` cures sleep; `onSetStatus` blocks slp)
- **Behavior:** Cannot be put to sleep. Intimidate-immune branch already shipped.
- **Hook:** `ability.rs::on_set_status`
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** Insomnia, Sweet Veil

### Insomnia

- **slug:** `insomnia`
- **PS:** `data/abilities.ts:2123` (identical to Vital Spirit sans Intimidate)
- **Behavior:** Cannot be put to sleep.
- **Hook:** `ability.rs::on_set_status`
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** Vital Spirit

### Limber

- **slug:** `limber`
- **PS:** `data/abilities.ts:2323` (`onSetStatus` blocks par; `onUpdate` cures par)
- **Behavior:** Cannot be paralyzed.
- **Hook:** `ability.rs::on_set_status`
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** Magma Armor, Immunity, Water Veil

### Magma Armor

- **slug:** `magmaarmor`
- **PS:** `data/abilities.ts:2456` (`onSetStatus` blocks frz)
- **Behavior:** Cannot be frozen. (Gen-9 freeze is rare — low leverage but trivial.)
- **Hook:** `ability.rs::on_set_status`
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** Limber batch

### Immunity

- **slug:** `immunity`
- **PS:** `data/abilities.ts:2051` (`onSetStatus` blocks psn/tox)
- **Behavior:** Cannot be poisoned.
- **Hook:** `ability.rs::on_set_status`
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** Limber batch

### Water Veil

- **slug:** `waterveil`
- **PS:** `data/abilities.ts:5386` (`onSetStatus` blocks brn)
- **Behavior:** Cannot be burned.
- **Hook:** `ability.rs::on_set_status`
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** Limber batch

### Oblivious (confusion + Attract + Taunt branches)

- **slug:** `oblivious`
- **PS:** `data/abilities.ts:2963` (blocks attract + taunt; Intimidate immune already
  shipped)
- **Behavior:** Immune to Attract and Taunt. Intimidate branch already in `ability.rs:89`.
- **Hook:** `ability.rs::on_set_volatile`
- **Complexity:** trivial (within scope of present systems); Attract/Taunt apply only if
  those volatiles exist.
- **Deps:** partial-blocked (Attract / Taunt volatiles per systems.md)
- **Batch with:** Own Tempo (confusion-immune)

### Shadow Shield (Mold-Breaker bypass patch)

- **slug:** `shadowshield`
- **PS:** `data/abilities.ts:4099` (Multiscale clone with `isBreakable: false`)
- **Behavior:** Multiscale that ignores Mold Breaker. Multiscale already shipped PR-240.
- **Hook:** `ability.rs` Multiscale arm — add slug + ignore Mold-Breaker suppression.
- **Complexity:** trivial
- **Deps:** partial-of: Multiscale (PR-240)
- **Batch with:** none

### Rattled (Intimidate trigger patch)

- **slug:** `rattled`
- **PS:** `data/abilities.ts:3726` (also fires on Intimidate received)
- **Behavior:** Bug/Ghost/Dark-hit branch already shipped (PR-262). Add Intimidate-source
  branch in the Intimidate dispatcher.
- **Hook:** `ability.rs` Intimidate dispatch — on target Rattled, +1 Spe instead of
  blocking.
- **Complexity:** trivial
- **Deps:** partial-of: Rattled (PR-262)
- **Batch with:** none

---

## Small

### Soundproof

- **slug:** `soundproof`
- **PS:** `data/abilities.ts:4391` (`onTryHit` blocks sound moves)
- **Behavior:** Immune to sound moves; sound table already exists (PR-51).
- **Hook:** `damage.rs` or `battle.rs` move-immunity check (early `onTryHit` equivalent).
- **Complexity:** small
- **Deps:** none
- **Batch with:** Bulletproof

### Bulletproof

- **slug:** `bulletproof`
- **PS:** `data/abilities.ts:470` (`onTryHit` blocks `flags.bullet`)
- **Behavior:** Immune to ballistic moves. Requires `flags.bullet` populated in MoveDef.
- **Hook:** `damage.rs` move-immunity check.
- **Complexity:** small (data-table addition for `flags.bullet`)
- **Deps:** none beyond move-flag wiring
- **Batch with:** Soundproof

### Damp

- **slug:** `damp`
- **PS:** `data/abilities.ts:801` (`onAnyTryMove` blocks explode-move list)
- **Behavior:** Prevents Explosion / Self-Destruct / Mind Blown / Misty Explosion field-wide.
- **Hook:** `battle.rs` pre-move resolve, scan field for Damp.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Friend Guard

- **slug:** `friendguard`
- **PS:** `data/abilities.ts:1488` (`onAnyModifyDamage` ×0.75 to allies)
- **Behavior:** Damage taken by ally ×0.75. Doubles-only.
- **Hook:** `damage.rs` post-type-eff defender mod scan (ally side).
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Telepathy

- **slug:** `telepathy`
- **PS:** `data/abilities.ts:4888` (`onTryHit` blocks ally damaging moves)
- **Behavior:** Immune to ally damaging moves (no friendly fire).
- **Hook:** `damage.rs` move-immunity check when source side == target side.
- **Complexity:** small
- **Deps:** none
- **Batch with:** Friend Guard (both doubles-only)

### Liquid Ooze

- **slug:** `liquidooze`
- **PS:** `data/abilities.ts:2357` (`onSourceTryHeal` inverts drain)
- **Behavior:** Drain moves used against the user damage the attacker instead of healing.
- **Hook:** `battle.rs` drain heal application — flip sign.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Cotton Down

- **slug:** `cottondown`
- **PS:** `data/abilities.ts:715` (`onDamagingHit` -1 Spe all foes)
- **Behavior:** On hit, -1 Spe to all opposing mons.
- **Hook:** `battle.rs::on_damaging_hit`
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Healer

- **slug:** `healer`
- **PS:** `data/abilities.ts:1772` (`onResidual` 30% cure ally)
- **Behavior:** End-of-turn 30% chance to cure partner's status.
- **Hook:** `battle.rs` residual phase.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Poison Heal

- **slug:** `poisonheal`
- **PS:** `data/abilities.ts:3286` (`onDamage` cancels psn damage; `onResidual` heals 1/8)
- **Behavior:** Poison heals 1/8 max HP per turn instead of damaging. Stacks with Toxic Orb.
- **Hook:** `battle.rs` residual psn/tox branch — replace damage with heal.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Defeatist

- **slug:** `defeatist`
- **PS:** `data/abilities.ts:873` (`onModifyAtk/SpA` ×0.5 if HP ≤ 50%)
- **Behavior:** Atk and SpA ×0.5 while user HP ≤ 50%.
- **Hook:** `battle.rs` stat-scale block.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Slow Start

- **slug:** `slowstart`
- **PS:** `data/abilities.ts:4266` (5-turn switch-in timer halves Atk + Spe)
- **Behavior:** First 5 turns after switch-in, Atk and Spe ×0.5.
- **Hook:** `battle.rs` stat-scale block + switch-in turn counter on the slot.
- **Complexity:** small
- **Deps:** none (turn counter is per-active-slot state)
- **Batch with:** Truant

### Truant

- **slug:** `truant`
- **PS:** `data/abilities.ts:5138` (`onBeforeMove` flips a flag, skips move every other turn)
- **Behavior:** Skip every other turn.
- **Hook:** `battle.rs` before-move resolve + per-slot flag.
- **Complexity:** small
- **Deps:** none
- **Batch with:** Slow Start

### Poison Touch

- **slug:** `poisontouch`
- **PS:** `data/abilities.ts:3325` (`onSourceDamagingHit` 30% poison on contact)
- **Behavior:** 30% chance on contact attack to poison target. Mirror of Static.
- **Hook:** `ability.rs::on_damaging_hit` (source side).
- **Complexity:** small
- **Deps:** none
- **Batch with:** none (could batch with Cotton Down)

### Pastel Veil

- **slug:** `pastelveil`
- **PS:** `data/abilities.ts:3144` (`onSetStatus` blocks psn; ally aura same)
- **Behavior:** User and partners cannot be poisoned.
- **Hook:** `ability.rs::on_set_status` + ally-broadcast.
- **Complexity:** small
- **Deps:** none
- **Batch with:** Sweet Veil

### Sweet Veil

- **slug:** `sweetveil`
- **PS:** `data/abilities.ts:4743` (`onAllySetStatus` blocks slp)
- **Behavior:** User and partners cannot be put to sleep.
- **Hook:** `ability.rs::on_set_status` + ally-broadcast.
- **Complexity:** small
- **Deps:** none
- **Batch with:** Pastel Veil

---

## Medium

### Cloud Nine / Air Lock

- **slug:** `cloudnine`, `airlock`
- **PS:** `data/abilities.ts:533` / `data/abilities.ts:90` (`onSwitchIn` suppresses weather
  in damage/effect lookups; weather state itself persists)
- **Behavior:** Damage formula and residual effects ignore weather while a holder is on
  the field. Weather state itself untouched.
- **Hook:** `damage.rs` weather-mod block + `battle.rs` weather residuals — gate on a
  field-scan helper `weather_suppressed()`.
- **Complexity:** medium (touches many sites)
- **Deps:** none
- **Batch with:** ship together (identical mechanic)

### Unaware

- **slug:** `unaware`
- **PS:** `data/abilities.ts:5171` (`onAnyModifyBoost` zeros foe boosts on offense and
  defense lookups)
- **Behavior:** User ignores opponent's stat-stage changes on both offense and defense.
- **Hook:** `damage.rs` stat-stage application — branch on attacker/defender ability.
- **Complexity:** medium
- **Deps:** none
- **Batch with:** none

### Pickpocket

- **slug:** `pickpocket`
- **PS:** `data/abilities.ts:3194` (`onAfterMoveSecondary` steals item on contact)
- **Behavior:** On contact hit received, steal attacker's item if user has none.
- **Hook:** `battle.rs::on_damaging_hit` (target side), item-transfer helper.
- **Complexity:** medium (needs item-transfer primitive + restricted-item list)
- **Deps:** none
- **Batch with:** Symbiosis

### Symbiosis

- **slug:** `symbiosis`
- **PS:** `data/abilities.ts:4794` (`onAllyAfterUseItem` passes user's item to ally)
- **Behavior:** When ally consumes its item, passes user's item to ally.
- **Hook:** `battle.rs` post-item-consume hook.
- **Complexity:** medium
- **Deps:** none beyond item-transfer primitive
- **Batch with:** Pickpocket

### Color Change

- **slug:** `colorchange`
- **PS:** `data/abilities.ts:553` (`onAfterMoveSecondary` sets user types = move type)
- **Behavior:** After being hit by a damaging move, user's type becomes the move's type.
- **Hook:** `battle.rs::on_damaging_hit` (target side) — mutate runtime type set.
- **Complexity:** medium (runtime type set is currently derived from species)
- **Deps:** runtime type-override slot on the active state.
- **Batch with:** Protean / Libero (shares type-mutation plumbing)

### Protean / Libero

- **slug:** `protean`, `libero`
- **PS:** `data/abilities.ts:3452` / `data/abilities.ts:2273` (`onPrepareHit` sets user
  type = move type; gen-9: only once per switch-in)
- **Behavior:** Each move used changes the user's type to that move's type, once per
  switch-in (gen-9 nerf).
- **Hook:** `battle.rs` pre-move type resolution + once-per-switch flag on the slot.
- **Complexity:** medium
- **Deps:** runtime type-override slot
- **Batch with:** Color Change

### Mirror Armor

- **slug:** `mirrorarmor`
- **PS:** `data/abilities.ts:2612` (`onTryBoost` reflects stat-drop back at source)
- **Behavior:** Reflects stat-lowering effects back at the source.
- **Hook:** `ability.rs::on_try_boost` — needs source-slot threading.
- **Complexity:** medium (source plumbing for boost effects)
- **Deps:** stat-drop source identity (some current arms drop it on the floor)
- **Batch with:** none

### Moody

- **slug:** `moody`
- **PS:** `data/abilities.ts:2656` (`onResidual` +2 random stat, -1 different stat)
- **Behavior:** End-of-turn: +2 to one random stat, -1 to another (Acc/Eva excluded
  gen-8+).
- **Hook:** `battle.rs` residual phase + RNG draws.
- **Complexity:** medium (RNG-heavy; needs deterministic ordering)
- **Deps:** none
- **Batch with:** none

### Wind Rider

- **slug:** `windrider`
- **PS:** `data/abilities.ts:5484` (`onTryHit` immune to wind moves + `onStart` Tailwind
  triggers +1 Atk + `onAnyTailwind` allies' Tailwind too)
- **Behavior:** Wind-move immunity + +1 Atk on wind-move hit received or when Tailwind is
  set on the user's side.
- **Hook:** `damage.rs` move-immunity + `ability.rs::on_damaging_hit` + Tailwind set hook.
- **Complexity:** medium (needs `flags.wind` table + Tailwind-set hook)
- **Deps:** wind-move flag table (shared with Wind Power)
- **Batch with:** Wind Power

### Wind Power

- **slug:** `windpower`
- **PS:** `data/abilities.ts:5466` (`onDamagingHit` sets `charge` volatile on wind hit;
  also `onAnyTailwind`)
- **Behavior:** On wind-move hit received (or Tailwind set), sets Charged volatile that
  doubles the next Electric move's BP.
- **Hook:** `ability.rs::on_damaging_hit` + Charge volatile interaction in `damage.rs`.
- **Complexity:** medium (Charged volatile needs to exist and apply BP doubling)
- **Deps:** Charged volatile model
- **Batch with:** Wind Rider

---

## Hard

### Magic Bounce

- **slug:** `magicbounce`
- **PS:** `data/abilities.ts:2392` (`onTryHit` reflects status moves)
- **Behavior:** Reflects status moves back at the user. Hatterene signature.
- **Hook:** `battle.rs` target resolution — replace target with attacker if move category
  == status and target has Magic Bounce.
- **Complexity:** hard (needs same predicate as Magic Coat — systems.md flag for
  reflectable moves)
- **Deps:** blocked: Magic-Coat reflectable-move predicate
- **Batch with:** none

### Disguise

- **slug:** `disguise`
- **PS:** `data/abilities.ts:960` (`onDamage` substitutes Mimikyu damage with 1/8 max chip
  + form-change to Busted)
- **Behavior:** First hit reduced to 1/8 max HP chip; move's damage negated; form changes
  to Busted.
- **Hook:** `damage.rs` final-damage substitution + species/forme swap on first hit.
- **Complexity:** hard (needs forme-change plumbing + per-mon "disguise broken" flag)
- **Deps:** forme-change primitive
- **Batch with:** none

### Cursed Body

- **slug:** `cursedbody`
- **PS:** `data/abilities.ts:774` (`onDamagingHit` 30% Disable on attacker's move)
- **Behavior:** 30% chance on hit to disable attacker's used move for 4 turns.
- **Hook:** `ability.rs::on_damaging_hit` once Disable volatile exists.
- **Complexity:** hard (Disable not modelled)
- **Deps:** blocked: Disable volatile (systems.md)
- **Batch with:** none

### Cute Charm

- **slug:** `cutecharm`
- **PS:** `data/abilities.ts:788` (`onDamagingHit` 30% Attract on contact attacker)
- **Behavior:** 30% chance on contact hit received to infatuate attacker.
- **Hook:** `ability.rs::on_damaging_hit` once Attract volatile exists.
- **Complexity:** hard (Attract not modelled)
- **Deps:** blocked: Attract volatile
- **Batch with:** none

### Toxic Debris

- **slug:** `toxicdebris`
- **PS:** `data/abilities.ts:5061` (`onDamagingHit` on physical, sets Toxic Spikes on
  foe side)
- **Behavior:** On taking a physical hit, lays a layer of Toxic Spikes opposing side.
- **Hook:** `ability.rs::on_damaging_hit` (target side) once Toxic Spikes hazard exists.
- **Complexity:** hard (Toxic Spikes hazard not modelled)
- **Deps:** blocked: Toxic Spikes
- **Batch with:** none

### Wonder Guard

- **slug:** `wonderguard`
- **PS:** `data/abilities.ts:5510` (`onTryHit` blocks moves that aren't super-effective)
- **Behavior:** Only super-effective damaging moves can damage the user. Shedinja-only.
- **Hook:** `damage.rs` move-immunity — gate on type-eff > 1.
- **Complexity:** hard (interacts with Mold Breaker, Scrappy, Tinted Lens, etc.)
- **Deps:** none structural, but interaction matrix is wide.
- **Batch with:** none

### Aroma Veil

- **slug:** `aromaveil`
- **PS:** `data/abilities.ts:234` (`onAllyTryAddVolatile` blocks Attract/Disable/Encore/
  Heal Block/Taunt/Torment on user + partners)
- **Behavior:** User and partners immune to Attract / Disable / Encore / Heal Block /
  Taunt / Torment.
- **Hook:** `ability.rs::on_set_volatile` + ally-broadcast.
- **Complexity:** hard (depends on all six volatiles existing)
- **Deps:** blocked: Attract / Disable / Encore / Heal Block / Taunt / Torment volatiles
- **Batch with:** none

### Inner Focus (flinch-immune branch)

- **slug:** `innerfocus`
- **PS:** `data/abilities.ts:2108` (`onFlinch` blocks)
- **Behavior:** Cannot be flinched. Intimidate-immune already shipped (ability.rs:84).
- **Hook:** `battle.rs` flinch application — guard on ability.
- **Complexity:** hard if flinch isn't modelled cleanly; trivial if it is.
- **Deps:** flinch volatile model (audit needed)
- **Batch with:** Own Tempo (confusion branch — same shape)

### Own Tempo (confusion-immune branch)

- **slug:** `owntempo`
- **PS:** `data/abilities.ts:3099` (`onUpdate` cures confusion; `onTryAddVolatile` blocks)
- **Behavior:** Cannot be confused. Intimidate-immune already shipped (ability.rs:89).
- **Hook:** `ability.rs::on_set_volatile`.
- **Complexity:** hard if confusion isn't modelled; small otherwise.
- **Deps:** confusion volatile (audit needed)
- **Batch with:** Inner Focus

---

## Shipped / Deferred (gen-9 N/A or no battle effect)

### Pressure — SHIPPED (PR-338)

- **slug:** `pressure`
- **PS:** `data/abilities.ts:3392` (`onDeductPP` returns 1 for a non-ally
  source) applied via `sim/battle-actions.ts:467-484`.
- **Behavior:** A foe move that targets the Pressure holder costs +1 PP (2
  total). In doubles a spread move sums +1 per Pressure foe it targets.
- **Shipped:** Engine DOES track PP (populated at team-build, decremented on
  use, gates selection at 0). `pressure_extra_pp` in `battle.rs` counts foe
  targets holding active Pressure and is added at every PP-decrement site.
  The switch-in `-ability` message is cosmetic and skipped.

### Frisk

- **slug:** `frisk`
- **PS:** `data/abilities.ts:1500` (reveals foe item — info only)
- **Behavior:** Information-only reveal. No battle effect.
- **Deferred:** Engine does not surface chat; pure no-op.
