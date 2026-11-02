# PR-I — Action-Independence Factoring for Doubles Outcome Enumeration

**Status:** Design / scoping.
**Author:** scoping agent, 2026-06-29.
**Crate:** `vgc-solver` (with possible thin engine helper).
**Prereqs:** none (the seam already exists at `enumerate_outcomes`).

---

## 1. Motivation and headline number

`vgc_solver::enumerate_outcomes` (crates/vgc-solver/src/lib.rs:169) builds the
outcome frontier for one `(Battle, joint_choice)` by cross-producting every
recorded RNG draw site. In singles that's typically 16 damage rolls × maybe a
crit branch × maybe an accuracy site — tens to low hundreds of `step()` calls
per cell.

In **doubles** the same code path cross-products all 4 actors' draws. Four
clean attacks (damage roll only) cost `16⁴ = 65,536` replays per matrix cell.
With a 50×50 matrix, that's `1.6 × 10⁸` `step()` calls per `solve_turn`. Even
at the current ~525 ns/step, that's ~85 s per node.

The crux: in many doubles turns, the four actions don't cross-interact. If
A1's damage roll never changes A2's outcome distribution, the joint frontier
**factors**:

```
enumerate(a1 × a2 × a3 × a4)
    ≡ enumerate(a1) ⊗ enumerate(a2) ⊗ enumerate(a3) ⊗ enumerate(a4)
```

That collapses `16⁴ = 65,536` to `4 × 16 = 64`, a **~1024×** speedup,
**lossless** on the frontier. The hard part is detecting independence
*safely* — a false-positive silently corrupts the Nash value.

**Headline estimate (see §5):** ~35–55 % of mid-game doubles turns are
fully or partially factorable. Conservative pre-check (PR-I.1) lands the
big win on the obvious "two ally singletons hit two foe singletons with no
field effects" case; lazy-verify (PR-I.3) catches the long tail.

---

## 2. Soundness analysis

### 2.1 Formal definition

Let a joint action be `J = (a₁, a₂, a₃, a₄)` (one move/switch per slot in
turn-order slot indexing). Let `F(J)` be the distribution over canonical
hashes returned by `enumerate_outcomes`. Let `F_k(a_k | s)` be the
distribution that *would* result from enumerating only actor `k`'s draws
while pinning every other actor's outcome to its recorder-drawn path.

**Factoring is sound** when:

```
F(J)  ≡  marg-prod F_k(a_k | s_pre)
```

i.e. each actor's marginal distribution is unaffected by the realized
outcomes of the other actors' RNG draws. This is the standard product
measure on independent random variables.

The critical phrase is "unaffected by the realized outcomes" — *not* "the
actors don't read shared state." Both A1 and A2 reading the same Pokemon's
HP is fine as long as **A1's RNG draw can't change what A2 reads**. The
distribution-level independence is what we need; serialization-order
shared reads are benign.

### 2.2 What breaks factorability — the catalog

Categories below are condensed from a grep of `crates/vgc-engine-core/src/`.
Every entry is one of: **ALWAYS** breaks (move/field-encoded, no exit), or
**CONDITIONAL** breaks (gated on state we can pre-check).

#### A. Spread / multi-target moves (ALWAYS breaks)

A single chosen move covers multiple targets. Joint outcomes per target
are NOT independent — they share one BP, one accuracy site, the spread
multiplier, and the same damage roll bucket.

- `damage.rs:125-129` — `is_spread` flag, applies ×0.75 per PS step 2.
- `damage.rs:2096` — spread modifier branch in main damage formula.
- `battle.rs:3200-3204` — `enumerate_targets()` resolves target lists.
- `battle.rs:6339` — spread moves bypass redirection.

→ **Detection:** read `move.target_kind` from move metadata. If the target
covers >1 slot (`AllAdjacentFoes`, `AllAdjacent`, `AllyAdjacent`,
`Allies`, `FoeSide`, `AllySide`, `AllSides`, `Field`), the actor cannot
be factored out independently of the actors it co-targets.

#### B. Helping Hand (ALWAYS breaks within the boosted pair)

Pure cross-actor damage boost.

