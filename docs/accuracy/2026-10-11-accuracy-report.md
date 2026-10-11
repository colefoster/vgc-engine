# vgc-engine vs Showdown: accuracy after the draw-order rounds (2026-10-11)

**Verdict: fit for local self-play.** The engine now matches PS almost exactly:

- On the held-out samples, 99.7–99.9% of battles replay identically to PS on PS's own PRNG, and 99.7–99.8% under forced RNG.
- On the sealed sample D, run once at the end, the figures are 97.9% on PS's PRNG and 98.9% under forced RNG. Per-turn agreement is 99.76% and 99.88%.
- The remaining gap is about 1–2% of battles. The cause is mostly niche mechanics, not the RNG, so self-play data will carry a small, rare bias. It won't produce systematically wrong play.

Main is at `951cf9e`. `cargo test --workspace --offline` exits 0. So do the ps-rng test run and `tools/audit-residual-index/audit.sh`.

## Numbers

These are fully clean battles out of 1,300 per sample: every compared turn matches PS, to the end. Brackets give the Wilson 95% CI.

| sample | role | keyed (forced RNG) at eadd622 | keyed at 951cf9e | seeded (PS PRNG) at eadd622 | seeded at 951cf9e |
|---|---|---|---|---|---|
| A | in-sample | 1299 (99.9%) | 1299 (99.9%) [99.6–100] | 1237 (95.2%) | **1299 (99.9%)** [99.6–100] |
| B | held-out, triaged | 1298 (99.8%) | 1298 (99.8%) [99.4–100] | 1201 (92.4%) | **1296 (99.7%)** [99.2–99.9] |
| C | held-out, triaged | 1290 (99.2%) | 1296 (99.7%) [99.2–99.9] | 1211 (93.2%) | **1299 (99.9%)** [99.6–100] |
| D | **sealed**, run once at 951cf9e | — | **1286 (98.9%)** [98.2–99.4] | — | **1273 (97.9%)** [97.0–98.6] |

Per-turn agreement on D, given an agreeing start:

- keyed: 11279 of 11293 turns (99.88%)
- seeded: 11190 of 11217 turns (99.76%)

The first figure in the September proof was 16.5% fully clean under keyed. The October rounds raised it to 98.8% (B, round 2) before this work.

**Read D as the honest number.** B and C were held out of round 1–3 tuning, but this round triaged their divergences directly. So their 99.7–99.9% is partly fitted. D is about one point lower on keyed and two points lower on seeded, which is the expected out-of-sample gap.

## What this round changed

There are 39 fix commits since `eadd622`. They were merged from six worker branches: WP9–WP15, plus WP12 for shuffle ties. Each commit cites the PS file:line. Every merge passed:

- workspace tests
- ps-rng tests
- the residual audit
- an A/B/C re-measure in which no battle that was clean before diverged

**Draw-order fixes.** These only affect `--features ps-rng`; the engine now makes PS's draws in PS's order. This was the bulk of the gain:

- The crit is rolled even against Shell Armor / Battle Armor.
- The multi-hit count is drawn before the first crit.
- Secondaries are rolled for KO'd targets: the fangs, Triple Arrows and Mortal Spin.
- Stone Axe and Ceaseless Edge roll their empty secondary.
- Eerie Spell, Sparkling Aria, Spirit Shackle and Syrup Bomb roll their 100% secondaries.
- A failed status move skips the hit-loop Updates.
- Future Sight and Doom Desire draws are fixed.
- The confusion gate roll and Substitute damage rolls are ordered correctly in the spread window.
- Random targets are fixed for Struggle and Outrage, Instruct, Champions Encore requeues, scripted moves and Outrage locks.
- Forewarn now draws its sample.
- Update and re-sort ties are fixed for Revival Blessing, beforeTurnMove, priorityChargeMove and the AfterMove handlers.
- Residual-sort membership is fixed: DisableMove, `twoturnmove`, charge and Counter / Mirror Coat volatiles, and a mon fainted on an earlier turn.
- Tie membership is fixed for Steel Roller and Ice Spinner TerrainChange and the Accuracy-event handlers.
- Transform's cached speed is fixed.
- The commit sort keys a charging mon on its locked move.

**Real mechanic fixes, in every build.** Cole expected none to be left; this round found about 20:

- Alluring Voice's confusion was not implemented.
- Powder immunity is now checked before accuracy.
- A Flash Fire holder behind Protect no longer makes the move sure-hit.
- Spread stat-drop status moves roll accuracy per target.
- A recharge turn sorts at priority 0.
- A partnerless Ally Switch now arms the chain.
- A status move with no living foe makes no accuracy roll.
- A switched-out user's cancelled move no longer re-sorts.
- Helping Hand fails on a partner that has already moved.
- Protect blocks only moves with the protect flag, so Roar now goes through.
- Champions Freeze-Dry has no secondary.
- A mid-turn pivot pick no longer overwrites the queued move, so Sucker Punch and Pursuit now see it.
- A move's own onTryHit runs after its accuracy roll.
- Prankster reads the current ability.
- Trick-or-Treat and Forest's Curse roll accuracy.
- Leech Seed's Grass immunity is checked before accuracy, on live types.
- Fairy Aura and Dark Aura are read at move start.
- Protect stops OHKO moves before the accuracy roll.
- Mimicry is implemented, and status immunity now uses live types.
- Quash fails when its target has no queued action.
- Speed Swap ends on faint, because a fainted mon's stats revert.
- Speed modifiers chain once, then paralysis halves.
- A choice lock ends when the Choice item is gone.

## What's left

**A/B/C seeded: 6 battles diverge.**

| id | cause | status |
|---|---|---|
| B fa1b9b5faa | PS log truncated mid-turn; keyed diverges too | irreducible |
| B 3160717f56 | Leech Seed on a mon that Forest's Curse made Grass; the engine has no third added type | needs feature |
| B 7e0bb8492b | Flip Turn + Eject Button switch both sides at once; PS runs one batched runSwitch, the engine runs one per side | needs `apply_self_switches` restructure |
| A 90494adef8 | Red Card / Emergency Exit / Imposter chain misses one runAction Update | not done |
| B e06911a6ab | Guard Split's hit loop never draws its loop-end Update | not done |
| C e6ec841558 | Encore volatile missing from the turn-7 residual handler list | cause unknown |

**D (sealed; inspected only to classify, never tuned on).** 27 battles diverge on the seeded run:

- 18 diverge first on a draw, spread thinly across 11 kinds. The largest group is 3 battles, an `onSourceDamagingHit` randomChance at `data/abilities.js:3320`, which suggests a contact-ability proc.
- 9 diverge on state alone.

Under forced RNG, 14 D battles diverge:

- 11 deterministic mechanics, no effect in more than 3 battles: Grassy Terrain, Life Orb, Make It Rain, Darkest Lariat, Wave Crash
- 2 RNG plumbing
- 1 Illusion, which is hidden info

**Known follow-ups:**

- Trick-or-Treat and Forest's Curse don't add a third type.
- Quick Feet, Unburden, Surge Surfer, Slow Start and weather speed abilities in `order.rs` read the base `ability_id`, the same bug fixed here for Prankster.
- The Eerie Spell PP cut, Sparkling Aria's burn cure, Spirit Shackle's trap and Syrup Bomb are rolled but their effects aren't modelled.
- One battle each in A and B, `6ba05774df` and `6661c345bb`, stay state-clean under forced RNG. Each now leaves one Tiebreak key unconsumed after the paralysis speed-chain fix, so the "strict" counts are A 1292 and B 1287.

## Self-play fitness

- **Use it.** About 98–99% of unseen real Reg M-C battles replay turn-for-turn identically to PS given the same choices and random outcomes. Per-turn agreement is ≥99.76%.
- The PRNG is bit-exact, and the draw order now matches PS through the whole battle in about 98% of unseen games. For any given team pair, outcome distributions follow PS's.
- **Don't treat it as the final evaluator.** Residual errors cluster in rare interactions: added types, batched double switches, contact-ability procs and some terrain / Life Orb edge cases. A policy trained only on engine self-play could learn to exploit one of these if it sits in a common line. Keep a periodic PS head-to-head or the keyed differential on fresh replays as the check.
- **Re-measure on new samples, not D**, after any engine change. D has now been opened once.

## Reproduce

```sh
W=/Users/cole/vgc-acc-2026-10-10
/Users/cole/vgc-acc-2026-10-10/pm2/measure.sh <tag> /Users/cole/Dev/vgc-engine   # A/B/C keyed + seeded, ~20 s
python3 $W/int/stats.py $W/pm2/<tag>                                            # table rows with CIs
```

PS reference: smogon/pokemon-showdown `a5df8274`. Samples A–D are 1,300 Reg M-C ladder logs each, from `tools/accuracy/recon.js` + `ps-battle.js`. Pipeline details are in `tools/accuracy/README.md`. Per-battle rows for every measurement point (`m0` = eadd622 … `m7` = 951cf9e) are in `$W/pm2/`.
