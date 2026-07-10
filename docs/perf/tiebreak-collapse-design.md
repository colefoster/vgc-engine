# Tiebreak collapse — design note (v1)

**Goal.** Cut the doubles solver's speed-tie branching. In the identical-speed mirror,
each turn spawns `2^4 = 16` outcome replays purely from four `DrawSpace::Tiebreak{speeds_tied:true}`
sites (four per-action nonces sharing one `(priority, frac_pri, speed_key)` bracket — see
`order.rs::mark_tied_tiebreaks`). When the tied actions **provably commute** (every ordering
reaches the same `canonical_hash`), we collapse the bracket to a single ordering.

Measured: 100 % of `enumerate_pass` calls in the mirror floor case have exactly 16 combos, and
the 16 is entirely turn-order (damage already collapsed by `DamageSegments`). So this attacks the
actual combinatorial driver.

## Where the gate lives

Engine-side in `order.rs::mark_tied_tiebreaks`, **not** solver-side: the four tiebreak nonces
record under stale leftover `RngKey` context, so the solver cannot tell which actors a site
disambiguates. `mark_tied_tiebreaks` still holds the full `MoveEntry` list (actors, moves,
targets, speed keys). Today it flips `speeds_tied:false → true` for tied entries; the collapse is
simply: **when the tied bracket is commute-safe, skip the flip** — leaving `false`, which the
existing solver `expand()` path already marginalizes to one branch. Zero solver changes.

**Perf guard.** The whole check runs only when `rng.recording_log().is_some()` (solver record
pass). `speeds_tied` is never read outside solver frontier enumeration — self-play / PsGen5 /
conformance decide order from the real nonce — so the hot path pays nothing and conformance
cannot change.

## Soundness obligation

If the gate returns "collapse", **every ordering of the tied bracket must produce an identical
`canonical_hash`.** We prove this structurally (deny-by-default), never by sampling. A false bail
only costs perf; a false certify silently drops reachable states. Prefer blunt over surgical.

## Commute conditions (ALL must hold to collapse)

1. **Recording mode** active (else skip — perf).
2. **Doubles** (`active_count() >= 2`) and **all actions are moves** (any mid-turn switch → bail).
3. **Distinct single targets, no global coupler:** `compute_coupled_targets(order) == 0`. This
   single check rejects: two attackers on one defender, any spread move, any redirection
   (Follow Me / Rage Powder / Lightning Rod / Storm Drain / Sap Sipper), Ally Switch, Instruct.