- `pokemon.rs:2011-2019` — sets `helping_hand` volatile on ally.
- `damage.rs:1319` — ×1.5 BP multiplier when the recipient attacks.

→ **Detection:** any actor's move slug = "helpinghand" disqualifies factoring
between that actor and its ally for this turn. The OTHER ally pair on the
field may still factor.

#### C. Redirection (CONDITIONAL on having a single-target move targeting a
redirector's side)

- `battle.rs:6358-6472` — Follow Me / Rage Powder volatile + target swap.
- `battle.rs:6474-6531` — Lightning Rod / Storm Drain absorb + SpA boost.
- `battle.rs:6416-6419` — Stalwart / Propeller Tail / Snipe Shot bypass.

→ **Detection:** if any actor's move targets the opposing side AND any
opponent has Follow Me / Rage Powder volatile or a redirecting ability
on a typed-match move, factoring is unsafe within the affected target's
pair (the redirector eats both attacks).

#### D. KO-triggered abilities + variance-gated KOs (CONDITIONAL on
defender HP / damage range straddling a KO threshold)

This is the **most dangerous** mode because it's subtle. Sequence:

1. Actor 1's damage roll varies; some rolls KO the target, others don't.
2. On KO: Beast Boost / Moxie / Chilling Neigh / Grim Neigh fires →
   attacker stat goes up.
3. Now actor 3 (same side as actor 1) attacks: the stat boost feeds the
   damage formula → actor 3's distribution depends on actor 1's roll.

- `battle.rs:5556-5572` — Beast Boost / Moxie dispatcher on KO.
- `battle.rs:5331` — `target_fainted_this_hit` gate.
- `battle.rs:34027-34051` — Moxie KO test.
- `battle.rs:5381-5402` — Sturdy gate (interacts with damage variance).
- `item.rs:154-201` — Focus Sash gate (same).

→ **Detection:** if any attacker carries one of {Beast Boost, Moxie,
Chilling Neigh, Grim Neigh, Soul-Heart, Battle Bond} AND its move could
plausibly KO any defender on the field, factoring fails for any actor on
the attacker's side whose result depends on attacker's stats. Static
range-check: `defender.hp_current > max_roll_damage` ⇒ safe; otherwise
fall back.

#### E. Switch-in abilities firing mid-turn (CONDITIONAL on KO + auto-switch
or U-turn / Volt Switch / Eject Button)

If actor 1 KOs a foe AND a replacement comes in mid-turn (rare in Reg M-B;
replacement is normally end-of-turn), Intimidate / Trace / Imposter / Download
fire and mutate state mid-resolution.

- `ability.rs:374-425` — Intimidate.
- `ability.rs:621-673` — Trace.
- `ability.rs:681-809` — Imposter.
- `ability.rs:1503-1507` — Download.
- `ability.rs:462-495` — Weather setter on entry.
- `ability.rs:512-534` — Terrain setter on entry.
- `item.rs:1200-1301` — Eject Button / Eject Pack.

→ **Detection:** if any actor's chosen move is U-turn / Volt Switch / Parting
Shot / Flip Turn / Baton Pass, OR any target carries Eject Button / Eject
Pack / Red Card, OR any KO can fire (see D), factoring fails for actors
ordered after the switch-in.

#### F. Weather / Terrain set THIS TURN by a move (ALWAYS breaks if any
later actor's move reads the field)

- Sunny Day / Rain Dance / Sandstorm / Snowscape / Electric Terrain etc.
- All weather/terrain *setter* moves; field state mutates within the turn.
- `ability.rs:472-475` — weather field assignment.

→ **Detection:** if any actor's move sets weather/terrain AND that actor goes
before another actor whose move's damage depends on weather/terrain
(Weather Ball, sun-Fire, rain-Water, Grassy/Misty/Psychic/Electric Terrain
boosts), factoring fails.

#### G. Speed re-order mid-turn (Tailwind / Trick Room / After You / Quash)

- `side.rs:45-51` — Tailwind.
- `battle.rs:286`, `order.rs:10` — Trick Room.
- `order.rs:117-123`, `battle.rs:316-321` — After You / Quash queue
  reorder mid-turn.

