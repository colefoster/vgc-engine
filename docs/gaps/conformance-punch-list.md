# Conformance punch list (2026-06-24)

## 100-battle stable-ID sweep (2026-06-24, later) — SHIPPED + QUEUE

Ran a 100-battle batch via the new stable-ID tooling
(`tools/conformance-batch/`, regenerate into `/tmp/confbig`). Clean count
22 → 24 after the fixes below. Triaged turn-1 / 0-unmatched divergences.

SHIPPED from this sweep: Cotton Guard +3 / Shelter +2 Def; Apple Acid SpD-1 /
Night Daze acc-1 / Muddy Water acc 30% (was 100%); Haze (clear all boosts);
Overgrow/Blaze/Torrent/Swarm pinch ×1.5.

SHIPPED from the queue (2026-06-24): **Weak Armor** (−1 Def/+2 Spe on physical
hit), **Gluttony** (≤1/4-HP berries at ≤1/2), **Knock Off vs own mega stone**
(no ×1.5, no removal — out_179459f0d9 turn 1 now matches).

QUEUE — REMAINING (heavier; need new volatile machinery or plumbing. Grep-verify
each — the audit false-positived on Last Respects, which IS impl via
DamageContext):
- **Synchronize** (8 teams) — reflect brn/par/psn/tox onto the inflictor.
  Needs `source_slot` threaded through `try_set_status_from` (only carries
  source_side today) so the reflect targets the right foe in doubles.
- **Yawn** (5) — drowsy volatile → sleep next end-of-turn. New volatile.
- **Disable** (6) — lock the target's last move 4 turns. New volatile +
  move-selection gating.
- **Destiny Bond** (6) — KO the attacker if the user faints to it. Needs
  faint-source tracking.
- **Beat Up** — fully broken (base_power 0, multihit 0/0 → ~0 dmg); needs
  per-conscious-party-member hits with each member's Atk.
- Terrain: Grassy / Psychic / Misty unimplemented (only Electric).
- Lower-usage: Psych Up, Soak, Recycle, Gastro Acid, No Retreat, Contrary,
  Forecast, Frisk, Leaf Guard, Magician, Infiltrator, Corrosion.
- Remaining 0-unmatched turn-1 gaps: out_f55fe99993 (−13 dmg), boost gaps
  out_b018656fa6 / out_d36156026b.

---

Real engine divergences surfaced by the Champions PS-conformance harness on a
50-battle Reg M-B doubles batch (`/tmp/conf-batch`, teams from
`~/Dev/mimikyu/data/generated_teams/regmb_random_100`). Each entry is
`unmatched_draws == 0` (RNG fully keyed → genuine mechanic bug, not a draw
desync). Verify any fix by re-running `cargo run -p vgc-engine-conformance --
/tmp/conf-batch/out_<NN>.json` to CLEAN.

## SHIPPED (2026-06-24)
- ✅ self-stat-drops only when the move connects (Protect/miss/immune) — out_03, out_36 (e62f6b9)
- ✅ Headlong Rush self-drops Def/SpD — out_09 (6d8d0f0)
- ✅ Shell Smash implemented (+White Herb/Eject Pack) — out_02 (4887a61)
- ✅ Minimize / Double Team raise evasion — out_26 (ff391ee)
- ✅ driver: per-target keying for spread secondaries (harness fidelity) — out_13 unmasked (a122565)
- ✅ Champions pokeRound/chainModify damage rounding (base-power chain + weather/STAB) — out_41 T1 (7b3438f)
- ✅ Knock Off ×1.5 at base-power stage, not final damage — out_44, out_30 (13a2b80)
- ✅ **Iron Head flinch 20% in Champions (was 30%)** — out_23 (4caade9). NOT a
  damage bug: the subagent's "Excadrill-Mega High Horsepower deals 0 damage" was
  a misdiagnosis — a roll of 20 flinched the foe's attacker at 30% but not at
  Champions' 20%, so the attacker never moved. The move/damage code was always
  correct. Lesson: trace before "fixing"; the harness shines at Champions data
  deltas (cf. Make It Rain -2).