4. **Every tied move is "plain damage":** category != Status; `has_secondary == false`; not
   fixed-damage / OHKO / Counter / Mirror Coat / Metal Burst (formula doesn't bound them);
   `multihit_max <= 1`; not in `ORDER_READING_MOVES` (reads turn-order / same-turn state):
   Sucker Punch, Fake Out, First Impression, Payback, Analytic, Assurance, Bolt Beak, Fishious
   Rend, Retaliate, Round, Me First, Pursuit, Focus Punch, Beat Up, plus any move whose BP reads
   **user or target HP** (Reversal, Flail, Wring Out, Crush Grip, Hard Press, Eruption, Water
   Spout, Dragon Energy) — a prior hit changes HP → changes BP.
5. **No pre-action faint (target-only bound + self-HP hazards eliminated at source).** For every
   tied attacker slot, `sum of max-incoming damage from all tied attackers targeting it <
   current_hp`. Max-incoming uses `damage::damage_range().1` × crit-1.5 upper bound (mirrors
   `mutual_focus_tensor_safe`), summed order-independently (stricter). If any attacker could be
   KO'd by incoming before it acts, orderings differ → bail.
   **REVISION (adversarial-review HOLE A):** the target-only sum misses an attacker's *own*
   mid-turn HP change — move recoil, Life Orb recoil, contact chip, or drain/Shell Bell heal —
   which can flip a later pre-action faint that a *third* tied mon's plain hit triggers. Rather
   than model self-HP deltas, **eliminate the hazards** so the target-only bound is sufficient:
   - per-move bail: `recoil_num > 0` (Brave Bird/Flare Blitz/…) or `drain_num > 0`
     (Drain Punch/Giga Drain/…) or crash-on-miss moves (Jump Kick family).
   - the all-active-mon inert requirement (condition 6) denies Life Orb / Shell Bell (self-HP
     items) and Rough Skin / Iron Barbs / Rocky Helmet (contact chip) holders → no chip/recoil/
     heal can occur among the tied set.
6. **No on-hit / on-KO SELF-STATE change (THE subtle one) — deny-by-default allowlist over ALL
   active mons, both attacker AND defender roles.** Every tied mon is both attacker and defender;
   if being hit by A triggers D's own stat/state change, D's *subsequent* attack differs by
   ordering — and attacker-side hooks (Magician, Poison Touch, Analytic, Gorilla Tactics, Supreme
   Overlord) matter too. So require **every active mon** to have ability ∈
   `TIEBREAK_INERT_ABILITIES` and item ∈ `TIEBREAK_INERT_ITEMS` (or no item). Deny-by-default:
   an unknown/unlisted ability or item → bail. A newly-added engine ability/item is denied until
   explicitly allowlisted → safe by construction.
   - **Structural denial test (assert):** any ability handled by an arm of
     `ability.rs::on_damaging_hit` is order-relevant and MUST NOT be in the allowlist. A unit test
     asserts the allowlist is disjoint from that match. Grounded DENY set (implemented hooks):
     Cotton Down, Sand Spit, Weak Armor, Color Change, Toxic Debris, Wind Power, Electromorphosis,
     Stamina, Anger Point, Berserk, **Anger Shell**, Justified, Steam Engine, Rattled, Rough Skin/
     Iron Barbs, Static/Flame Body/Poison Point, Spicy Spray, Cute Charm, Cursed Body, Effect
     Spore, Mummy/Lingering Aroma, Wandering Spirit, Poison Touch, Toxic Chain, Magician, Disguise,
     **Steadfast**, Stance Change, Unburden, Symbiosis, Analytic, Supreme Overlord, Gorilla Tactics,
     Solar Power, Protosynthesis/Quark Drive, Motor Drive, Lightning Rod/Storm Drain, Flash Fire.
     Also DENY (own-HP/status readers): Defeatist, Slow Start, Guts, Hustle. Also DENY by name
     though unimplemented (future-proofing): Moxie, Beast Boost, Chilling/Grim Neigh, Soul-Heart,
     Aftermath, Innards Out, Perish Body, Emergency Exit, Wimp Out, Gulp Missile, Ice Face, etc.
   - **v1 ALLOW seed (order-inert):** static damage mods (Technician, Rivalry, Sand Force, Iron
     Fist, Mega Launcher, Strong Jaw, Sharpness, Tough Claws, Reckless, Punk Rock, Sniper,
     Adaptability, Tinted Lens, Filter/Solid Rock/Prism Armor, Multiscale, Thick Fat, Huge/Pure
     Power, the -ate conversions; **Sheer Force safe ONLY because condition 4 bails any move with
     `has_secondary`**), switch-in-only (Intimidate, weather/terrain setters, Trace, Download),
     never-mutating auras (Ruin flags, Friend Guard, Aura Break, Unnerve/As One, Neutralizing Gas).
   - **item ALLOW seed:** no item; Choice Band/Specs/**Scarf** (speed folded into `speed_key`
     pre-tie — cannot create/destroy the tie); Muscle Band, Wise Glasses, Expert Belt, Punching
     Glove, Assault Vest, type plates/gems, Leftovers/Black Sludge (end-of-turn only), Covert Cloak.
     **item DENY** (implemented hooks): Weakness Policy, Absorb Bulb, Cell Battery, Snowball,
     Luminous Moss, Throat Spray, Focus Sash, Focus Band, Air Balloon, Red Card, Eject Button/Pack,
     Rocky Helmet, Sticky Barb, Jaboca, Rowap, Sitrus/Oran/Berry Juice, all pinch berries,
     type-resist berries, White Herb, Mirror Herb, **Custap**, Lagging Tail/Full Incense,
     **Life Orb** (recoil → HOLE A), **Shell Bell** (heal → survive-flip), Loaded Dice.

## Reusable predicates (from `mutual_focus_tensor_safe`, `battle.rs:3033`)

- `compute_coupled_targets(&order)` — condition 3, wholesale.
- The chance-gated status/secondary bail loop (para/sleep/freeze/confusion/attract/flinch/Truant +
  any-secondary) — condition 4 partial.
- The max-incoming faint walk (fixed/OHKO id set, crit×1.5, multihit) — condition 5, wholesale but
  computed as an order-independent SUM per slot (stricter, simpler).
- `same_action_slots`, `ResidualIndex::abs_slot`, `damage::damage_range`, `MOVES[id].has_secondary`.

## What v1 deliberately does NOT do

- Only fires on **non-KO plies** (full/high HP). KO-race plies legitimately don't commute; the
  gate bails there. It prunes the top of the tree, where branching compounds.
- Only fires for mons whose ability + item are in the (initially small) inert allowlists —
  unknown → bail. Widening the allowlists is safe follow-up.
- Does not collapse shared-target brackets (even though two plain hits on one defender with no
  threshold item commute) — deferred; `compute_coupled_targets != 0` bails.

## Audit plan

- Bit-exact: enumerate a commuting mirror cell gate-ON vs gate-OFF → identical frontier
  (outcomes, probs, hashes). raw_combos 16 → 1.
- Anti-vacuous guards: `assert_collapsed` / `assert_not_collapsed` per bail category, each failing
  before / passing after — cover distinct-target, faint-possible, secondary, order-reading move,
  inert-ability-violation (Berserk), inert-item-violation (Weakness Policy), spread, redirect.
- Independent adversarial code-review pass whose sole job is hunting a missing commute-violation.