→ **Detection:** any actor's move is in the set {Tailwind, Trick Room,
After You, Quash}, factoring fails for actors whose turn order changes.
(Tailwind doesn't affect this turn's order so it's usually safe THIS turn;
but if a subsequent actor's move is speed-gated, double-check.)

#### H. Field-state items + damage variance

- `item.rs:732-755` — Air Balloon pops on hit (changes Ground immunity).
- `item.rs:758-807` — Weakness Policy (changes attacker stats post-hit).
- `damage.rs:189` — Power Spot (ally-presence multiplier; affected if
  ally KO'd this turn).
- `damage.rs:197` — Battery (same).
- `damage.rs:202-206` — Steely Spirit (same).
- `damage.rs:207-210, 372` — Friend Guard (affected if ally KO'd).

→ **Detection:** if any ally with Power Spot / Battery / Steely Spirit /
Friend Guard could be KO'd by an opposing damage roll this turn, the
remaining ally's distribution depends on that roll.

#### I. Stat-rebound abilities + Mirror Herb (CONDITIONAL on a stat change
being inflicted)

- `ability.rs:133-161` — Defiant / Competitive.
- `item.rs:981-983, 1350-1357` — Mirror Herb.
- `battle.rs:8532` — Mirror Armor.

→ **Detection:** if any actor's move includes an opposing-stat-drop
secondary AND the target side has Defiant / Competitive / Mirror Armor /
Mirror Herb, factoring fails.

#### J. Ally-presence damage multipliers (ALWAYS-safe-if-no-KO)

Power Spot, Battery, Steely Spirit, Friend Guard are static reads of "is
this slot alive". If no factor-pair member can KO any of these holders
this turn (range-check), they're factor-safe.

#### K. Sucker Punch / Quick Guard / Wide Guard (CONDITIONAL on priority
move present)

- `battle.rs:2944-2962` — Sucker Punch detection.
- `side.rs:81` — Wide Guard.
- `side.rs:87-89` — Quick Guard.

→ **Detection:** Sucker Punch reads "is foe using a positive-priority move
this turn" — that's inherently a joint-action read. Factoring fails when
a Sucker Punch user is present and any foe could use a priority move.
Wide/Quick Guard similarly cross-read intent.

#### L. Tiebreak ordering (PARKED, see PsGen5 memory)

`order.rs:435` ordering nonce. Equal-speed actors flip a Tiebreak draw
that's currently marginalized to the single recorded value (lib.rs:152).
Factoring shouldn't make this worse; the marginalization is already
imperfect under cross-product enumeration. Document as a known
limitation; no new risk introduced.

### 2.3 Summary table

| Class | Always breaks? | Detection cost |
|---|---|---|
| Spread move | yes (within targeted pair) | O(1) per actor move metadata |
| Helping Hand | yes (within boosted pair) | O(1) |
| Redirection volatile | conditional on volatile + matching target | O(slots) field scan |
| KO-triggered ability + variance KO | conditional on damage-range + HP | O(slots) damage range estimate |
| Mid-turn switch-in | conditional on U-turn-class move or Eject items | O(slots) |
| Weather/terrain setter move | yes if downstream actor weather-sensitive | O(1) per actor + O(actors) cross |
| Speed reorder move | yes if downstream actor is speed-sensitive | O(actors) |
| Field-state item / ally-presence multiplier | conditional on KO-this-turn | O(slots) |
| Stat-rebound vs stat-drop secondary | conditional on stat-drop secondary | O(slots) |
| Sucker Punch / Quick / Wide Guard | conditional on priority move present | O(actors) |

---

## 3. Detection strategy

### Two angles

**A. Conservative pre-check.** Inspect static metadata (move target,
secondary effects, item, ability, field) at `enumerate_outcomes` entry.
Compute a `Factorable` verdict; if `FullyFactor`, enumerate each actor's
draws independently and tensor-product. Cheap (single pass over 4 actors)
and easy to test. False-negative-prone (we leave perf on the table) but
false-positive-safe by construction (every breaker above is enumerated).

**B. Lazy verification.** Enumerate per-actor, take the cross-product of
hashes, and **verify by sampling**: pick one combo at random, run it
through full `enumerate_outcomes` (the existing path), check that its
canonical hash falls in the tensor-product set with the right marginal
prob. Expensive on success (one full step) but catches detection bugs.

### Recommendation: ship A first, layer B as guard

- **PR-I.1**: pre-check classifier. This is the load-bearing perf PR.
- **PR-I.3 (later)**: lazy-verify *opt-in* under `--paranoid` mode for
  conformance harness use — never in the hot solver loop.

The pre-check's bug surface is small (it's a closed enumeration of known
breakers, hand-audited against the catalog above), and **every false
positive is a perf regression, not a correctness bug** as long as the
catalog is complete. The risk is a missed entry in the catalog → silent
incorrectness. Mitigations in §4.

