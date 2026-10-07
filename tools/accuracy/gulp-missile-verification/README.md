# Gulp Missile: PS captures

**Purpose:** mechanic regression evidence for Cramorant's Gulp Missile in
standard Gen 9. This is not a holdout or an acceptance gate.

## Sources (a5df8274)
- `data/abilities.ts:1745-1769` (`gulpmissile`).
- `data/moves.ts:3760-3767` (Dive's `onTryMove`): the forme is loaded on the
  initial charge only, before ChargeMove, and not for a transformed user.
- There is no Champions override, and Cramorant is absent from both Reg M-B
  and Reg M-C.
- Human reference:
  <https://bulbapedia.bulbagarden.net/wiki/Gulp_Missile_(Ability)>.

## Status
All five frozen runs were captured clean by the manager on pinned PS: `ok`,
no errors, no faints. Seeds and actions are unchanged from what was
declared before capture. The raw captures stay in the manager's evidence.

| Run | Installed as | Turns |
|---|---|---|
| Surf at full HP into Splash → Gulping, then Tackle → response (Snorlax 193 → 134, −1 Def), revert | `crates/vgc-engine-golden/goldens/gulp-surf-gulping-response.{input,ps}.json` | 2 |
| Super Fang 176 → 88 (exact half), then Surf → Gorging, then Tackle → 129 and paralysis, revert | `…/gulp-superfang-half-gorging-response.{input,ps}.json` | 3 |
| Dive charge loads Gulping; Snorlax's Surf hits the diver (177 → 151) with no response | `…/gulp-dive-surf-semi-invulnerable.{input,ps}.json` | 1 |
| Imposter Ditto: the copy's Surf loads nothing (PS logs the handler suppressed); the real Cramorant's Surf loads | `…/gulp-transformed-copy-guard.{input,ps}.json` | 2 |
| Full-info job `gulp-superfang-gorging-fullinfo-01` (`jobs.jsonl`, seed `sodium,…0d01`) | `tools/accuracy/repros/battles/gulp-superfang-gorging-fullinfo-01.json` | 3 |

- **Inputs:** the golden inputs moved here → `goldens/` unchanged, once
  paired with actual PS output.
- **Job:** `jobs.jsonl` is kept.
- **Speed ties:** the transformed-copy run ties on speed, but only with
  Splash, so move order doesn't matter.

## Review additions (public tests, `crates/vgc-engine-core/tests/gulp_missile.rs`)

**Synchronize, exact holder slot.** The Gorging paralysis takes the
holder as its source, so an attacker's Synchronize reflects back onto that
holder.
- Covered in singles, and in doubles with the holder at p1b; the partner at
  p1a is not paralyzed.
- Implemented via `try_set_status_from_src` with the holder's side and slot.

**White Herb control.** Close Combat into Gulping leaves Def and SpD both
at 0, with White Herb consumed once after the whole move.
- It passes on the existing deferred helper; no source change.

**Set-forme origin.** PS takes `baseSpecies` from `set.species`
(`sim/pokemon.ts:322`), and `clearVolatile` returns to it (`:1559`).
- On switch-out and back, a Cramorant set comes back Cramorant, while a
  direct Gulping set and a legacy unknown origin come back Gulping.
- A direct Gulping set that responded and reverted also returns to
  Gulping.

**Canonical hash.** The 44th field, `forme_origin`, and the bench sort key
now carry the Cramorant set origin, for non-transformed mons only.
- Swapping two benched mons with different origins hashes the same, while
  different origins hash differently.
- The absolute hash of states holding such a Cramorant changed.

**Extra captures not installed.** Two extra full-info review captures
(Synchronize, and White Herb with Close Combat) were not installed:
reading them was refused (EPERM). The public tests above are the durable
coverage for those cases.

## Limits
- **Not captured:** the attacker-at-zero-HP and attacker-inactive gates
  have no fixed capture.
- **Magic Guard:** covered only by an engine test, which follows PS
  `this.damage` semantics and the Bulbapedia note.
- **Spread Surf:** not captured. The legacy flat replay rolls spread
  accuracy per target, unlike PS.
