# Gen-9 mechanic interactions — vgc-engine reference

A catalogue of the non-obvious edge cases that bite competitive-sim implementations. Each topic states the rule, lists the cases that diverge from naive expectations, and points at where in `vgc-engine-core` the behaviour needs to live. Sources are the Pokémon Showdown tree (`smogon/pokemon-showdown`, `data/` and `sim/`) and Bulbapedia.

> Hard rule from `docs/AGENTS.md`: when PS and the cartridge disagree, **vgc-engine matches PS**. Where they disagree it is called out explicitly.

The PS file:line references throughout point at the shallow clone laid down by research at `/tmp/pokemon-showdown-research`; line numbers track the current `master` snapshot used. Bulbapedia URLs are canonical and stable; PS line numbers may drift one or two as the tree evolves.

---

## 1. Prankster

### What it is

Prankster gives the user's **status moves** +1 priority. Since gen 7, Dark-type Pokémon are immune to any Prankster-boosted move *targeted at them by a foe*. The immunity is per-target and does not retroactively un-boost the priority — the move still goes early, it just bounces off Dark mons on the opposing side.

### The interactions

- **Dark immunity is per-target and gated on `!isAlly(pokemon)`.** A Prankster Tailwind, Light Screen, Reflect, Aurora Veil, Mist, Safeguard, Wish, Ally Switch, or Helping Hand still works fine because it targets the user's side or an ally. Self-targeted Roost / Recover / Substitute / Calm Mind also unaffected. The Dark check fires only inside `hitStepTryImmunity` for targets where `!targets[i].isAlly(pokemon)`. PS: `sim/battle-actions.ts:671` (`if (this.battle.gen >= 7 && move.pranksterBoosted && pokemon.hasAbility('prankster') && !targets[i].isAlly(pokemon) && !this.dex.getImmunity('prankster', target))`). Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Prankster_(Ability)>.
- **Doubles spread, mixed types.** A Prankster Thunder Wave can't be spread; but a Prankster Will-O-Wisp likewise targets one foe — Dark immunity only matters per-target. For genuinely spread Prankster moves (e.g. Confide, Growl) PS evaluates immunity per-slot inside the for-loop in `hitStepTryImmunity`: one Dark + one non-Dark on the foe side means the Dark mon shows `-immune` and the other still gets hit normally. Same logic block; see the `for (const [i, target] of targets.entries())` loop.
- **Magic Bounce vs Prankster.** Magic Bounce reflects reflectable status moves back at the user. The reflected move has `pranksterBoosted = false` explicitly stripped, so the bounced copy *does not* get +1 priority and is *not* Dark-immune (because the new target is the original Prankster user, who is rarely Dark). PS: `data/abilities.ts:2400` and `:2410` (`newMove.pranksterBoosted = false`).
- **Soundproof vs Prankster sound moves.** Soundproof blocks `move.flags['sound']` before Prankster's Dark check matters; the move shows `-immune … from ability: Soundproof`. Soundproof's `onTryHit` runs in the same step and produces the immune line first only if the targets are evaluated in order — but practically: both immunities point at "no effect", and Soundproof has a `breakable: 1` flag so Mold Breaker would lift it, while Prankster's Dark immunity is **not** an ability check on the defender and cannot be Mold-Breakered.
- **Prankster priority survives even when the move misses or fails.** The priority is recomputed by `onModifyPriority` regardless of legality; the Dark check is in immunity, not priority. So a Prankster Encore at +1 still moves first even if the foe is Dark — it just doesn't connect.