### Pre-check API sketch

```rust
pub enum Factorable {
    /// All 4 actors enumerate independently.
    FullyFactor,
    /// Subsets of actors form independent groups. e.g. vec![vec![0,1],
    /// vec![2,3]] means actors 0-1 must enumerate jointly, 2-3 jointly,
    /// but the two groups factor across each other.
    PartialFactor { factor_groups: Vec<Vec<usize>> },
    /// No safe factoring — fall back to current cross-product.
    NoFactor,
}

fn classify_factorability(
    battle: &Battle,
    joint: &[(Choice, Choice)], // by slot
) -> Factorable;
```

`PartialFactor` is the realistic common case in doubles: one side has a
spread move (slots 0-1 jointly enumerate) but the other side has two
independent singletons (slots 2 and 3 each factor). That alone is a
`16⁴ → 16²·16·16 / dedup`… actually wait — slots 2 and 3 are independent
of each other AND of {0,1}, so we get `16² · 16 · 16 = 16⁴` ?? No: the
factoring savings come from enumerating *separately*: `16²` combos for
the spread + `16` combos for slot 2 + `16` combos for slot 3 = `16² +
32 ≈ 288` step calls, replacing `16⁴ ≈ 65 k`. ~225× win on this case.

### Joint enumeration combinator

After per-group enumeration, the frontier is the tensor product of group
frontiers. Two helpers:

```rust
fn enumerate_group(
    base: &Battle,
    full_joint: &[(Choice, Choice)],
    group_slots: &[usize],
    record_seed: u64,
) -> OutcomeFrontier;

fn tensor_product(groups: &[OutcomeFrontier]) -> OutcomeFrontier;
```

`enumerate_group` runs `enumerate_outcomes` with "pinned" actions for the
non-group slots (recorder seed selects one realization), then strips the
non-group draws from the per-site list before cross-producting. This
preserves the existing record-pass / lazy re-record machinery exactly.

`tensor_product` multiplies probabilities and **must re-run step() on
each combined cell** to produce a real `Battle` (the canonical_hash and
mutated state) — there's no way to "stitch" two `Battle`s. So the actual
savings come from enumerating *what state the recorder needs to vary*,
not from skipping the cross-product entirely. Hmm.

**Restated savings model.** Without factoring, we vary every site across
every other site: `Π |site_i|` step calls. With factoring on independent
groups, we still need to run `step()` on every joint outcome (we need a
real `Battle`), but the **non-redundant joint outcomes** are
`Σ_g Π_{i ∈ g} |site_i|`. Same denominator after dedup, *fewer
pre-dedup combos*: that's the win.

Concretely: 4-singleton, 16-damage-roll case. Without factoring: 65,536
step calls, dedup to ≤ 16⁴ canonical outcomes (probably ~hundreds after
dedup, since post-KO and pre-KO collapse). With factoring: enumerate
each group (16 step calls each = 64 total), then for the **joint
canonical-state generation** we need ~|group₁| · |group₂| · … combos
*post-dedup*. In the all-singleton case that's `4 · 4 · 4 · 4 = 256`
joint step calls if each actor's group dedups to 4 distinct outcomes.

Total: **~320 step calls vs ~65 k. ~200× speedup,** not 1024× — but the
factor-of-200 is the right order of magnitude.

### 3.1 Tensor product gotcha