## SHIPPED (cont.)
- ✅ **Curse implemented** (both branches). Non-Ghost user → self +1 Atk/+1
  Def/−1 Spe, no cost. Ghost user → directDamage floor(maxhp/2) to self +
  `curse` volatile on a foe (fails if already cursed; bypasses Sub; ignores
  Protect — no protect flag), target loses 1/4 max HP each end of turn
  (residual order 12, Magic Guard blocks the chip). 3 unit tests. NOTE: the
  old out_13 entry below referenced the *previous* batch (Trevenant Curse);
  the regenerated out_13 never uses Curse — its residual turn-2 p2b gap
  (engine 46 vs ps 145) is an **Excadrill Iron Head damage divergence**
  (trace-first; see memory `match-real-champions`), not a Curse bug.

## SHIPPED (cont.)
- ✅ **Spiky Shield family contact-punish** — out_24 turn-1 now CLEAN. Tag the
  Protect volatile payload with a `protect_variant` code; on a fully-blocked
  CONTACT move, `apply_protect_punish` fires the attacker-side effect: Spiky
  Shield 1/8 chip (Magic-Guard-blocked), Baneful Bunker poison, King's Shield
  −1 Atk, Obstruct −2 Def, Burning Bulwark burn, Silk Trap −1 Spe. Protector
  is the boost/status source (Safeguard/Defiant/Clear Body resolve right);
  contact via `move_makes_contact` so Protective Pads/Punching Glove suppress.
  6 unit tests. NOTE: out_24's residual turn-2 p2a atk −1 gap is a separate
  **Incineroar Intimidate-on-switch-in ordering** bug (both sides double-switch
  the same turn; engine misses the Intimidate on the freshly-switched foe).
## SHIPPED (cont.)
- ✅ **Pre-turn switch-in ordering interleaved by leaving mon's Speed** —
  out_24 turn 2 now CLEAN (advances to a turn-4 HP divergence). `apply_switches`
  used to resolve ALL of P1's switches + onStart hooks, then all of P2's — so an
  Intimidator switching in opposite a double-switching foe intimidated the
  OUTGOING foes. New `apply_pre_turn_switches` gathers both sides' voluntary
  switches, sorts by the leaving mon's effective Speed (fastest first), and runs
  swap → hazards → ability/item onStart for each before the next — matching PS's
  speed-sorted switch actions. Also fixes weather/terrain-setter "fastest wins"
  on simultaneous switch-ins. Heap-free (≤4 fixed buffer, sort_unstable). Test:
  `intimidate_on_double_switch_hits_only_the_foe_already_in`.

- **Burn residual on a mon that switched in that turn** — old-batch out_35
  (Polteageist Heat Wave burn DOT). NOT reproduced in the regenerated batch (new
  out_35 is an Intimidate/cascade battle). The status-DOT residual loop iterates
  active slots with no "skip just-switched" logic, so this is likely stale —
  needs a fresh repro before any fix.

## SHIPPED (cont.)
- ✅ **ModifyDamage chain accumulated into one pokeRound** — out_37 (the +3
  rounding diverger) now CLEAN. `calculate_damage`'s final modifiers (Multiscale,
  Filter, Tinted Lens, Ice Scales, Punk Rock, Fluffy, screens) were each applied
  as their own truncating op; PS chains them ALL into one Q12 modifier and
  applies a single pokeRound. Now accumulated into `dmg_mod` via `chain_modify`
  and applied once via `apply_modifier`; burn ÷2 moved ahead of the chain to
  match PS's order. 810 tests green (calc-oracle exactness preserved).
  - REMAINING for a follow-up: Friend Guard (×0.75 ally, **unimplemented**) and
    Expert Belt (×1.2) should join this chain, and **Life Orb's ×1.3 is applied
    in battle.rs:4960 OUTSIDE the chain** (a second pokeRound — double-rounds).
    Moving it in needs a DamageContext flag.
  - out_32 is NOT a rounding bug: it carries an unmatched draw (keying cascade),
    and its "Giga Drain −1" label was the stale-index trap (different battle now,
    engine 112 vs ps 122 + 1 unmatched).
- **out_46** Scolipede-Mega takes too little from Dual Wingbeat (mega Def stat
  delta?) — needs a trace (could be another Champions data delta like Iron Head).