- **Prankster does not boost moves that are already non-status under some modifier.** Sucker Punch is non-status, no effect. Hidden Power category determination is moot for Prankster (all damaging).
- **Mode-changing moves: Hyperspace Hole, Hyperspace Fury, Photon Geyser** — irrelevant; not status-category.
- **Z-status Prankster moves (gen 7).** Out of scope for gen 9.
- **What about an illusion-masked Dark mon?** PS: the `isImmune` check uses real types (Illusion doesn't change types), so a Zoroark masquerading as a Pikachu is still Dark-immune to Prankster. PS shows a `hint` if the target was illusion-masked: `if (target.illusion || !(move.status && ...))`. The illusion breaks visibly through the hint when revealed by Prankster-immunity. PS: `sim/battle-actions.ts:674`.
- **Queenly Majesty / Dazzling / Armor Tail on allies.** These abilities block priority moves targeting their side, including Prankster-boosted status moves into the user's adjacent allies. Distinct from Dark-type immunity; check applies to ALL priority moves. `breakable: 1` — Mold Breaker bypasses.
- **Psychic Terrain.** Blocks all priority moves against grounded mons on the terrain. Prankster-boosted status moves into a grounded foe fail entirely. Self-targeted / ally-targeted Prankster moves unaffected because they don't target the grounded foe.

### Engine implementation notes

- Prankster priority boost lives in `crates/vgc-engine-core/src/ability.rs` (`onModifyPriority` analogue). Set a transient `move.prankster_boosted: bool` on the chosen move at queue time.
- Dark immunity check goes in the move-resolution path in `battle.rs` immediately before damage / effect application — mirror PS's `hitStepTryImmunity` location, gated on `prankster_boosted && !is_ally(user, target) && target.has_type(Dark)`.
- Magic Bounce is not yet implemented; when it lands, ensure the bounced `ActiveMove` clone clears `prankster_boosted`.
- Soundproof / breakable handling: see Mold Breaker section below.

---

## 2. Mold Breaker / Teravolt / Turboblaze

### What it is

These three abilities make the **user's damaging moves** ignore defender-side abilities that would otherwise change move legality, damage, or immunity. They are functionally identical at the rules level — the only differences are the announce text and the species that get them.

### The interactions

- **Mold Breaker sets `move.ignoreAbility = true` via `onModifyMove`.** That flag is consulted only when the move resolution path queries the defender's ability. PS: `data/abilities.ts:2648` (`onModifyMove(move) { move.ignoreAbility = true; }`).
- **What it bypasses: every defender ability tagged `flags: { breakable: 1 }`.** Pulled from `data/abilities.ts`, the complete gen-9 list of `breakable` abilities is:

  Armor Tail, Aroma Veil, Aura Break, Battle Armor, Big Pecks, Bulletproof, Clear Body, Contrary, Damp, Dazzling, Disguise, Dry Skin, Earth Eater, Filter, Flash Fire, Flower Gift, Flower Veil, Fluffy, Friend Guard, Fur Coat, Good as Gold, Grass Pelt, Guard Dog, Heatproof, Heavy Metal, Hyper Cutter, Ice Face, Ice Scales, Illuminate, Immunity, Inner Focus, Insomnia, Keen Eye, Leaf Guard, Levitate, Light Metal, Lightning Rod, Limber, Magic Bounce, Magma Armor, Marvel Scale, Mind's Eye, Mirror Armor, Motor Drive, Mountaineer, Multiscale, Oblivious, Overcoat, Own Tempo, Pastel Veil, Punk Rock, Purifying Salt, Queenly Majesty, Rebound, Sand Veil, Sap Sipper, Shell Armor, Shield Dust, Simple, Snow Cloak, Solid Rock, Soundproof, Sticky Hold, Storm Drain, Sturdy, Suction Cups, Sweet Veil, Tangled Feet, Telepathy, Tera Shell, Thermal Exchange, Thick Fat, Unaware, Vital Spirit, Volt Absorb, Water Absorb, Water Bubble, Water Veil, Well-Baked Body, White Smoke, Wind Rider, Wonder Guard, Wonder Skin.

  PS: greppable via `data/abilities.ts` — `awk '/^\t[a-z]+: \{$/{name=$1} /breakable: 1/{print name}'`. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Mold_Breaker_(Ability)>.
- **What it does NOT bypass.**
  - **Magic Guard** — not `breakable`. Magic Guard isn't a damage-modifying or immunity ability per se, and PS treats Mold Breaker as not affecting it. The Brave Bird recoil example often quoted is wrong: Magic Guard still negates Life Orb on the *user* (orthogonal), but Mold Breaker doesn't strip Magic Guard from a defender either way.
  - **Storm Drain / Lightning Rod redirection.** These are *redirection*, executed before the immunity step; they are `breakable` and Mold Breaker DOES bypass them — the move both ignores the immunity and is not redirected. PS: both abilities carry `breakable: 1` (Lightning Rod at `data/abilities.ts:~2249`, Storm Drain at `~4596`). The redirection logic checks `!this.runEvent('RedirectTarget', …)` which Mold Breaker's flag short-circuits when the redirector's ability would otherwise be invoked.
  - **Disguise.** Listed as `breakable` — so Mold Breaker *does* bypass Disguise (Mimikyu's bust). Same source list. Bulbapedia confirms: <https://bulbapedia.bulbagarden.net/wiki/Disguise_(Ability)>.
  - **Substitute.** Sub is not an ability at all — it's a volatile. Mold Breaker has no effect on Sub absorption. The hit still goes through Sub's `onTryPrimaryHit` (PS: `data/moves.ts:18351`).
  - **Non-defender abilities.** Battle Armor on the *attacker* (someone else hitting them) is fine. Mold Breaker only sets `ignoreAbility` on moves coming OUT, not coming IN. So a Mold Breaker mon being hit by a crit-banning ability holder isn't relevant — the relevant case is "does the user's attack ignore the target's ability."
- **Status moves are not affected.** `onModifyMove` runs for status moves too and sets the flag, but PS's defender-ability checks for things like Soundproof, Volt Absorb etc. are already gated on the move category in many places; in practice Mold Breaker does suppress Soundproof against Boomburst etc. Bulbapedia notes status moves are still affected since gen 5.

### Engine implementation notes

- Add a `move.ignore_defender_ability: bool` flag set during `on_modify_move` from the attacker's ability hook in `ability.rs`.
- All defender-ability lookups in `damage.rs` and `battle.rs` (immunity, damage modifier, post-hit triggers) must consult `move.ignore_defender_ability` AND `defender_ability.flags.breakable` before applying. Centralising via a `effective_defender_ability(&move, &defender) -> Option<Ability>` helper is cheaper than scattering the check.
- Soundproof / Wonder Guard etc.: same path.
- Deferred until: most listed abilities aren't implemented yet. The flag plumbing should land *before* the next batch of `breakable` defender abilities (Storm Drain, Levitate, Sturdy, Multiscale, etc.) so each addition is a one-liner.

---

## 3. Sheer Force

### What it is

Sheer Force boosts the power of moves with a secondary effect by ×1.3 (5325/4096 in PS's fixed-point) but **deletes the secondary entirely** before it can fire. The famous downstream consequence: Sheer Force + Life Orb skips Life Orb's recoil for boosted moves.

### The interactions

- **What counts as a "secondary".** PS's `onModifyMove` for Sheer Force checks `move.secondaries` (the array) and also strips `move.self` (self-stat boosts/drops that aren't the main effect). If those exist and `!move.hasSheerForceBoost`, it deletes them and sets `move.hasSheerForce = true`. PS: `data/abilities.ts:4158-4166`. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sheer_Force_(Ability)>.
- **`hasSheerForceBoost: true` is the manual opt-in.** A handful of moves (e.g. Mind Blown, Mystical Fire flavour notes) carry `hasSheerForceBoost: true` so Sheer Force boosts them even though their effect isn't in `move.secondaries`. PS: search `hasSheerForceBoost: true` in `data/moves.ts` (lines 4656, 13071, etc.).
- **Sheer Force + Life Orb = no Life Orb recoil for boosted moves.** Life Orb's recoil fires in `onAfterMoveSecondarySelf`, which is *skipped* when `move.hasSheerForce && pokemon.hasAbility('sheerforce')`. PS: `sim/battle-actions.ts:531` (`if (!(move.hasSheerForce && pokemon.hasAbility('sheerforce')) && !move.flags['futuremove']) { … runEvent('AfterMoveSecondarySelf') … }`). Life Orb damage modifier still applies (×1.3 in `onModifyDamage`, multiplicative with Sheer Force's ×1.3 base-power boost). Bulbapedia confirms this is also true on cartridge from gen 5 forward.
- **Sheer Force + King's Rock / Razor Fang.** Flinch chance from these items is layered as a secondary on the move when held — Sheer Force strips secondaries before they can be added, so no flinch. (King's Rock implements via `onModifyMove` adding to `secondaries` *after* Sheer Force's `onModifyMove` runs in some orderings — but in gen 9 PS, both go through the same modifier pass; King's Rock's secondary is a real entry that Sheer Force removes. See `data/items.ts` for `kingsrock`.)
- **Sheer Force + Knock Off.** Knock Off's item-removal happens in `onAfterHit` and is not in `move.secondaries` — it's the move's *primary* effect. Sheer Force does NOT boost Knock Off (no secondary present) and does NOT block the item removal. Verify: `data/moves.ts:9961` Knock Off has no `secondaries` array.
- **Sheer Force does NOT block defender-side reactive effects.** Rocky Helmet recoil, Rough Skin, Iron Barbs, Static, Flame Body, Cute Charm — all are the *defender's* effect triggered by contact, not the attacker's secondary. PS fires them via `onDamagingHit` on the defender's ability/item, completely outside the attacker's `secondaries` array. So Sheer Force users still take Rocky Helmet damage and can still get burned by Flame Body. Bulbapedia explicitly notes this.
- **Sheer Force + recoil moves (Brave Bird, Wood Hammer, Flare Blitz).** Recoil isn't a secondary and is not stripped. Flare Blitz's burn chance IS a secondary and gets stripped (Sheer Force boost applies). Brave Bird has no secondary → no Sheer Force boost.

### Engine implementation notes

- On move build/modify in `damage.rs` (or a dedicated move-modify pass in `battle.rs`), if attacker ability is Sheer Force and `move.secondaries` non-empty OR `move.has_sheer_force_boost`, set `move.has_sheer_force = true`, clear `secondaries`, clear `self_effect`, and apply the ×1.3 base-power modifier.
- The Life Orb recoil hook in `item.rs` must early-return when `move.has_sheer_force && attacker_ability == SheerForce` — mirror PS gating exactly.
- Do NOT touch defender-side contact effect dispatch; Rocky Helmet etc. remain unaffected.

---

## 4. Life Orb

### What it is

Holder's damaging moves do ×1.3 damage. After any damaging move that hit at least one target, the holder loses 10% of max HP. Recoil is "indirect damage" — Magic Guard blocks it.

### The interactions

- **Trigger location.** Recoil fires in Life Orb's `onAfterMoveSecondarySelf`. PS: `data/items.ts:3408` (`if (source && source !== target && move && move.category !== 'Status' && !source.forceSwitchFlag) { this.damage(source.baseMaxhp / 10, source, source, this.dex.items.get('lifeorb')); }`). The gating in the engine is the call site at `sim/battle-actions.ts:531`.
- **Skipped when:**
  - **Move category is Status.** Trivially gated.
  - **`move.hasSheerForce && hasAbility('sheerforce')`.** Whole `AfterMoveSecondarySelf` step skipped. PS: `sim/battle-actions.ts:531`.
  - **Magic Guard.** Magic Guard's `onDamage` returns `false` for any non-Move effect; Life Orb's damage source is the item, not the move, so it's blocked. PS: `data/abilities.ts:2421-2425`.
  - **`move.flags['futuremove']` (Future Sight, Doom Desire).** Same `:531` gate excludes futuremoves — they don't trigger the user's Life Orb on resolution turn either, because the user isn't the active source.
  - **The user fainted before the hook.** Sheer Force isn't required; if the user fainted from recoil-on-hit (Brave Bird against a Sturdy + Rocky Helmet target etc.), the hook runs but `source.damage()` against a fainted mon does nothing. Specifically PS's `damage()` no-ops on fainted.
  - **The user is force-switching after the move (Volt Switch, U-turn).** PS gates on `!source.forceSwitchFlag` — Life Orb recoil from a U-turn hits before the switch though? No: U-turn's `selfSwitch` is processed *after* `onAfterMoveSecondarySelf`, so Life Orb does fire on U-turn (no `forceSwitchFlag` set yet at recoil time). The `forceSwitchFlag` gate is specifically for moves like Eject Button-triggered swaps; normal `selfSwitch` users still take Life Orb recoil. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Life_Orb>.
  - **The move hit nothing (every target immune / protected / missed).** PS computes `move.totalDamage` and `move.hitTargets`; `onAfterMoveSecondarySelf` still runs but with no damage delivered. Practically Life Orb's `data/items.ts:3408` check `source !== target` is always true and the recoil applies even if all targets were immune in many builds. Per current PS source, recoil still fires — this is a known surprise. Cartridge behaves the same way per Bulbapedia.
- **Substitute-absorbed hits.** Sub takes the whole damage (`HIT_SUBSTITUTE`) — but the move still counts as having connected, so Life Orb recoil fires. PS: `data/moves.ts:18380` returns `HIT_SUBSTITUTE`, which is treated as a successful hit for the purposes of `AfterMoveSecondarySelf`.
- **Spread moves in doubles.** Life Orb fires once per move, not per target.

### Engine implementation notes

- Recoil is already implemented (Phase 2 PR-16, see commit log).
- The Sheer Force interaction must be added when Sheer Force lands — gate the recoil hook on `!(move.has_sheer_force && holder.ability == SheerForce)`.
- Magic Guard interaction lives in the indirect-damage dispatcher; the user-side Life Orb damage call must route through that path so MG can short-circuit it.
- Future Sight gate: `!move.flags.contains(FutureMove)` — clean once future-move plumbing exists.

---

## 5. Magic Guard

### What it is

Holder only takes damage from direct attacks. All "indirect" damage sources are skipped. The implementation is uniform: an `onDamage` hook returning `false` whenever `effect.effectType !== 'Move'`.

### The interactions

PS: `data/abilities.ts:2420-2426`:
```
onDamage(damage, target, source, effect) {
  if (effect.effectType !== 'Move') {
    if (effect.effectType === 'Ability') this.add('-activate', source, 'ability: ' + effect.name);
    return false;
  }
}
```

- **Blocked:** Life Orb recoil, Burn DOT, Poison DOT, Toxic DOT, Sandstorm chip, Hail/Snow chip (gen 9 Snow doesn't chip, so moot), Spikes layers on switch-in, Stealth Rock on switch-in, Curse self-damage from the ghost user, Leech Seed *outgoing* damage to the seeded target (Leech Seed siphons via `damage` event — blocked), Bind/Wrap/Clamp/Whirlpool partial-trap chip, Nightmare chip, Bad Dreams DOT, recoil from recoil moves (Brave Bird, Flare Blitz, Wood Hammer, etc.), High Jump Kick crash damage, Mind Blown / Steel Beam HP cost, Belly Drum HP cost (yes — see below).
- **Recoil moves: yes, fully blocked.** Magic Guard on a Brave Bird user takes no recoil. PS treats recoil as `this.damage(amount, source, source, 'recoil')` → effectType 'Effect' (not Move), so MG returns false. Bulbapedia confirms: <https://bulbapedia.bulbagarden.net/wiki/Magic_Guard_(Ability)>.
- **High Jump Kick / Jump Kick crash:** blocked.
- **Belly Drum / Substitute / Curse HP costs.** These go through `directDamage` for Substitute, which... actually `directDamage` ignores `onDamage` hooks in PS — so Magic Guard does **NOT** block the cost of *creating* a Substitute. Same for Belly Drum. PS: Substitute's `onHit(target) { this.directDamage(target.maxhp / 4); }` (`data/moves.ts:18334-18336`). Bulbapedia agrees: HP costs of using Belly Drum and Substitute are taken normally under Magic Guard.
- **NOT blocked:**
  - **Sticky Web speed drop.** Sticky Web uses `boost({ spe: -1 }, …)`, not damage. PS: `data/moves.ts:17962`. Magic Guard's `onDamage` doesn't fire — speed drop applies. Clear Body / White Smoke would block it, but MG does not.
  - **Status itself.** Magic Guard prevents *damage from* Burn / Poison / Toxic, but does NOT prevent being burned, paralysed, etc. A burned MG mon still has its Attack halved (gen 9 physical burn cut still applies in PS — see `data/conditions.ts:brn` `onModifyAtk`). Bulbapedia: same.
  - **Confusion self-hit.** Confusion damage is a typeless physical "Move"-category attack on self; `effectType === 'Move'` so MG does NOT block it. Verify in `data/conditions.ts:confusion`.
  - **Direct attack damage.** By definition.
  - **Pain Split.** Implements via `setHP` style change, not `damage` event — Magic Guard doesn't apply (Pain Split goes through anyway).
  - **Destiny Bond, Perish Song.** Both bypass `onDamage` (Destiny Bond uses `faint()` directly, Perish Song uses `faint()`). MG does not save you.

### Engine implementation notes

- Single hook: in the indirect-damage application path (currently scattered — burn DOT in `battle.rs` residuals, recoil in `damage.rs`, items in `item.rs`), funnel through a `try_apply_indirect_damage(target, amount, source_effect)` that consults the holder's ability.
- Substitute-creation cost and Belly Drum cost must go through a *direct* damage path that ignores `onDamage` (matches PS's `directDamage` semantics).
- Confusion self-hit must be classified as a Move-category damage event, not indirect.
- Deferred until: most DOT sources still pending. The funnel API is the prerequisite.

---

## 6. Substitute

### What it is

Costs 1/4 max HP, creates a decoy with `floor(maxHP/4)` HP. Most incoming damage and almost all status effects are absorbed by the sub; certain moves and abilities bypass it.

### The interactions

- **Cost.** `this.directDamage(target.maxhp / 4)` — not gated by `onDamage`, so Magic Guard does NOT prevent the cost. Fails (no HP loss) if user already has Sub, or HP ≤ 1/4 maxHP, or maxHP === 1 (Shedinja). PS: `data/moves.ts:18324-18336`.
- **What bypasses Sub:**
  - **`move.flags['bypasssub']`** — sound moves (Boomburst, Hyper Voice, Round, Disarming Voice, etc.), Roar, Whirlwind, Dragon Tail, Circle Throw (force-switch), Encore, Taunt, Torment, Confide, Block, Mean Look, Curse (Ghost), Spite, Heart Swap, Aromatic Mist, Crafty Shield's coverage, and a handful more. Greppable via `bypasssub: 1` in `data/moves.ts`.
  - **`move.infiltrates`** — set by Infiltrator ability via `onModifyMove`. PS: `data/abilities.ts:2086-2090`. Note: Infiltrator only bypasses Substitute and screens, not all sub-blocks.
  - **Force-switch moves** — Whirlwind, Roar etc. are flagged `bypasssub: 1` AND have `forceSwitch: true`. Even without the flag, force-switch logic ignores Sub because it operates on the side's queue, not as direct damage.
  - **Self-targeting moves** — `if (target === source) return;` in `onTryPrimaryHit` (`data/moves.ts:18352`). The user can use Calm Mind / Recover / Belly Drum / etc. behind their own Sub freely.
  - **Future Sight / Doom Desire.** The delayed hit lands on whatever Sub is up *at the moment it resolves* — not the sub when the move was queued. If the original sub broke and a new one is up, the new one absorbs. If no sub, full damage to mon. PS: futuremoves resolve via standard move flow on their resolution turn, going through the current `onTryPrimaryHit`.
- **What does NOT bypass Sub:**
  - **Critical hits.** Crits still go through Sub (Sub HP eaten faster, but mon protected). PS: damage is computed normally then capped by sub HP. No special crit handling.
  - **Z-moves through Protect** — moot in gen 9 (no Z-moves).
  - **Tera blast etc.** — no special flag.
- **Status through Sub.** Sub blocks status moves targeting the foe entirely because the move's effect is gated behind a successful hit, and the hit is absorbed by Sub which returns `HIT_SUBSTITUTE`. Toxic, Thunder Wave, Spore, Will-O-Wisp, Stun Spore, Glare, Poison Powder, Sleep Powder — all fail against a sub'd target unless the move has `bypasssub: 1`. PS: the `onTryPrimaryHit` returns `null` for status moves where `getDamage` returns falsy and there's no sub-bypass flag. Self-status moves (Rest, Refresh) unaffected since they target self.
- **Knock Off behind Sub.** Damage IS reduced/applied to Sub normally. Item removal is gated on `target.takeItem()` after the hit — but `onAfterHit` runs after `onTryPrimaryHit` returned `HIT_SUBSTITUTE` (a successful-hit sentinel). Per PS, `takeItem` is called regardless of sub. **However**, since gen 5, item removal from Knock Off is blocked while Sub is up — verify: PS Knock Off (`data/moves.ts:9977`) calls `target.takeItem()` unconditionally, but `takeItem` runs the `TakeItem` event which does NOT have a sub-check. So PS allows Knock Off to remove items through Sub? Investigation: cartridge says NO, items cannot be Knock Off'd behind Sub. PS does match cartridge for this case because the Knock Off `onBasePower` and `onAfterHit` only fire if the move actually hit the *Pokémon* — when Sub absorbs (`HIT_SUBSTITUTE` return), `move.hitTargets` excludes the absorbed target in the `onAfterHit` dispatch. So in practice Knock Off behind Sub: damage to Sub, no item removal. **vgc-engine matches PS per docs/AGENTS.md rule 6** — and our PR-25 already implements the Sub-blocks-item-removal rule.
- **Counter / Mirror Coat behind own Sub.** The damage on the Sub still registers as `lastDamage` and can fuel Counter/Mirror Coat (the user took 0 HP but `lastDamage` was set). PS: `source.lastDamage = damage;` at `data/moves.ts:18365`.
- **Drain moves into Sub.** Drain is computed off the *Sub damage* and the attacker heals normally. PS: `data/moves.ts:18375-18377`.
- **Recoil into Sub.** Recoil also computed off Sub damage. PS: same block, line 18372.
- **AfterSubDamage event.** Some abilities/items trigger here (e.g. Rocky Helmet does NOT — Rocky Helmet uses `onDamagingHit` on the *target*, and the target took 0 HP, but PS still fires `AfterSubDamage`). Worth auditing each defender contact effect against this event.
- **Multi-hit moves through Sub.** Each hit checks Sub anew. If Sub breaks on hit 2, hits 3-5 land on the Pokémon. PS handles this via the per-hit loop in `moveHit`.

- **Curse against a sub'd Ghost target.** Ghost-Curse costs 1/2 user HP and sets a Curse volatile on the target. The volatile-application step targets the foe; Sub does NOT block volatile application from Curse because PS routes Ghost-Curse through a path that checks `bypasssub`. Verify: PS Curse `flags` include `bypasssub: 1` in gen 7+. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Curse_(move)>.
- **Leech Seed against a sub'd target.** Leech Seed does NOT have `bypasssub: 1`, so Sub blocks the seed-application against a sub'd foe. The hit fails entirely. Conversely a seeded mon that LATER puts up Sub still drains HP every turn (residual damage routes around Sub for already-applied volatiles).
- **Multi-hit moves vs Sub HP cap.** If Sub has 10 HP left and a multi-hit move's first hit would do 30, the hit caps at 10 (breaking the Sub), and the remaining hits go through to the Pokémon. Each hit re-checks `hasOwnProperty('substitute')`. PS: per-hit loop.
- **Disguise vs Sub.** Distinct mechanics, can stack (Mimikyu can have a Sub up while Disguise is intact — bust order: Disguise busts on first physical hit, Sub absorbs damage after Disguise busts? No: Disguise's `onTryHit` runs at higher priority and converts the hit to 1/8 max HP self-damage. Sub never sees the hit. So Disguise consumes first. After Disguise busts, future hits go through Sub normally.
- **Substitute survives ability suppression?** Yes — Sub is a volatile, not an ability.
- **Belly Drum behind Sub.** Belly Drum requires HP > 1/2 maxHP. The Sub costs 1/4. So with Sub up at full HP minus 1/4, the user has 3/4 maxHP. Belly Drum costs 1/2 maxHP additional, leaving 1/4. PS: Belly Drum allowed. Sub HP untouched (it's separate).

### Engine implementation notes

- Substitute volatile already on the books; PR-25 added the Knock Off block.
- Add a `move.bypass_sub: bool` and `move.infiltrates: bool` to the move struct.
- Sub absorption belongs in the hit-resolution path in `battle.rs` immediately before damage application, mirroring PS's `onTryPrimaryHit`.
- Future Sight queue must check the current Sub state at resolution, not queue time.
- Audit defender-side reactive triggers: which fire on `AfterSubDamage` vs only on real HP loss.

---

## 7. Protosynthesis / Quark Drive

### What it is

The Paradox abilities. When the relevant condition is active (sun for Protosynthesis, Electric Terrain for Quark Drive) OR the holder uses up Booster Energy on switch-in, a volatile is added that locks in the holder's *best* base stat and boosts it: ×1.3 to Atk/Def/SpA/SpD, ×1.5 to Spe.

### The interactions

- **Best-stat selection algorithm.** Computed via `pokemon.getBestStat(false, true)` — first arg is `unboosted` (false → boosted), second is `unmodified` (true → ignores item/ability multipliers). Wait, PS source `data/abilities.ts:3494` calls `getBestStat(false, true)`. The args are `(unboosted, unmodified)`. So `unboosted = false` means stat boosts are *included*; `unmodified = true` means item/ability modifiers are *excluded*. **This contradicts the common belief that stat stages are ignored.** Investigation against PS: `getStat(i, unboosted, unmodified)` at `sim/pokemon.ts:656-668`. So the comparison uses **stat stages included**, items/abilities excluded. Bulbapedia describes it as "highest stat" without specifying. Cartridge per dataminers: stages included. **vgc-engine matches PS** (which matches cartridge per current consensus).
- **Tie-breaking order.** The for-loop iterates `['atk', 'def', 'spa', 'spd', 'spe']` and uses strict `>` comparison: `if (this.getStat(i, ...) > bestStat)`. So in a tie, the **earliest** in iteration order wins — atk > def > spa > spd > spe. PS: `sim/pokemon.ts:659-665`.
- **Booster Energy trigger.** Booster Energy is consumed on switch-in if its holder has Protosynthesis/Quark Drive AND the relevant weather/terrain is NOT active. Otherwise weather/terrain wins (no consumption). PS: see Booster Energy item and the ability's `onStart` + condition `onStart`. The volatile carries `fromBooster: true` so it does NOT auto-end when weather/terrain expires. PS: `data/abilities.ts:3477` (`if (!pokemon.volatiles['protosynthesis']?.fromBooster && !this.field.isWeather('sunnyday')) { pokemon.removeVolatile('protosynthesis'); }`).
- **Weather/terrain expiring mid-battle.** If the volatile came from weather/terrain (not Booster), it ends when conditions end. If from Booster, it persists indefinitely until switch-out or ability suppression. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Protosynthesis_(Ability)>.
- **Sun re-applies on top of a Booster volatile?** No — the volatile is already up, `addVolatile` is idempotent. The locked stat is whatever was best at the moment of first activation.
- **Order of precedence for trigger** at switch-in:
  1. `onSwitchInPriority: -2` runs the ability's `onStart`.
  2. `onStart` checks weather → adds volatile (best stat computed *now*).
  3. If weather not active, on the same switch-in, the Booster Energy item's hook fires (later in the same step) and adds the volatile with `fromBooster: true`.
  PS: `data/abilities.ts:3469-3479`.
- **Locked stat does not re-pick if sun expires and a new Booster Energy is somehow consumed mid-battle.** It can't — Booster Energy is one-shot. But if sun expires and re-activates (e.g. Drought switch), the volatile re-applies via `WeatherChange` and the best-stat is recomputed at that moment using current stat stages.
- **Ignored when ability suppressed.** Each `onModifyXxx` checks `pokemon.ignoringAbility()` and bails. So Neutralizing Gas / Gastro Acid / Mold Breaker (status moves only, abilities only) all suppress the boost.
- **`flags: { failroleplay: 1, noreceiver: 1, noentrain: 1, notrace: 1, failskillswap: 1, notransform: 1 }`** — cannot be copied, swapped, traced, transformed. PS: `data/abilities.ts:3530`.

- **Ability swap edge cases.** Trace cannot copy Protosynthesis / Quark Drive (`notrace: 1`). Skill Swap fails (`failskillswap: 1`). Role Play fails (`failroleplay: 1`). Entrainment fails (`noentrain: 1`). Receiver does not inherit on faint (`noreceiver: 1`). Transform does not copy (`notransform: 1`).
- **Tera + Quark Drive / Protosynthesis.** Tera-ing a Paradox does NOT remove the volatile; the locked stat still gets boosted. The Tera type is a separate property and doesn't affect the best-stat calc.
- **Multiple Paradoxes on one side.** Each evaluates independently — no shared state.
- **Booster Energy on a Paradox that doesn't have Protosynthesis/Quark Drive.** Useless — the item only activates if the ability is the right one. PS: Booster Energy's `onUpdate` checks for the ability before consuming.

### Engine implementation notes

- `pokemon.rs`: add `get_stat(stat, unboosted: bool, unmodified: bool)` mirroring PS, and `get_best_stat(unboosted, unmodified)` with the strict-`>` tie-break iteration order atk→def→spa→spd→spe.
- `ability.rs`: Protosynthesis/Quark Drive ability stub records the locked stat in a per-mon volatile (`Volatile::Proto { stat: StatId, from_booster: bool }`).
- Booster Energy in `item.rs` must fire on switch-in *after* the ability's `onStart` checks weather, and only consume if no volatile is present.
- Mod hooks: `on_modify_atk/def/spa/spd`: ×1.3 (chainModify [5325, 4096] = 1.3); `on_modify_spe`: ×1.5 (chainModify 1.5).
- All four must gate on `!holder.ignoring_ability()`.
- Deferred until weather/terrain ability triggers are wired (Drought, Electric Surge etc. — needed for `WeatherChange` event analogue).

---

## 8. Knock Off

### What it is

Dark-type physical move. Base power 65 (gen 6+), boosted to ×1.5 (effective 97.5 BP) if the target has a removable item. After the hit, the target's item is removed (not consumed — it's gone for the battle).

### The interactions

- **Item removal blocked when:**
  - **Sticky Hold.** PS: `data/abilities.ts:4579-4587` — `onTakeItem` returns `false` if source != self OR `activeMove.id === 'knockoff'`. So even sticky-Hold holding their own item against Knock Off is protected. Mold Breaker bypasses Sticky Hold (`breakable: 1`).
  - **Target behind Substitute.** Per PS, Knock Off's `onAfterHit` runs only if the target was actually hit. When Sub absorbs the hit, `move.hitTargets` excludes the absorbed mon and `onAfterHit` doesn't fire on that mon. The damage portion still hits the Sub at boosted BP if the target had an item (the `onBasePower` check `singleEvent('TakeItem', …)` is what runs before the boost is applied — but Sub doesn't block the *check*, only the *removal*; on PS the boost still applies when the target has a removable item, even behind Sub). Bulbapedia confirms cartridge behaviour: damage boosted, item not removed.
  - **Target fainted from the hit.** `onAfterHit` runs but `target.hp <= 0` short-circuits `takeItem`.
  - **Attacker fainted before post-damage step** (e.g. from Rocky Helmet recoil putting attacker at 0). `onAfterHit` runs on the attacker's queue but `takeItem`'s event chain checks ability suppression on the attacker side — generally proceeds, but if attacker fainted PS skips the move's residual effects. Investigation: `onAfterHit` is part of the `moveHit` flow; PS does run it for fainted attackers in some cases. Conservative match: skip item removal when attacker fainted between damage and onAfterHit.
  - **Item is unremovable.** Mega stones held by a Mega-evolving species, Plates held by Arceus when used in Multitype contexts, Drives held by Genesect, Memories held by Silvally, Z-crystals (gen 7 only), griseousorb on Giratina-O, primal orbs (Red/Blue Orb) on Groudon-Primal/Kyogre-Primal. In gen 9 the relevant cases are Plates/Arceus (Arceus locked-form items can't be Knock Off'd), and any species-locking item.
  - **Items with `onTakeItem: false`.** Greppable: `data/items.ts:230`, `:692` etc. These are the form-mandatory items.
- **Damage boost applies even if item won't be removed?** The `onBasePower` hook runs `singleEvent('TakeItem', …)` first to check if removal would succeed; if `false` (Sticky Hold, form-locked items), no ×1.5 boost. PS: `data/moves.ts:9970-9976`.
- **Knock Off + Magician (attacker ability) etc.** Magician steals after damage. With Knock Off, Knock Off removes the item first (`onAfterHit` on the move). Magician's `onAfterMoveSecondarySelf` runs later — but the target no longer has the item. So Knock Off + Magician = item destroyed, attacker gets nothing. (Unlikely combo since Magician users prefer their own item-stealing moves.)
- **Mold Breaker + Knock Off + Sticky Hold.** Mold Breaker suppresses Sticky Hold → item is removed (and damage boost applies).
- **PR-25 status:** the vgc-engine codebase already blocks Knock Off item removal behind Substitute. Cross-checked against PS — matches.

- **Knock Off doesn't affect the user.** Self-cast Knock Off (theoretically via Magic Coat-like reflection) is gated by `target === source` returns in `onAfterHit`.
- **Knock Off + Symbiosis.** If an ally has Symbiosis and holds an item, Symbiosis passes its item when the holder loses theirs. Knock Off triggering Symbiosis: the target loses their item to Knock Off → Symbiosis fires → target now holds the Symbiosis-mon's item. Knock Off does NOT re-trigger; the move's `onAfterHit` only takes the item once.
- **Knock Off vs Klutz / Embargo.** The target still loses the item even if they can't use it (Klutz / Embargo'd / Magic Room). These conditions affect *whether the item activates*, not whether it can be removed.
- **Item lost via Knock Off does NOT come back.** Permanent for the battle. Distinct from Pluck / Bug Bite (consume berries — also gone).
- **Held items that are consumed during the same turn as Knock Off.** Order matters. If a Sitrus Berry triggers from the Knock Off damage and is consumed before `onAfterHit`, Knock Off finds no item and doesn't fire the announce. PS event ordering: damage → `onAfterMoveSecondary` (berry consumption) → move's own `onAfterHit`. Verify: PS processes berry consumption inside `onUpdate` after the hit, which runs before `onAfterHit`. So a Sitrus-triggering Knock Off does not see the berry to remove.

### Engine implementation notes

- Knock Off is implemented (PR-17, recent commits show PR-25 polish). The boost-and-remove logic lives in the move's `on_base_power` and `on_after_hit` hooks.
- The `take_item` API must consult: (a) defender ability (Sticky Hold), (b) form-locked items list (item has `unremovable: true` flag), (c) Sub volatile.
- Mold Breaker integration: route Sticky Hold check through the breakable-ability path described in §2.

---

## 9. Speed Boost

### What it is

At the end of each turn, the holder's Speed rises by 1 stage — but only if they were on the field at the start of that turn (gen 5+).

### The interactions

- **Switch-in turn skipped.** The check is `if (pokemon.activeTurns) { boost({ spe: 1 }); }`. `activeTurns` increments at the *start* of the turn after `onStart`, so on the switch-in turn `activeTurns === 0` at residual time. PS: `data/abilities.ts:4408-4415`. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Speed_Boost_(Ability)>.
- **Fires regardless of whether the mon acted.** Asleep, frozen, paralysed-skipped, flinched, fully attracted-immobilised — all still trigger Speed Boost at residual. PS hook is `onResidual` with no action-check. Verified — no `lastMove` or `movedThisTurn` gate.
- **Stops at +6.** Standard `boost` failure when already maxed; ability silently no-ops.
- **Disabled when ability is suppressed.** Gastro Acid, Neutralizing Gas, Mold Breaker (status moves only) — `onResidual` runs through the ability dispatcher which respects `ignoringAbility`.
- **Pivot moves (U-turn, Volt Switch).** If a Speed Boost mon uses U-turn, they leave the field before residuals. Speed Boost does NOT fire that turn (the switch-out preempts residuals for that slot). The replacement Pokémon, whatever it is, does not get the boost — Speed Boost is per-mon volatile state.
- **Baton Pass.** Baton Pass passes stat boosts but not the ability. The receiver gets the +Spe stages already accrued, but does not gain Speed Boost (unless they also have it).

- **Trick Room.** Trick Room inverts speed order but does NOT change actual Speed stat. Speed Boost still raises the stat; this matters when TR ends (faster mon goes first as usual).
- **Tailwind.** Stacks multiplicatively with Speed Boost. Tailwind is a side condition with ×2 to Spe applied as `onModifySpe`.
- **Quark Drive + Speed Boost.** Both active: Speed Boost provides stages (multiplicative with the QD ×1.5 base). PR-25-level math applies normally.
- **Order of residual events.** Speed Boost has `onResidualOrder: 28, onResidualSubOrder: 2`. Same order class as Moody (28/1) and other "end-of-turn ability triggers". PS residual ordering is canonical; document if implementing residual queue.

### Engine implementation notes

- Implemented (per commit log). Verify the residual gate is `active_turns > 0` not "moved this turn".
- Test case: status-locked Blaziken (sleep, freeze) still gains Speed each turn.
- Residual queue must respect `onResidualOrder` / `onResidualSubOrder` for determinism vs PS.

---

## 10. Choice items (Band / Specs / Scarf)

### What it is

Lock the holder into the first move they select, until they switch out (or the item is removed). Band/Specs add ×1.5 to Atk/SpA respectively; Scarf adds ×1.5 to Spe.

### The interactions

- **Lock mechanism.** When the holder uses any move with `isChoice` item active, PS adds the `choicelock` volatile via the move's resolution path (specifically when a move other than `struggle` is selected). The volatile records `effectState.move = activeMove.id`. PS: `data/conditions.ts:324-345`.
- **Skipped lock:**
  - `activeMove.hasBounced` (the move was reflected — no lock for the originator).
  - `activeMove.sourceEffect === 'snatch'` (Snatch-stolen move).
- **Selecting a different move next turn.** Move attempt with `move.id !== effectState.move && move.id !== 'struggle'` → `addMove; attrLastMove('[still]'); debug("Disabled by Choice item lock");` and the move fails. PS: `data/conditions.ts:332-345`. No PP lost. **In practice this should be prevented at choice-selection time by the UI/agent** — vgc-engine should disallow the choice in `legal_actions()`.
- **Locked move runs out of PP.** Holder uses Struggle. PS does not explicitly handle this in choicelock — Struggle is the fallback when no move has PP. The `choicelock` condition allows Struggle through (`move.id !== 'struggle'` is in the conjunction).
- **Encore + Choice on the same target.** Encore forces a specific slot via `onOverrideAction`. Choice locks via `onBeforeMove`. Resolution order matters:
  - If Encore's locked move == Choice's locked move: no conflict, mon uses that move.
  - If Encore's locked move != Choice's locked move: **Encore's override happens earlier in the action pipeline** (selects the move) than `choicelock`'s `onBeforeMove` (which would disable a non-Choice move). The mon uses Encore's move, and PS does NOT count this as a Choice lock violation because the override replaces the user's selection — `choicelock.onBeforeMove` then checks the *current* move against the recorded one. If they mismatch, the move fails via the choicelock gate. **End result on PS: Encore + Choice mismatch causes Struggle** (the action becomes failure → Struggle fallback). Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Choice_Band>, <https://bulbapedia.bulbagarden.net/wiki/Encore_(move)>. Verify edge: simulate a Choice Band Gholdengo locked on Make It Rain, Encored into Shadow Ball — PS produces Struggle.
- **Disable + Choice.** If Choice locks onto a move that Disable then disables, holder uses Struggle. The disable-checks run before move resolution. Same logic.
- **Imprison.** Same outcome.
- **Choice on switch-in clears stale `choicelock`.** `onStart` in each Choice item removes any leftover `choicelock` volatile. PS: `data/items.ts:965-970` for choiceband.
- **Item knocked off / Tricked / Switcheroo'd.** `choicelock.onBeforeMove` checks `if (!pokemon.getItem().isChoice) { pokemon.removeVolatile('choicelock'); return; }`. So if you lose your Choice item mid-battle, the lock evaporates immediately. PS: `data/conditions.ts:332-336`.
- **Gigaton Hammer / Blood Moon (consecutive-turn-locked moves).** Selecting on a Choice user: the move is allowed once; on the next turn the holder is Choice-locked into the same move BUT the move's own can't-use-consecutively gate fails it. Result: Struggle. (Gigaton Hammer text: "Can't be used twice in a row.")

### Engine implementation notes

- `item.rs`: Choice items already implemented for stat boost (PR-19) and lock (PR-20).
- `choice.rs` / move-selection: must filter out non-locked moves from `legal_actions()` when `choicelock` volatile present. Single source of truth for the lock.
- Encore is not yet implemented. When it lands: priority of `onOverrideAction` runs before `onBeforeMove` checks; mismatch → Struggle path. Match PS exactly.
- Strugglefallback path: needs the no-PP and disabled-conflict cases. Currently a stub.

---

## 11. Intimidate

### What it is

On switch-in, lowers each adjacent foe's Attack by 1 stage. In doubles, both foe slots are checked independently.

### The interactions

- **Per-target evaluation in doubles.** The PS `onStart` loop `for (const target of pokemon.adjacentFoes())` evaluates each foe; the `-ability` announce fires once (via the `activated` flag) but the boost call goes per target. PS: `data/abilities.ts:2148-2161`.
- **Substitute blocks.** Per PS `data/abilities.ts:2156-2158` — explicit `if (target.volatiles['substitute']) { this.add('-immune', target); }` short-circuit. Sub mon takes nothing. This is in the Intimidate code, not the generic Sub bypass list.
- **Blocked by Clear Body / White Smoke / Full Metal Body.** Generic `onTryBoost` hook filters all stat drops including Intimidate's. PS Clear Body: `data/abilities.ts:513-527`. White Smoke and Full Metal Body identical pattern.
- **Blocked by Hyper Cutter.** Only filters Attack drops. PS: `:1895-1908`.
- **Blocked by Inner Focus.** Explicitly checks `effect.name === 'Intimidate'`. PS: `:2108-2121`. Does NOT block other Attack drops (those are still allowed).
- **Blocked by Oblivious (gen 8+).** Same pattern as Inner Focus. PS shows the block under `oblivious:` though gen 9 may handle differently; greppable.
- **Blocked by Scrappy.** Same explicit-Intimidate-name check. PS: `:4034-4049`.
- **Blocked by Own Tempo.** Same.
- **Triggers Rattled.** Rattled's `onAfterBoost` checks `effect?.name === 'Intimidate' && boost.atk` and adds +1 Spe. PS: `:3726-3741`. Note: this fires even if the Attack drop was *blocked* by another ability? No — `onAfterBoost` runs after `onTryBoost`; if the boost was deleted, `boost.atk` is undefined and Rattled no-ops. So Clear Body Rattled would block both the Atk drop AND the Spe boost. Bulbapedia confirms.
- **Guard Dog (gen 9).** Inverts: instead of -1 Atk, gives +1 Atk via `onTryBoost` deleting the drop and calling `boost({ atk: 1 })`. PS: `data/abilities.ts:1682-1696`. **Deferred in vgc-engine.**
- **Defiant / Competitive.** Trigger on stat drop from an opponent: +2 Atk (Defiant) or +2 SpA (Competitive). Both fire on Intimidate-induced drop. They do NOT fire if Inner Focus / Clear Body / etc. prevented the drop. They DO fire if Rattled fires (Rattled doesn't block the drop, just adds Spe).
- **Mirror Armor.** Reflects the Intimidate drop back at the user. Implements as `onTryBoost` redirecting to source. PS: `data/abilities.ts:2659~`.
- **Mold Breaker / Teravolt / Turboblaze does NOT bypass these blockers** — Intimidate is an ability-triggered effect, not a move. Mold Breaker only affects defender abilities for *the user's damaging moves*. So an Intimidate user does NOT bypass Clear Body via Mold Breaker.

### Engine implementation notes

- `ability.rs`: Intimidate's `on_switch_in` hook iterates `adjacent_foes` and calls `try_lower_boost(target, Stat::Atk, 1, source: Intimidate)`.
- The boost-attempt path must run an `on_try_boost` event chain on the target so each blocking ability can short-circuit.
- Sub check is hard-coded inside Intimidate's `on_switch_in` (matches PS), not a generic Sub block.
- Guard Dog: deferred (note in CLAUDE.md / next-up list).
- Rattled / Defiant / Competitive / Mirror Armor: deferred but plumbing must support an `on_after_boost` event downstream of the boost application.

---

## 12. Terastalization (Tera)

### What it is

Gen 9's mid-battle type change. The user spends their once-per-battle Tera and assumes their Tera type for the rest of the battle. Affects STAB, type matchups, and a few ability interactions.

### Tera is Phase 3 work in `docs/PLAN.md`. The following is reference, not implementation.

- **STAB rules change.** Before Tera: STAB ×1.5 on moves matching the species's original type(s). After Tera:
  - If the Tera type matches one of the original types: ×2.0 STAB on that type (was 1.5).
  - If the Tera type doesn't match original types: ×1.5 STAB on the Tera type AND the original type(s) retain ×1.5 STAB on moves matching them. So a Water-Tera Garchomp gets ×1.5 on Water *and* ×1.5 on Ground/Dragon.
- **Adaptability + Tera.** Normally Adaptability turns STAB into ×2.0. With Tera:
  - Tera type matches original: ×2.25 (not ×2.0 — the Tera +0.5 stacks).
  - Tera type doesn't match original: ×2.0 on original types (Adaptability), ×2.0 on Tera type (1.5 + 0.5 Tera bonus, NOT ×2.25). Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Terastal_phenomenon>.
- **Defensive type changes.** After Tera, the user is only the Tera type for purposes of incoming moves. Any double-type weaknesses from the original typing are gone. Hidden-by-type abilities (Levitate on a Ground-Tera mon) — Levitate is an ability, not a type, so it persists.
- **Tera Blast.** Becomes the user's Tera type and switches category to physical if Attack > SpA (post-modifiers).
- **Tera Stellar.** Special case — boosts every type once, after which the boost ends per-type. Distinct mechanic; defer.

### Engine implementation notes

- Deferred until Phase 3. Currently no `tera_type` field on `Pokemon` (or there is one but unused).
- When added: STAB computation in `damage.rs` must consult both `original_types` and `tera_type` post-tera, and apply the correct multiplier from the matrix above.
- Adaptability stacking is a one-liner in the STAB formula.
- Phase 2 turn-agreement gate (80%) does not require Tera since replay corpus can be filtered.

---

## 13. Aurora Veil

### What it is

Snow-conditional screen. Halves damage from both physical and special attacks against the user's side for 5 turns (8 with Light Clay). Stacks multiplicatively with Reflect/Light Screen? No — same multiplier applied once via the `auroraveil` condition's modifier; the condition's `onAnyModifyDamage` early-returns if Reflect/Light Screen also active.

### The interactions

- **Setup requires Snow at the moment of move use.** `onTry()` returns `this.field.isWeather(['hail', 'snowscape'])`. PS: `data/moves.ts:840-842`. If snow ends after Aurora Veil is up, the veil persists.
- **Duration is fixed.** 5 turns base, 8 with Light Clay. PS: `data/moves.ts:843-850`. No re-check against weather; the side condition's `onSideStart`/`onSideEnd` track a turn counter.
- **Modifier applied in `onAnyModifyDamage`.** Halves damage in singles (×0.5), ×2732/4096 (~0.667) in doubles when more than one mon is on each side (PS uses `this.activePerHalf > 1`). PS: `data/moves.ts:851-862`. The check is `target !== source && this.effectState.target.hasAlly(target)` — Aurora Veil protects allies, not the user themselves… wait, re-reading PS: `hasAlly(target)` returns true when target is an ally of the veil's side, which INCLUDES the user since you're your own ally per PS's `hasAlly`. Verify: `pokemon.ts` `hasAlly` impl. In practice Aurora Veil protects the entire side, including the setter.
- **Doesn't stack with Reflect/Light Screen** — explicit early-return in the condition body: if the matching screen is also up, the Aurora Veil modifier doesn't apply (the per-category screen handles it). Bulbapedia confirms.
- **Brick Break / Psychic Fangs / Defog (foe) destroy it.** Brick Break breaks Reflect/Light Screen — and Aurora Veil. Psychic Fangs (gen 7+) breaks all three. Defog removes screens including Aurora Veil. PS: each move has explicit code clearing the side conditions.
- **Crits ignore screens including Aurora Veil.** Standard. PS: `damage` flow checks `move.willCrit` and skips screen modifiers.
- **Infiltrator ignores Aurora Veil.** Same `move.infiltrates` flag as for Sub.

### Engine implementation notes

- Side condition with a fixed turn counter; not weather-coupled after setup.
- `damage.rs`: screen modifier consults Aurora Veil only when the matching per-category screen is absent.
- `weather.rs`: Snow lookup at move-use time only.
- Deferred until: screens are implemented.

---

## 14. Sleep / Freeze edge cases

### What it is

The two "skip-your-turn-entirely" status conditions. Sleep has 1-3 turn duration (PS: `random(2, 5)` exclusive upper → 2, 3, or 4 selected, but `time--` runs once before the act-check so effective sleep turns is 1-3). Freeze is permanent until a 20% per-turn thaw check, fire-type moves on self, or being hit by a fire-type move.

### The interactions

- **Sleep timer decrement is in `onBeforeMove`.** PS: `data/conditions.ts:67-76`. So the timer only ticks when the mon attempts to act. **A flinched-asleep mon does NOT decrement** — flinch's `onBeforeMove` runs at higher priority and returns false before sleep's `onBeforeMove`. Wait: verify priorities. Sleep's `onBeforeMovePriority: 10` (high). Flinch's check is in volatile `flinch`, also `onBeforeMove`. Order: in PS, sleep is evaluated first (priority 10), increments timer, decides wakeup-or-skip. Verify by reading: Yes, sleep's `time--` runs unconditionally before any other `onBeforeMove`. So timer ticks even when flinched (the mon "tried to act"). Bulbapedia agrees.
- **Sleep Talk / Snore set `move.sleepUsable = true`.** When the sleep mon's `onBeforeMove` would skip the action, it allows Sleep Talk / Snore to proceed. The timer still decrements. PS: `data/conditions.ts:77-`.
- **Early Bird.** Halves remaining timer by decrementing twice per turn. `if (pokemon.hasAbility('earlybird')) pokemon.statusState.time--;` THEN the regular `time--`. PS: `data/conditions.ts:68-71`. Bulbapedia: gen 5+ behaviour.
- **Insomnia / Vital Spirit.** Block sleep at `try_set_status`. Both have `breakable: 1` — Mold Breaker can bypass. PS: see `insomnia:` ability — `onSetStatus` returns false for `slp`.
- **Sleep Clause.** Smogon metagame rule, NOT engine. Do not implement in core. Format layer can enforce.
- **Switching does not reset the sleep timer.** Sleep persists across switches with current timer.
- **Rest sets sleep to 2 turns (acts on turn 3).** PS: Rest's `condition` overrides sleep timer to fixed 2. Bulbapedia confirms.
- **Freeze thaw is RNG-only at action time.** 20% per turn. Frozen mons with a Fire-type move attempt: thaw and execute. Frozen mons hit by a Fire move (other than Hidden Power Fire in some gens): thaw. Scald / Steam Eruption etc. thaw via explicit `thawsTarget: true` flag. PS: `data/moves.ts` various moves with `thawsTarget`.
- **Frostbite (gen 9 Hisui replacement?)** — Frostbite is **not** in gen 9 main games; Ice still uses Freeze. Hisui (Legends Arceus) has frostbite but Scarlet/Violet doesn't.

### Engine implementation notes

- `pokemon.rs`: `Status::Sleep { turns_remaining: u8 }`. Decrement inside the pre-move dispatch in `battle.rs`, before flinch/paralysis-skip checks (or at higher priority).
- Early Bird: ability hook in the pre-move dispatch decrements twice.
- Sleep Talk / Snore: special-case in `legal_actions()` — when sleep volatile present, allow only `sleepUsable` moves through.
- Insomnia / Vital Spirit: `try_set_status` returns Err for Sleep when defender has either.
- Rest sets `turns_remaining = 2`.
- Freeze: 20% RNG check at action time, thaw on Fire-typed move use (including hit-by).
- Sleep Clause: NOT in core. Format module (`format.rs`) is the right home if ever wanted.

---

## 14b. Paralysis / Burn / Poison / Toxic supplementary edges

- **Paralysis full-paralysis chance.** 25% to skip a turn (gen 7+). Cartridge had 25% since gen 7; pre-gen-7 was the same in modern PS but earlier gens used different mechanics in mods. PS: `data/conditions.ts:par:` `onBeforeMove` rolls and returns false 25% of the time.
- **Paralysis speed cut.** ×0.5 to Speed (gen 7+; was ×0.25 pre-gen-7). PS handles in `par:` `onModifySpe`. Quick Feet ignores the cut AND boosts ×1.5.
- **Electric types immune to paralysis (gen 6+).** Implemented at `try_set_status`. Bulbapedia confirms gen 6+ change. vgc-engine should gate at status-application time.
- **Burn Atk cut.** ×0.5 to Attack (physical). Guts boosts ×1.5 and ignores the cut. PS: `data/conditions.ts:brn:` `onModifyAtk`. Special attack unaffected — Facade BP doubles only when status is present (handled in Facade's `onBasePower`).
- **Fire types immune to burn.** Cannot be burned by anything except Tri Attack / Scald / Steam Eruption / Burning Jealousy — wait, no: Fire types are fully immune to burn at type-immunity check level, regardless of source. PS: `data/conditions.ts:brn:` `onStart` checks `target.hasType('Fire')` and refuses. Same for Will-O-Wisp. Bulbapedia confirms.
- **Toxic damage scales.** Turn 1: 1/16. Turn N: N/16, capped after 15 turns at 15/16 (essentially full HP). The counter is on the volatile, not the status itself. Toxic Spikes inflicts regular Poison from one layer, Toxic from two layers. Poison-type hitting Toxic Spikes absorbs both layers.
- **Toxic Orb / Flame Orb activation.** Activate at end-of-turn residual. Status-immune holders (Fire Type → Flame Orb is wasted) get nothing. Magic Guard holders take no damage but DO get the status (Guts / Toxic Boost / Flare Boost / Marvel Scale fuel). PS items: `data/items.ts:flameorb:`, `:toxicorb:`.
- **Sleep grounds Yawn?** Yawn applies a pre-sleep volatile; if the target switches before resolution, the volatile clears.
- **Static / Effect Spore / Flame Body etc. percentage.** Static: 30% para on contact. Effect Spore: 30% chance of one of {par, slp, psn} on contact. Flame Body: 30% burn on contact. PS: each ability's `onDamagingHit`. None of these are blocked by Sheer Force (defender-side).

### Engine implementation notes

- `pokemon.rs`: status-immunity matrix in `try_set_status` — gate by type and ability.
- Toxic counter is a per-mon counter that resets on switch-out. Volatile or part of the status struct.

## 15. Other surprises (free-form)

A grab-bag of edge cases that have historically bitten competitive sims. Each is short — verify against PS before implementing.

- **Trick / Switcheroo + Choice item.** The recipient becomes Choice-locked into whatever move they next use. The trickster (now with the recipient's old item) is NOT locked, because Choice items' `onStart` clears `choicelock` only on the user. PS: `data/items.ts` choiceband `onStart`. Practically: if you Trick a Choice Scarf onto a non-Choice mon, they get locked the next turn. Tricking AWAY a Choice item also clears the trickster's lock because the choicelock onBeforeMove checks `!pokemon.getItem().isChoice` and removes the volatile.
- **Sticky Hold blocks Trick / Switcheroo too.** Same `onTakeItem`. Mold Breaker bypasses.
- **Status moves with priority + Psychic Terrain.** Psychic Terrain blocks priority moves against grounded targets — INCLUDING Prankster-boosted status moves targeting grounded foes. So Prankster Thunder Wave fails into a grounded mon on Psychic Terrain. Self-targeted Prankster moves are unaffected (no target). PS: Psychic Terrain condition.
- **Queenly Majesty / Dazzling / Armor Tail.** Block all priority moves targeting an ally on their side, including Prankster. These are `breakable` — Mold Breaker bypasses.
- **Tera Shell.** All super-effective moves against a full-HP Tera Shell holder become neutrally effective. `breakable: 1` so Mold Breaker bypasses. Only triggers at full HP — if the holder has taken any damage, the modifier doesn't apply. PS: search `terashell:` in `data/abilities.ts`.
- **Disguise (Mimikyu).** First hit does 1/8 max HP self-damage (cartridge gen 7 was full damage; PS matches gen 8+ change). `breakable: 1`. Doesn't trigger on status moves. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Disguise_(Ability)>.
- **Ice Face (Eiscue).** Similar: physical attacks are blocked by busting the helmet, transforming into Noice Face. Only triggers on the first physical hit. `breakable: 1`. Recovers when Snow is active.
- **Trace.** Copies a random foe's ability on switch-in. Fails to copy `notrace: 1` flagged abilities (Protosynthesis, Quark Drive, Trace itself, Receiver, Power of Alchemy, etc.). PS: `data/abilities.ts:trace:` and the `notrace` flag list.
- **Dancer.** Triggers on any Dance move used by anyone (including self?? No, excluding self). The Dancer's copy of the move uses the original target. Cannot copy Dancer's own dance (loop prevention). Affected by Mold Breaker if the original Dance was boosted? No — Dancer copies the move's effect; the copy goes through its own ability checks.
- **Multi-hit moves with Skill Link / Loaded Dice.** Skill Link forces all 5 hits. Loaded Dice forces minimum 4 hits (95% chance to actually hit 4-5). Both interact with Population Bomb and Triple Axel cleanly.
- **Population Bomb misses.** Each hit has independent accuracy (90%). Loaded Dice changes it to 4-10 hits, ignoring per-hit accuracy. PS: `data/moves.ts:populationbomb:`.
- **Wide Lens.** ×1.1 accuracy. Stacks multiplicatively with Compound Eyes.
- **Stealth Rock with Heavy-Duty Boots.** Heavy-Duty Boots negates all entry hazards (Stealth Rock, Spikes, Toxic Spikes, Sticky Web). PS: `data/items.ts:heavydutyboots:`. Magic Guard does NOT block Sticky Web speed drop (only damage hazards) — but Heavy-Duty Boots does.
- **Defog clears your OWN side's screens too** (and removes terrain in gen 8+). Court Change is the swap version.
- **Magic Bounce timing on Stealth Rock.** Magic Bounce reflects SR back at the user's side — but only if SR has `reflectable: 1`. Yes, SR is reflectable. The reflected SR lands on the user's side.
- **Multi-attack abilities and Sheer Force.** Triple Axel / Triple Kick: are they secondary-bearing? Bullet Punch etc.: no secondaries. Verify each before assuming Sheer Force interacts.
- **Tera Stellar — Stellar STAB.** Once per type per battle, ×2.0 STAB-equivalent. Tracked per-type on the Pokemon. Defer.
- **Booster Energy used pre-switch-in?** Cannot. Item activates only when ability's switch-in `onStart` confirms no triggering weather/terrain. PS: handled inside the ability's volatile `onStart`.
- **Good as Gold.** Blocks all status moves targeting the holder. Listed as `breakable` — Mold Breaker on status moves bypasses (rare, but Prankster Mold Breaker Sableye lore). Bulbapedia.
- **Purifying Salt.** All Ghost-type moves do ×0.5 damage to the holder, and the holder cannot be statused. `breakable: 1`.
- **Tera Blast Stellar.** Hits all 18 types at × ?? — actually Stellar Tera Blast does ×1.2 multiplier and only "Stellar" type once per type used. Highly mod-able; defer.
- **Last Respects.** BP scales with allies fainted (×50 per faint, capped at 1000). Includes pre-fainted allies that never came in. Sub-targeted: no special interaction. PS: `data/moves.ts:lastrespects:`.
- **Rage Fist.** BP scales with times hit this battle (×50 per hit, capped at 350). Counter resets only on switch-out. PS: `data/moves.ts:ragefist:` and a `timesAttacked` counter on `Pokemon`.
- **Comatose (Komala).** Treated as permanently asleep — but still acts every turn. Immune to other status. NOT `breakable` (has `cantsuppress: 1`). Mold Breaker does NOT affect it.
- **Neutralizing Gas.** Suppresses all other abilities while the holder is on the field. PS: `sim/pokemon.ts:864-882` (`ignoringAbility` consults `neutralizinggas` ability). `cantsuppress: 1` itself. On switch-out the suppression ends and switch-in `onStart` hooks fire for affected mons (e.g. Intimidate re-triggers, weather sets re-fire). This re-fire is a famous PS quirk that matches cartridge.
- **Gastro Acid.** Single-target ability suppression via volatile. Same rules as Neutralizing Gas but per-mon. Cannot suppress abilities with `cantsuppress: 1` flag.
- **Imposter / Transform.** Copies the target's species, types, stats (using user's HP), ability, moves (with 5 PP each). Does NOT copy item. Does not copy `notransform: 1` abilities (Multitype, Disguise, RKS System, Power Construct, Ice Face, Protosynthesis, Quark Drive).
  - **The copied ability's `onStart` FIRES on transform (gen 5+).** `transformInto` calls `setAbility(target.ability, ..., isTransform=true)` (`sim/pokemon.ts:1358`), and `setAbility` runs `singleEvent('Start', ...)` when `!isTransform || oldAbility.id !== ability.id` (`sim/pokemon.ts:1946-1949`). Under a transform `isTransform` is true, so onStart fires **iff the copied ability differs from the transformer's own**. Concretely: a Ditto-Imposter that becomes Mawile/Incineroar (or a Transform-move user onto Landorus-T) **DOES fire the copied Intimidate** and drops the foes' Attack. The engine mirrors this in `Battle::transform_into` (re-fires `fire_intimidate` when the acquired ability is Intimidate and changed). Other copied onStart abilities (Download, weather setters, …) via transform are not yet re-fired — future work.
- **Skill Swap.** Cannot swap `failskillswap: 1` abilities. Includes Wonder Guard, Multitype, Stance Change, Schooling, Comatose, Zen Mode, Protosynthesis, Quark Drive, RKS System, Power Construct, Ice Face.
- **Court Change.** Swaps both sides' side-conditions: Reflect, Light Screen, Aurora Veil, Mist, Safeguard, Tailwind, Stealth Rock, Spikes, Toxic Spikes, Sticky Web. Does NOT swap weather, terrain, room effects, or Wonder Room / Magic Room. PS: `data/moves.ts:courtchange:` for the explicit list.
- **Booster Energy and Eject Pack interaction.** Eject Pack triggers on stat drop. Booster Energy holders eat their item on switch-in if no terrain/weather; that activation does NOT cause a stat drop (it's a boost). No interaction.
- **Eject Button / Red Card / Eject Pack timing.** All resolve after damage-dealing hits. Eject Button on the defender forces them out (replacement chosen end of turn). Red Card forces the *attacker* out. Eject Pack forces holder out on stat drop. None of these triggers chain into Intimidate / hazards until the replacement actually switches in, which may not be until later in the same turn or next turn depending on PS phase ordering.
- **Loaded Dice + Population Bomb.** Skill Link forces 5 hits in legacy multi-hit; Population Bomb without Loaded Dice rolls per-hit 90% accuracy. With Loaded Dice: minimum 4 hits, ignoring accuracy. PS: `data/moves.ts:populationbomb:` and `data/items.ts:loadeddice:`.
- **Tera Shell + recoil moves.** Tera Shell reduces super-effective damage at full HP. If the user has Brave Bird recoil, the damage applied to the Tera Shell mon is reduced (gen 9's neutral cap), but the recoil to the user is computed from the *damage dealt* — so recoil is also reduced. Bulbapedia notes this.
- **Quick Claw.** 20% chance to act first within priority bracket. RNG check at action time. Does NOT override actual priority brackets (a +1 priority move beats a Quick Claw 0-priority).
- **Custap Berry.** Acts first within priority bracket when holder is at ≤25% HP. Consumed on activation. Same priority semantics as Quick Claw.
- **Air Balloon.** Grounded-immunity to Ground moves; popped on any hit (any move that connects). Once popped, gone for the battle. Stealth Rock damage on switch-in does NOT pop Air Balloon (because Air Balloon makes its holder ungrounded → SR still hits based on the Rock vs holder typing; for typing purposes Air Balloon doesn't change types; SR damage *does* hit and is calculated, but doesn't pop the balloon per PS). Verify: PS `data/items.ts:airballoon:` `onDamagingHit` pops it. Stealth Rock damage routes through `damage` not `onDamagingHit` so balloon survives SR. Bulbapedia confirms this is also cartridge behaviour.
- **Heavy-Duty Boots and Sticky Web.** HDB blocks Sticky Web speed drop too, not just damage hazards. PS: `data/items.ts:heavydutyboots:` checks all entry hazards in the side condition's `onSwitchIn`.
- **Tera Stellar Tera Blast** has a quirk: it's the user's *original primary type* + Stellar bonus, NOT the user's Tera type. Specifically Tera Blast on a Stellar Tera user becomes Normal-typed (Tera Blast's default before any Tera type assignment) with Stellar STAB applied. Defer.
- **Glaive Rush.** User locked into a follow-up "vulnerability": next hit against them is a guaranteed hit AND deals double damage. Volatile cleared at end of next turn. PS: `data/moves.ts:glaiverush:`.
- **Spit Up / Swallow / Stockpile.** Stockpile increments a counter (max 3), boosts Def/SpD. Spit Up / Swallow consume the stack. Stockpile counter is separate from Tera and unaffected by ability suppression.
- **Salt Cure.** Inflicts a volatile that does 1/8 max HP per turn (1/4 for Water and Steel types). Skipped by Magic Guard (it's residual damage routed through `damage`).
- **Syrup Bomb.** -1 Spe per turn for 3 turns after the hit. The volatile checks the *source* exists each turn; if the source switched out, the volatile ends early. PS: `data/moves.ts:syrupbomb:` condition.
- **Doodle.** Copies the target's ability to the user AND their ally. Fails on `failreceiver: 1` / similar.

---

## 16. Damage formula edges

A non-exhaustive list of damage-pipeline corners that have bitten implementations. The PS pipeline is roughly: `getDamage(source, target, move)` → base damage → STAB → type-effectiveness → burn → screens → other modifiers (Life Orb, Tinted Lens, Filter/Solid Rock, weather, terrain, etc.) → random factor → cap at target HP. Each modifier is a `chainModify` call with a 4096-fixed-point factor for engine determinism.

- **Order of operations matters.** Modifier order is fixed in PS — and the cumulative product is computed via integer chain-modify rounding (`x = (x * num + 2048) >> 12`). Reordering changes single-digit damage values that break replay determinism. Match PS modifier order exactly.
- **STAB after Tera.** STAB is conditioned on attacker types; with Tera, the attacker has potentially two type sets. PS computes STAB by checking `attacker.hasType(move.type)` and `attacker.terastallized && move.type === attacker.terastallized`. The result is the higher of the two STAB tiers per the table in §12.
- **Type chart with abilities.** Type effectiveness is computed by looping over `target.getTypes()` and accumulating multipliers from `typechart`. Abilities like Levitate, Flash Fire, Water Absorb, Volt Absorb, Sap Sipper, Storm Drain, Lightning Rod, Earth Eater, Well-Baked Body, Thermal Exchange, Wind Rider apply BEFORE the type chart — they're type-immunity overrides that short-circuit damage entirely (and may trigger a side-effect like SpA boost). All `breakable: 1` except Magic Guard (which doesn't change type interactions anyway).
- **Tinted Lens.** Attacker-side ×2 to damage on resisted hits. PS: `data/abilities.ts:tintedlens:`. NOT `breakable` (it's attacker-side).
- **Filter / Solid Rock / Prism Armor.** Defender-side ×0.75 to super-effective damage. Filter and Solid Rock are `breakable`; Prism Armor (Necrozma) is `cantsuppress: 1` and NOT `breakable`. So Mold Breaker bypasses Filter/Solid Rock but not Prism Armor.
- **Multiscale / Shadow Shield.** ×0.5 to incoming damage at full HP. Multiscale is `breakable`; Shadow Shield (Lunala-Dawn-Wings, Necrozma forms) is `cantsuppress: 1`.
- **Fluffy.** ×0.5 contact damage, ×2 Fire damage. Modifiers stack: a Fire contact move does ×1 (canceled out). `breakable: 1`.
- **Spread damage in doubles.** Moves targeting "all adjacent foes" or "all adjacent" get ×0.75 damage in doubles only. PS uses `move.spreadHit` flag at damage time. Singles-mode: no reduction.
- **Critical hits.** ×1.5 (gen 6+, was ×2 in gen 5). Ignore positive defender Def/SpD stages and negative attacker Atk/SpA stages. Also ignore screens (Reflect/Light Screen/Aurora Veil). Sniper boosts crit damage to ×2.25.
- **Same-side ally damage (Helping Hand).** ×1.5 to the ally's damaging move that turn. Stacks with itself? No — multiple Helping Hands on the same target re-apply the volatile but the multiplier doesn't stack (volatile is boolean-ish).
- **Burn cut applied where.** ×0.5 to Attack stat during physical damage calc. Facade ignores the cut AND doubles BP. PS: `data/conditions.ts:brn:` `onModifyAtk` returns half except for Guts holders.
- **Weather damage modifiers.** Sun: Fire ×1.5, Water ×0.5. Rain: Water ×1.5, Fire ×0.5. Utility Umbrella negates these for the holder. Cloud Nine / Air Lock suppress weather effects (but not the weather itself).
- **Terrain damage modifiers.** Electric Terrain: Electric ×1.3 for grounded users. Grassy Terrain: Grass ×1.3 for grounded users, Earthquake/Magnitude/Bulldoze ×0.5 to grounded targets. Misty Terrain: Dragon ×0.5 against grounded targets. Psychic Terrain: Psychic ×1.3 for grounded users, blocks priority into grounded.
- **Stealth Rock damage scales by Rock-vs-target.** 1/8 max HP for 1× weak, 1/16 for 0.5× resist, etc. Bug × Ice × Flying × Fire = ×4 weak → SR does 1/2 max HP. Heavy-Duty Boots negates.
- **Multi-hit moves and crit.** Each hit rolls independently for crit. PS: per-hit damage calc.
- **Multi-hit moves and ability triggers.** Each hit triggers `onDamagingHit` on the defender. Rocky Helmet damages the attacker per hit. Static / Flame Body / Effect Spore percentage rolls per hit. Skill Link / Loaded Dice users vs. Rocky Helmet take a LOT of recoil.
- **Parental Bond (Kanga-Mega; gen 6+).** Mostly historical for gen 9 (no Megas in current format). Skip.

### Engine implementation notes

- `damage.rs`: damage modifier chain must use fixed-point 4096 with round-half-up to match PS's `chainModify`.
- All multipliers go through a single `chain_modify(value: u32, numerator: u32, denominator: u32) -> u32` helper.
- Modifier order is canon — encode as an explicit pipeline in `damage.rs`, not as scattered hooks.
- Spread reduction is a flag on the resolved move, applied late in the chain.

## 17. Move-priority brackets

Worth surfacing because Prankster, Quick Claw, Custap, Quick Draw, and the priority moves interact in non-obvious ways.

- **Priority brackets** (gen 9): +5 (Helping Hand) > +4 (Magic Coat / Snatch) > +3 (Detect / Protect / Endure / etc.) > +2 (Extreme Speed, Feint) > +1 (Quick Attack, Bullet Punch, Ice Shard, Mach Punch, etc.) > 0 (default) > -1 (Vital Throw) > -3 (Focus Punch) > -4 (Avalanche, Revenge) > -5 (Counter, Mirror Coat) > -6 (Roar, Whirlwind, Circle Throw, Dragon Tail) > -7 (Trick Room).
- **Within a bracket, Speed decides.** Quick Claw / Custap Berry can promote within bracket. Prankster moves a status move from 0 to +1 (or whatever to +1; Prankster is `priority + 1`).
- **Stall (ability).** Stall reverses speed order within bracket — fastest goes last. Implemented via a flag in the priority compare.
- **Trick Room.** Inverts speed comparison globally. Stacks with Stall (both reverse → fastest first again).
- **After You / Quash.** Force a specific actor to move first/last within their bracket regardless of speed.
- **Quick Draw (gen 8+).** 30% chance per turn to move first within bracket on damaging move use. Similar to Quick Claw.
- **Mycelium Might.** Status moves move LAST in their priority bracket AND ignore abilities. Forces -6.something effective ordering for status moves only.

### Engine implementation notes

- `order.rs` is the right home. Already exists; verify priority enumeration matches PS's full bracket list.
- Quick Claw / Custap / Quick Draw / Stall: per-mon flag affecting compare function within bracket.
- Trick Room: side-condition flag flipping the speed comparator.

## 18. Weather and terrain abilities

The "setter" abilities and their interactions form a tangled web. Brief reference:

- **Drought** (sun), **Drizzle** (rain), **Sand Stream** (sand), **Snow Warning** (snow): 5 turns base, 8 with the matching rock (Heat Rock, Damp Rock, Smooth Rock, Icy Rock). Set on switch-in via `onStart`. Most-recent setter wins if multiple weathers fire same turn — PS resolves in switch-in order (speed-decided).
- **Primordial Sea / Desolate Land / Delta Stream** (Primal Kyogre/Groudon/Mega-Rayquaza). Cannot be overridden by ordinary weather. End only when the holder switches out or loses the ability. These are NOT in gen 9 main format (no Primal Reversion in S/V); listed for completeness.
- **Cloud Nine / Air Lock.** Suppress weather *effects* (damage, speed mods, type boosts) but the weather itself persists. Sun-Drought + Cloud Nine: weather is sun, but Fire moves are not boosted. When the Cloud Nine mon switches out, sun effects resume mid-battle.
- **Utility Umbrella.** Holder ignores sun and rain effects on themselves. Does NOT suppress for others. Stacks: Cloud Nine globally + Utility Umbrella locally → redundant.
- **Electric Surge, Grassy Surge, Misty Surge, Psychic Surge, Hadron Engine, Orichalcum Pulse.** Set respective terrain on switch-in. Hadron Engine = Electric Terrain + special SpA boost in terrain. Orichalcum Pulse = sun + special Atk boost in sun. Both are unique to Iron Bundle... no, Iron Treads / Koraidon / Miraidon.
- **Terrain duration.** 5 turns base, 8 with Terrain Extender. Same as weather rocks.
- **Terrain ends Misty/Electric blocks status.** Electric Terrain: prevents sleep for grounded mons. Misty Terrain: prevents all status for grounded mons. Psychic Terrain: blocks priority moves into grounded. Grassy Terrain: heals 1/16 max HP per turn for grounded mons.
- **Grounded check.** A mon is grounded unless: Flying type, Levitate ability, Air Balloon, Magnet Rise volatile, Telekinesis volatile. Roost temporarily removes Flying type (so a Flying mon under Roost is grounded for the rest of the turn). Iron Ball grounds the holder (overrides Levitate / Flying type). Gravity grounds everyone globally.
- **Weather and abilities interaction.** Chlorophyll / Swift Swim / Sand Rush / Slush Rush double Speed in matching weather. Solar Power: ×1.5 SpA in sun, -1/8 HP per turn. Rain Dish: +1/16 HP per turn in rain. Ice Body: +1/16 HP per turn in snow/hail. Dry Skin: -1/8 HP in sun, +1/8 HP in rain. Sand Force: ×1.3 to Rock/Ground/Steel moves in sand. Sand Veil / Snow Cloak: +20% evasion in matching weather.
- **Forecast (Castform).** Changes type and form based on weather: Fire in sun, Water in rain, Ice in snow/hail. Reverts to Normal when weather ends. PS: `data/abilities.ts:forecast:`. `breakable: 1`.
- **Flower Gift (Cherrim).** In sun: ×1.5 Atk and SpD for self and allies. `breakable: 1`.
- **Protosynthesis / Quark Drive.** See §7.
- **Tera Stellar Tera Blast in sun?** Sun doesn't affect Stellar typing.

### Engine implementation notes

- `weather.rs` already exists. Verify it tracks setter and duration.
- Terrain needs an analogous `terrain.rs` with the same duration semantics.
- Grounded check is a Pokemon method that consults type, ability, item, and volatiles.
- Cloud Nine / Air Lock: the *weather check* method must consult presence of any suppressor on field. The weather doesn't end, but `is_weather_active()` returns false for damage-calc purposes.

## 19. PS quirks and known divergences from cartridge

Documented for the engineer who Googles a behaviour and finds conflicting answers. **vgc-engine matches PS in every case below.**

- **Anger Point's chance to trigger.** PS triggers on any crit. Cartridge always triggers on crit. Match.
- **Mummy / Lingering Aroma on Fling.** PS does not trigger Mummy on Fling because Fling's `flags` doesn't include `contact: 1`. Cartridge: same.
- **Knock Off vs species-locked items.** PS treats Plates on Arceus (gen 4) as un-Knock-Offable. Gen 9 Arceus is past-only; moot.
- **Magic Bounce ordering on bounce.** PS's reflected move uses the current battle state (re-checks Sub etc.), not the state at bounce time. Cartridge: same.
- **Fake Out priority interaction with Quick Claw.** Fake Out is +3; Quick Claw within bracket. So Quick Claw can promote a Fake Out user above another +3 user (rare). Cartridge: same.
- **Stomping Tantrum's failed-last-move detection.** PS checks `lastMove?.id` for misses or full-paralysis-skipped moves. Switching out resets. Cartridge: same.
- **Last Respects scaling.** PS scales BP by faint count on user's side, including the user's own previous faints if recalled via Revival Blessing? Revival Blessing is gen 9 — PS does count revived-then-fainted-again mons multiple times? Verify: PS's `lastrespects:` reads `pokemon.side.totalFainted`, which is monotonic. Cartridge: same. So revived-and-re-fainted is counted twice.
- **Endure surviving 1 HP.** PS: returns 1 HP if damage would faint. Cartridge: same.
- **Sturdy at full HP.** Same as Endure but ability-gated, single-trigger per switch-in window. Cartridge: same.
- **Healing Wish / Lunar Dance restore.** Heal received mon to full + cure status (+ restore PP for Lunar Dance). PS resolves on the user's switch-out, with the replacement chosen normally. Cartridge: same.
- **Court Change weather/terrain swap.** Court Change does NOT swap weather/terrain (see §15 list). Cartridge: same.
- **Pollen Puff vs ally Substitute.** Pollen Puff against an ally is a heal. With ally Sub up: heal still goes through (Sub doesn't block heals from allies). Cartridge: same.

## 20. Phase 2 turn-agreement implications

`docs/PLAN.md` Phase 2 gate is 80% turn agreement with PS replays. Most divergences will come from:

1. **Damage formula rounding** — chain-modify integer math must match exactly.
2. **Modifier ordering** — see §16.
3. **RNG advancement order** — when do we advance the RNG for crit roll vs damage roll vs accuracy roll? PS does accuracy → damage → crit (or crit before damage, depending on the move).
4. **Residual ordering** — see §9 note. Weather damage, status DOT, item activation, ability triggers all have a canonical order.
5. **Switch-in ordering** — multiple switch-ins on the same turn are speed-sorted before any `onStart` fires. Multiple `onStart` events from the same switch-in evaluate in declared order.
6. **Move failure cascades** — when a move fails for one reason, does PS still consume PP / trigger Pressure / trigger choicelock / etc.?

Each of these is a likely source of 1-2% turn-disagreement when running against PS replay corpus. The 80% gate is forgiving but each item above is worth a small investigation when the engine sits at ~75-80% agreement.

## 21. Ability quick-reference table (gen 9 priority items)

For abilities not yet in the engine but likely needed for Phase 2/3 coverage. Sorted by likely implementation priority.

| Ability | What it does | Breakable? | Notes |
|---|---|---|---|
| Levitate | Ground immunity | Yes | Iron Ball / Gravity / Smack Down override |
| Sturdy | Survive 1 HP at full HP | Yes | One-shot per switch-in |
| Multiscale | ×0.5 damage at full HP | Yes | Shadow Shield = same, not breakable |
| Magic Guard | Indirect-damage immunity | No | See §5 |
| Wonder Guard | Only super-effective hits land | Yes | Shedinja only realistically |
| Filter / Solid Rock | ×0.75 to super-effective | Yes | |
| Tinted Lens | ×2 to resisted moves | No | Attacker-side |
| Adaptability | STAB ×2 (×2.25 w/ matching Tera) | No | Attacker-side, see §12 |
| Technician | ×1.5 to BP ≤ 60 moves | No | Pre-modifier BP check |
| Iron Fist | ×1.2 to punch moves | No | Punch flag list |
| Tough Claws | ×1.3 to contact moves | No | |
| Sheer Force | See §3 | No | |
| Mold Breaker / Teravolt / Turboblaze | See §2 | No (`cantsuppress`) | |
| Flash Fire | Fire immunity → +Fire damage | Yes | Stat-up flavour |
| Water Absorb / Volt Absorb | Type immunity → heal 1/4 | Yes | Storm Drain / Lightning Rod same, also redirect |
| Sap Sipper | Grass immunity → +1 Atk | Yes | |
| Earth Eater | Ground immunity → heal 1/4 | Yes | New in gen 9 |
| Well-Baked Body | Fire immunity → +2 Def | Yes | New in gen 9 |
| Wind Rider | Wind moves → +1 Atk (immune) | Yes | Tailwind also boosts |
| Purifying Salt | Status immunity, ×0.5 Ghost dmg | Yes | New in gen 9 |
| Good as Gold | Status-move immunity | Yes | Mold Breaker bypasses |
| Tera Shell | Super-effective → neutral at full HP | Yes | Terapagos only |
| Beads of Ruin / Sword of Ruin / Tablets of Ruin / Vessel of Ruin | -×0.75 to a stat for all others | No | Treasures of Ruin (Wo-Chien / Chien-Pao / Ting-Lu / Chi-Yu) |
| Quark Drive / Protosynthesis | See §7 | No | |
| Hadron Engine | Sets Electric Terrain; ×1.333 SpA in it | No | Miraidon |
| Orichalcum Pulse | Sets sun; ×1.333 Atk in sun | No | Koraidon |
| Cud Chew | Re-eat berry next turn | No | Trivial |
| Supreme Overlord | ×1.1 per fainted ally up to ×1.5 | No | Kingambit |
| Toxic Debris | Sets Toxic Spikes when hit physically | No | Glimmora |
| Lingering Aroma | Replaces attacker ability with Lingering Aroma on contact | No | Mummy clone |
| Mycelium Might | Status moves last in bracket + ignore abilities | No | Toedscool/Toedscruel |
| Sharpness | ×1.5 to slicing moves | No | Slicing-flag list |
| Mind's Eye | Hit Ghost with Normal/Fighting + ignore evasion | Yes | Scrappy + Keen Eye combo |
| Anger Shell | At 50% HP from hit: drop Def/SpD, raise Atk/SpA/Spe | No | Klawf |
| Costar | Copy ally stat boosts on switch-in | No | Flamigo |
| Cotton Down | Lower all foes' Spe on hit | No | Eldegoss |
| Pickpocket | Steal attacker's item on contact (if no own item) | No | Liepard / Weavile |
| Symbiosis | Pass item to ally when ally's item is consumed | No | |
| Dauntless Shield / Intrepid Sword | +1 Def / Atk on switch-in (once per battle in gen 9) | No | Zacian / Zamazenta |
| Beast Boost | +1 to highest stat on KO | No | UB-series; tie-break same as Protosynthesis |
| Soul-Heart | +1 SpA on any KO | No | Magearna |
| Magician | Steal target's item after hit | No | Attacker keeps |
| Gluttony | Eat berry at 50% HP not 25% | No | |
| Unburden | ×2 Speed after losing item | No | |
| Frisk | Reveal foe's item on switch-in | No | Info only |

This table is provisional — verify each entry against PS as it's implemented.

## 22. Item quick-reference (gen 9 priority items)

| Item | Effect | Skip conditions |
|---|---|---|
| Choice Band / Specs / Scarf | ×1.5 to Atk / SpA / Spe + lock move | See §10 |
| Life Orb | ×1.3 damage, 10% recoil | See §4 |
| Assault Vest | ×1.5 SpD, no status moves | Implemented PR-19/20 |
| Eviolite | ×1.5 Def/SpD if can evolve | NFE check |
| Leftovers | +1/16 HP per turn | Magic Guard receives still — heals work |
| Black Sludge | +1/16 for Poison types, -1/16 otherwise | |
| Sitrus Berry | Heal 1/4 max HP at ≤ 50% | Gluttony triggers at higher % |
| Salac / Liechi / Petaya / Apicot / Ganlon Berries | +1 to respective stat at ≤ 25% (Gluttony 50%) | |
| Focus Sash | Survive 1 HP at full HP | One-shot |
| Air Balloon | Ground immunity; pops on hit | See §15 |
| Heavy-Duty Boots | Ignore entry hazards | All hazards including Sticky Web |
| Booster Energy | Activate Quark Drive / Protosynthesis | See §7 |
| Loaded Dice | Multi-hit min 4 hits | Excludes Triple Axel and a few others |
| Throat Spray | +1 SpA on sound move use | Single-use |
| Mirror Herb | Copy stat boosts from foe on switch-in | Single-use |
| Covert Cloak | Block secondary effects | New in gen 9 |
| Clear Amulet | Block stat drops | New in gen 9; like Clear Body |
| Ability Shield | Block ability suppression | New in gen 9 |
| Punching Glove | +10% BP to punch moves, removes contact | New in gen 9 |
| Rocky Helmet | -1/6 attacker HP on contact | Defender-side |
| Toxic Orb / Flame Orb | Status self at end of turn 1 | See §14b |
| Light Clay | Screens last 8 turns | Including Aurora Veil |
| Terrain Extender | Terrain lasts 8 turns | |

## 23. Move-attribute quick-reference

Frequently-referenced flags in PS move data. When implementing a move, copy ALL the flags from PS; many interactions key on flag presence.

- `contact: 1` — triggers Rocky Helmet, Static, Flame Body, Effect Spore, Cute Charm, Iron Barbs, Rough Skin, King's Rock, Tough Claws, Long Reach (cancels)
- `protect: 1` — blocked by Protect / Detect / Wide Guard / Quick Guard (per move)
- `mirror: 1` — copyable by Mirror Move
- `metronome: 1` — selectable by Metronome
- `bypasssub: 1` — bypasses Substitute
- `sound: 1` — bypasses Sub, blocked by Soundproof, boosted by Punk Rock, amplified by Throat Spray, blocked by Throat Chop
- `powder: 1` — fails against Grass / Overcoat / Safety Goggles
- `bullet: 1` — blocked by Bulletproof
- `pulse: 1` — boosted by Mega Launcher
- `punch: 1` — boosted by Iron Fist, Punching Glove
- `bite: 1` — boosted by Strong Jaw
- `slicing: 1` — boosted by Sharpness (gen 9)
- `wind: 1` — boosted by Wind Power / Wind Rider (gen 9)
- `dance: 1` — copyable by Dancer
- `gravity: 1` — disabled under Gravity
- `heal: 1` — disabled under Heal Block
- `nonsky: 1` — disabled in Sky Battles (no longer in gen 9)
- `defrost: 1` — thaws the user
- `reflectable: 1` — bouncable by Magic Bounce / Magic Coat
- `recharge: 1` — user skips next turn (Hyper Beam etc.)
- `failencore: 1` — Encore can't lock to this
- `failmimic: 1` — Mimic can't copy
- `futuremove: 1` — Future Sight / Doom Desire — special resolution path
- `cantusetwice: 1` — Gigaton Hammer, Blood Moon (gen 9)
- `snatch: 1` — can be Snatched
- `noassist: 1` — can't be selected by Assist

### Engine implementation notes

- Move struct in `damage.rs` or `pokemon.rs` must store full flag bitset.
- Each flag's downstream effect is a separate hook, but the flag-bitset is the source of truth.

## Cross-cutting concerns

A short index of effects that bypass multiple defender abilities/conditions in different ways, for quick lookup when implementing a new mechanic.

- **`breakable: 1` flag** → Mold Breaker / Teravolt / Turboblaze / Sunsteel Strike / Moongeist Beam / Photon Geyser / G-Max Drum Solo etc. bypass.
- **`bypasssub: 1` flag on the move** OR **`move.infiltrates` from Infiltrator** → bypass Substitute.
- **`bypassscreens` / `bypassallyscreens`** — no current gen-9 move; reserved.
- **`thawsTarget` / `thawsUser` flags** → thaw on Fire-typed moves.
- **`pranksterBoosted` move state** → triggers Dark-immunity check, lost on Magic Bounce reflection.
- **`hasSheerForce` move state** → skip `AfterMoveSecondarySelf` (Life Orb recoil etc.).
- **`forceSwitchFlag` on user** → skip Life Orb recoil (gates the item's own check, distinct from category-Status).

When PS and Bulbapedia disagree, the dominant pattern is that Bulbapedia describes cartridge intent while PS sometimes simplifies. Per `docs/AGENTS.md` rule 6, **vgc-engine matches PS**. Any divergence should be documented in this file as a topic update with the PS citation.

---

## 24. Glossary

For unfamiliar readers — terms used throughout this document.

- **`onStart`** — PS event fired when a Pokemon switches in or an effect first applies.
- **`onModifyMove`** — PS event during move resolution, before damage calculation.
- **`onModifyDamage`** — fires inside the damage chain to add a multiplier.
- **`onBasePower`** — fires earlier in the chain, modifies the base power of the move.
- **`onTryHit`** — pre-hit immunity / redirect / absorption check.
- **`onTryPrimaryHit`** — Substitute's hook; runs before the move's hit effects.
- **`onDamage`** — fires when any damage would be applied; Magic Guard hooks here.
- **`onDamagingHit`** — fires on the defender after a damaging hit lands (contact effects, etc.).
- **`onAfterHit`** — move-level hook after damage resolution.
- **`onAfterMoveSecondarySelf`** — runs after secondaries fired on the user; Life Orb recoil hooks here.
- **`onResidual`** — end-of-turn hook for status DOT, item heals, ability triggers (Speed Boost, Moody, etc.).
- **`onTryBoost`** — runs before a stat-stage change applies; Clear Body et al. hook here.
- **`onAfterBoost`** — runs after a successful stat-stage change; Rattled, Defiant, Competitive hook here.
- **`chainModify(num, denom)`** — PS's fixed-point modifier function. Multiplies by `num/denom` with rounding. Common values: `[5325, 4096]` = ×1.3 (Sheer Force, Life Orb), `[2732, 4096]` = ×0.667 (doubles spread reduction), `0.5`/`1.5`/`2` for whole-number multipliers.
- **Volatile** — a per-Pokemon condition that clears on switch-out (Substitute, Encore, Disable, Taunt, Curse, Leech Seed, partial trap, etc.). Distinct from status conditions (sleep, burn, poison, paralysis, freeze) which persist across switches.
- **Side condition** — applies to one side of the field (Reflect, Light Screen, Aurora Veil, Tailwind, Stealth Rock, Spikes, Toxic Spikes, Sticky Web, Mist, Safeguard, Wish, Healing Wish).
- **Pseudo-weather** — global field condition that's not weather (Trick Room, Magic Room, Wonder Room, Gravity, Fairy Lock, Ion Deluge).
- **Spread / spreadHit** — flag set on a move when it hits multiple targets in doubles, triggering the ×0.75 damage reduction.

## Sources

- Pokémon Showdown (`smogon/pokemon-showdown`) — shallow clone at `/tmp/pokemon-showdown-research`, current `master`.
  - `data/abilities.ts`, `data/moves.ts`, `data/items.ts`, `data/conditions.ts`
  - `sim/battle-actions.ts`, `sim/pokemon.ts`
- Bulbapedia — <https://bulbapedia.bulbagarden.net/wiki/>
- vgc-engine internal docs — `docs/AGENTS.md`, `docs/PLAN.md`, `docs/REFERENCES.md`