Joint step calls aren't avoidable because canonical state is needed for
the solver. The win is on the *enumeration* (RNG-table-construction)
side: per-group enumeration discovers the relevant draw-site distributions
in `Σ` time, then the joint-step pass uses *one realization per group*
combined. This means the joint pass replays each combination of group
realizations — but only the *distinct outcome buckets*, not the raw
draw-site combos.

Open question (§6): can we skip the joint-step pass entirely by hashing
"applied delta" per group and composing? Likely no — KO ordering, weather
overlap, Friend Guard etc. interfere. Park.

---

## 4. Risk register

### 4.1 Failure modes ranked

| ID | Mode | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Catalog miss: a new mechanic added later that cross-interacts isn't in `classify_factorability` | High over time | Wrong Nash value, hard to detect | CI conformance check (R-mit-1), AGENTS.md rule |
| R2 | Classifier misreads move metadata (e.g. target-kind enum value drift) | Medium | Wrong factoring → wrong frontier | Per-breaker unit test (R-mit-2) |
| R3 | Range-check for KO/variance is wrong (off-by-one on min/max roll) | Medium | False FullyFactor when KO is achievable on max roll only | Use existing damage formula (R-mit-3) |
| R4 | Joint-step composition forgets a Battle field | Low | Wrong canonical hash → wrong dedup | Property test: factored frontier ≡ unfactored frontier under random teams (R-mit-4) |
| R5 | Lazy re-record loop interacts poorly with per-group enumeration | Medium | Convergence failure, runaway loop | Cap iterations as today; surface `unmatched_total` (R-mit-5) |

### 4.2 Guards

- **R-mit-1 (CI conformance).** Extend the `breadth_corpus` harness
  (memory: `project_breadth_corpus.md`) with a new mode: for every joint
  action in the corpus, run both `enumerate_outcomes` (current) and the
  factored variant; assert the deduped distributions match within 1e-9.
  Run weekly or per-PR-to-solver.
- **R-mit-2 (unit tests per breaker).** One test per row in §2.3.
  Fixture-build the interaction (Helping Hand on slot 0, Beast Boost on
  slot 1, etc.), assert classifier returns `NoFactor` or correct
  `PartialFactor`. ~12 tests.
- **R-mit-3 (range-check uses real damage).** Reuse
  `vgc_engine_core::damage::compute_damage` to bound min/max — don't
  hand-roll the range check.
- **R-mit-4 (property test).** Generate random teams + random joint
  actions; for each, compare factored vs unfactored frontier distributions.
  Bound to 100 seeds in CI, 10 k seeds for nightly.
- **R-mit-5 (lazy re-record per group).** Each group runs its own lazy
  loop; cap at `MAX_LAZY_ITERATIONS` per group, surface combined
  `unmatched_total` in the result.

### 4.3 The acceptable-bug bar

Per the `feedback_no_psgen5_rng_draw_matching` memory, we already accept
some imperfection in marginalization (Tiebreak, UniformPercent). Factoring
must NOT make the *deduped distribution* worse, modulo those known
shortcuts. The property test in R-mit-4 is the gate.

---

## 5. Scope estimate

### PR breakdown

| PR | Title | Effort | LoC | Deps |
|---|---|---|---|---|
| PR-I.1 | `Factorable` classifier (pre-check) — closed enumeration of breakers from §2.2 | M | ~400 in `vgc-solver/src/factoring.rs`, ~50 in `lib.rs` | none |
| PR-I.2 | Per-group enumeration + tensor product wiring in `enumerate_outcomes` | M | ~250 (refactor `enumerate_outcomes`) | PR-I.1 |
| PR-I.3 | Unit tests per breaker class (12 tests, fixtures) | S | ~600 (tests + fixtures) | PR-I.1, PR-I.2 |
| PR-I.4 | Conformance corpus extension: factored vs unfactored equality check | S | ~150 (new conformance mode) | PR-I.2 |
| PR-I.5 (optional) | Lazy-verify safety net (sample-and-replay) | S | ~100 | PR-I.2 |

**Total effort:** ~5 PRs, ~1450 LoC, ~3–5 days end-to-end.

### Headline factorability % estimate

Based on the catalog and rough composition of doubles turns in the
`regmb_full_breadth` corpus (memory: `project_breadth_corpus.md`):