- **out_05** Galar Slowbro ~38 low (partner-missed spread Blizzard + Regenerator)
  — `is_spread` may require >1 target HIT vs PS keeping ×0.75 on a missed
  partner; needs trace. **out_19** (T4) and **out_23 T5** uninvestigated.

## 2026-06-24 — new LEGAL-team batch (regmb_random_100, regenerated)

Re-ran the 50-battle batch on the regenerated legal teams (proper mega-stone
notation, valid sets). 14 clean / 12 real / 19 cascades. Findings:

- ✅ **Mega base-stat audit CLEAN** — all 100 mega formes' base stats match PS's
  pokedex.ts exactly; the one localdex discrepancy (Starmie-Mega atk 140 vs 100)
  is already corrected by `MegaFix starmiemega atk: 100`. So out_46 is NOT a
  Scolipede-Mega stat bug (that hypothesis is dead).
- ✅ **Skill Swap blocked by Protect** — out_45 (7b02a6f).
- ✅ **foe-stat-lowering STATUS moves implemented** (a544cc1) — were ALL
  unimplemented (fell through `resolve_status_move_inner`'s default arm to "no
  effect"; Growl/Tickle left the foe at 0 boosts). Found via out_46 (Emolga
  Tickle on Froslass — PS −1 atk/−1 def, engine 0). New `foe_debuff_moves` table
  (21 moves: Growl, Leer, Tail Whip, Tickle, Charm, Feather Dance, Baby-Doll
  Eyes, Play Nice, Confide, Eerie Impulse, Fake Tears, Metal Sound, Screech,
  Scary Face, Cotton Spore, String Shot, Sand Attack, Smokescreen, Kinesis,
  Flash, Sweet Scent, Noble Roar, Tearful Look) routed through `apply_boosts`
  (Mist/Mirror Armor/Clear Body) with full call-site gating: accuracy, Magic
  Bounce (upstream), Protect flag, Substitute (unless sound/bypasssub), per-stat
  Clear Body/Hyper Cutter, White Herb / Eject Pack / Defiant. allAdjacentFoes
  moves hit both foes in doubles. Captivate (gender-gated) + Defog (side
  effects) excluded. out_46 Tickle now matches.
- ✅ **Liquid Voice implemented** — out_25 turn 1 now CLEAN (advances to a
  turn-3 cascade). Primarina's Hyper Voice was treated as Normal (no STAB, 90)
  vs PS Water (Primarina Water STAB ×1.5 → 135). Sound moves now retype to
  Water for a Liquid Voice attacker in BOTH `damage.rs::move_type_in_ctx` and
  the `calculate_damage` type binding (next to the -ate hook; no BP boost,
  gated on the sound flag not Normal-type). Test
  `liquid_voice_retypes_sound_moves_to_water_with_stab_no_bp_boost`.
- ✅ **Trop Kick −1 Atk secondary implemented** — out_16 turn 1 now CLEAN
  (advances to turn 3). Was REAL, not false: re-capturing the PS golden via the
  new stable-ID tool reproduced 107, so PS is authoritative. Root cause: Trop
  Kick (100% atk −1, like Lunge) was missing from `stat_drop_secondary`, so
  Araquanid — Trop Kicked first by the faster Tsareena — kept full Atk and its
  follow-up Leech Life dealt 74 vs PS 50 (74 × 2/3 ≈ 50). **Lesson:** the trace
  subagent mis-called this "false" because its @smogon/calc check used 0 boosts
  and missed Trop Kick's −1; the re-capture (PS = 107 reproducibly) caught it.
  Trace before trusting "engine matches calc, PS is wrong." Test
  `trop_kick_lowers_target_atk_by_one`.
- Untriaged reals: out_12 (Ditto/Imposter + switches), out_06 (T2 freeze
  secondary not applied), out_34 (T2 boosts), deeper-turn out_13/29/01/48.

## Remaining cascades (harness keying gaps, mostly not engine bugs)
13 battles still carry `unmatched > 0` at turns 2–5 (out_00, out_08, out_14,
out_18, out_20, out_21, out_22, out_27, out_34, out_38, out_39, out_42, out_48).
The big systematic one (spread-secondary target) is fixed; the tail is
per-battle (out_20 ability fairyaura/blaze, out_48 paralysis, deeper-turn draw
attribution). Lower value than the real bugs above.
