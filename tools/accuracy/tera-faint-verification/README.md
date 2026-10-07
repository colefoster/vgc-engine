# Tera ends at faint: PS verification inputs

PS a5df8274 faintMessages (sim/battle.ts:2558-2574) restores a
formeRegression origin, runs clearVolatile(false), marks the mon fainted and at
:2568 `delete pokemon.terastallized`. The side's one-Tera permit stays spent.
Standard captures (protocol, HP-only state) can't show the flag, so each job
is run twice: once through the normal capture, once through a read-only probe.

- `jobs.jsonl`: two predeclared jobs (fixed sodium seeds, no tuning).
  - `tera-faint-resist-and-ko`: the `tera-before-moves-resist-and-ko` job
    unchanged (same teams, choices and seed 0301). p2b Slowbro Terastallizes
    to Grass, then the faster Chandelure's Flamethrower KOs it before it moves.
    p1a Snorlax (Tera Steel) is the live control.
  - `tera-faint-partner-lives`: the same leads with no bench (seed 0311).
    Slowbro is KO'd while Weavile lives on, and nothing replaces it. On turn
    2, Weavile's Ice Shard hits the still-Terastallized Steel Snorlax.
- `probe.js`: plays the jobs on a plain PS `Battle` with no RNG patching.
  After each turn it dumps every team member's species, hp, fainted,
  terastallized and canTerastallize, plus any side Tera field.

## Measured (pinned a5df8274 dist, offline)

`ps-state.jsonl` holds the probe's output lines verbatim, as recorded by
the manager run. Do not re-record them.

- Both jobs ran without errors.
- Every observation (job 1 turn 1, job 2 turns 1 and 2) shows:
  - Slowbro: `hp 0`, `fainted`, `terastallized: null`;
  - live Snorlax: `terastallized: "Steel"`;
  - every team member: `canTerastallize: null`.
- The normal `ps-battle.js` captures of both jobs also ran without errors:
  1 and 2 turns, `lastTurn` 2 and 3.

## Limits

- The PS side object has no Tera-named field (`tera: {}` throughout), so
  the probe observes no side-level state. The spent permit is witnessed only
  by every mon's `canTerastallize: null`.
- The strict golden `tera-before-moves-resist-and-ko` (crates/vgc-engine-golden)
  stays the regression for event order, RNG and HP. It is HP-only, so it
  can't observe the flag. This probe and the core tests
  (`tera_happens_even_when_the_user_is_knocked_out_first` and the
  `tera_faint_*` tests in battle.rs, plus
  `step_chance_tera_faint_clears_tera_only_on_fainted_branches` in chance.rs)
  cover it.
## Engine replay of the normal captures (manager-measured, at 1b5ed70)

- Both captures match all 3 state turns (1 + 2) in both keyed and seeded
  modes. Neither mode had an engine error, a PS error, a missing draw or a
  repair.
- Seeded: every count is zero, and there is no first-draw divergence.
- Keyed: **each** capture has 1 leftover draw:
  `t1 / Secondary / flamethrower / p1b>p2b`. The keyed run is not
  leftover-free.
- `tera-faint-resist-and-ko`, the job with a bench, had 1 choice fallback
  for its replacement switch; `tera-faint-partner-lives` had none.
- The existing seeded fixture of the same KO already covers the engine
  outcome. `ps-state.jsonl` is the direct state reference for the flag.
- Worker-wide at the same point:
  - workspace tests: 1603 default / 1646 seeded / 1313 chance, 0 failed
    (32 ignored in the workspace runs, 2 in the chance run);
  - 500-battle allocation probe: complete, 9887 steps, 0 cap, 0 alloc.

```sh
export PS_DIST=<pinned a5df8274 build>/dist/sim
node tools/accuracy/ps-battle.js tools/accuracy/tera-faint-verification/jobs.jsonl /tmp/tera-faint-b --start 0 --count 1 --max-turns 1
node tools/accuracy/ps-battle.js tools/accuracy/tera-faint-verification/jobs.jsonl /tmp/tera-faint-b --start 1 --count 1 --max-turns 2
node tools/accuracy/tera-faint-verification/probe.js tools/accuracy/tera-faint-verification/jobs.jsonl > /tmp/tera-faint-probe.jsonl
```