- **Fully factorable (4 singletons, no field setter, no Helping Hand, no
  cross-actor item/ability)**: ~25–35 % of mid-game turns. Heaviest perf
  wins here.
- **Partially factorable (3 singletons + 1 spread, OR 2 pairs each
  jointly-coupled)**: ~25–30 % of turns. Still 4–50× speedup.
- **Not factorable (spread + Helping Hand, weather setter + Sun-boosted
  attacker, Sucker Punch present, Mirror Herb present, etc.)**: ~35–50 %
  of turns. No win.

**Aggregate expected speedup on doubles outcome enumeration:**
~6–15× wall-clock on a representative corpus (the fully-factorable
quarter contributes most; partials add a multiplier on top).

These percentages are inferred from move/ability frequency in
`docs/champions-data-deltas.md` and the breadth corpus's known
distributions. **Open question §6.1:** measure for real before committing
to PR-I.2.

### Conclusion

**Ship PR-I.1 first.** It's the keystone (the classifier) and is
independently testable without any frontier wiring. If the measured
factorable-% on the breadth corpus is <20 %, abandon PR-I.2 (the perf
win isn't worth the joint-step composition complexity). If it's >25 %,
proceed to PR-I.2.

---

## 6. Open questions

1. **(§5) What's the real factorable-% on the breadth corpus?** Need a
   quick measurement script: classify every turn in `regmb_full_breadth`
   and bucket. Cole to confirm the corpus path; this is a 1-hour task.
2. **(§3.1) Can the joint-step pass be skipped entirely?** I think no
   because canonical state composition requires re-running the engine,
   but a "Battle delta" representation might exist. Worth a 30-min
   prototype.
3. **(§2.2 D) KO-range check granularity.** Pre-check uses min/max
   damage rolls to decide "can this KO?". Edge cases: Sash holders at
   full HP (always survive once), Sturdy holders, Multiscale (full HP
   only). Is it worth modeling these explicitly, or fall back to NoFactor
   conservatively when any of these are present?
4. **(§3) PartialFactor data structure.** Vec-of-Vecs is fine for ≤4
   actors but feels overengineered. Bitmasks?
5. **(§4.2 R-mit-4) Property test seed budget.** 100 in CI is cheap; is
   nightly 10 k actually catching new bugs or just paranoia?
6. **(§2.2 L) Tiebreak handling.** When equal-speed actors are on
   opposite sides, factoring within each side is fine; cross-side
   tiebreak is the existing marginalization. Confirm no new risk
   introduced (I believe none, but worth a paragraph).
7. **(§3 enumerate_group)** Does the "pinning" approach interact
   correctly with `Rng::Recording`'s site discovery for the
   *non*-pinned actors? Specifically: a non-pinned actor's recorded
   sites must not be in the per_site list when we enumerate the pinned
   group. The current `enumerate_outcomes` records ALL actors' sites in
   one pass; we'd need a way to filter by `RngKey.actor`. The key
   already carries an actor field (lib.rs:336), so this should be a
   filter-and-go — but verify.
8. **Does PR-I require changes to `vgc-engine-core` at all?** Current
   plan: no, if pre-check + per_site filtering is sufficient. Confirm
   on prototype.

---

## Appendix A: Mechanism citations (condensed)

Full catalog of breakers with `file:line` references is in §2.2. Notable
groupings:

- Damage formula spread / ally multipliers: `damage.rs:125-129,
  137-147, 189, 197, 202-210, 372, 1319, 2096`.
- KO + ability triggers: `battle.rs:5331, 5381-5402, 5556-5572`.
- Field setters: `ability.rs:374-425, 462-495, 512-534, 621-673,
  681-809, 1503-1507`.
- Redirection & priority: `battle.rs:2944-2962, 3599-3604, 6358-6531`.
- Order machinery: `order.rs:8-12, 10, 17-18, 117-123, 428`;
  `battle.rs:286, 316-321`.
- Field-state items: `item.rs:154-201, 732-755, 758-807, 981-983,
  989-1154, 1200-1301, 1350-1357`.

These are the load-bearing references for PR-I.1's classifier; each is a
hand-verified breaker from the §2.2 catalog.
