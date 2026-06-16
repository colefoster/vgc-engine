# Engine missing-mechanic catalog

Source of truth for what the engine doesn't yet do. Each file lists per-mechanic descriptions and current status (`not implemented` / `partial — note` / `shipped — PR-N`). Updated as PRs land.

Cross-reference: `docs/PLAN.md` for the phase roadmap, `docs/interactions.md` for gen-9 mechanic edge cases that need research *before* implementation, `data/smogon-stats/2026-05/gen9championsvgc2026regma-1760.txt` for usage figures that justify priority.

## Format per entry

```
### Slug Name

**What it is**: One-sentence mechanic description.

**Why it matters**: Corpus / meta impact in one sentence (Smogon usage % where applicable).

**Depends on**: Other gaps that block this, or the data/code change required (omit when none).

**PS reference**: file:slug in the PS source clone at `/tmp/pokemon-showdown-research`.

**Status**: not implemented / partial — <note> / shipped — PR-N.
```

## Categories

### Systems — `systems.md`

Major structural gaps where no scaffolding exists yet. Each entry needs at minimum a new state field on `Pokemon` / `Side` / `Battle` plus a pipeline hook.

Covers: Terastallization, Mega Evolution, multi-turn charge moves, recharge moves, lock-in moves (Outrage), delayed moves (Future Sight), confusion volatile, Taunt / Disable / Torment / Heal Block / Imprison / Embargo, entry hazards (Stealth Rock / Spikes / Toxic Spikes / Sticky Web), hazard control (Rapid Spin / Defog / Court Change / Tidy Up), self-boost status moves (Swords Dance / Nasty Plot / Dragon Dance / etc.), counter-class (Counter / Mirror Coat / Metal Burst), force-switch, Pursuit, action-order manipulators (After You / Quash / Me First / Speed Swap), item / ability swap (Trick / Skill Swap / Gastro Acid), Recover-class heals, Wish, Pain Split / Endeavor, Memento / Healing Wish / Lunar Dance, Heal Pulse / Pollen Puff, Strength Sap, Yawn, Magic Coat / Snatch, Fling, Sleep Talk / Snore, Heal Bell, Perish Song, Copycat / Mimic / Sketch / Metronome family, plus pipeline refactors (per-source damage attribution, action queue, multi-turn move state, switch-in / end-of-turn ordering).

Approx 40 entries.

### Moves — `moves.md`

Per-slug move mechanics where structural support exists but the slug-specific arm is missing or partial. Excludes moves that depend on whole missing systems (those are in `systems.md`).

Covers: Tera Blast, Round, terrain-conditional power-ups (Misty Explosion / Expanding Force / Rising Voltage / Psyblade / Grassy Glide / Terrain Pulse), Triple Kick / Triple Axel per-hit BP ramp, Population Bomb per-hit accuracy, Acrobatics / Hex (shipped), Sucker Punch / Avalanche / Revenge (partial), Steel Beam / Mind Blown / Chloroblast max-HP recoil, Gigaton Hammer / Blood Moon lockout, Body Press (shipped), Foul Play (shipped), Photon Geyser / Shell Side Arm, Tri Attack, Fire/Ice/Thunder Fang dual-secondary, Scald target-thaw, Last Respects (shipped), Wave Crash, Heat Crash / Heavy Slam / Gyro Ball / Electro Ball / Low Kick / Grass Knot (weight/speed-scaled BP), Power Trip / Stored Power (shipped), Eruption / Water Spout / Dragon Energy (shipped), Weather Ball (shipped), Fake Out (shipped), OHKO moves (Fissure / Horn Drill / Guillotine / Sheer Cold), Encore (shipped), Focus Punch, Skill Link / Loaded Dice multihit interactions, Freeze-Dry / Flying Press / Thousand Arrows type-override moves.

Approx 30 entries.

### Abilities — `abilities.md`

Per-slug ability gaps. Dispatcher hooks already exist in `ability.rs` — most additions are a `match` arm.

Covers: damage modifiers (Adaptability, Supreme Overlord, Tough Claws, Strong Jaw, Mega Launcher, Iron Fist, Sniper, Sand Force, Solar Power, weather-speed abilities, Sand Veil / Snow Cloak, Toxic Boost / Flare Boost); defender modifiers (Filter / Solid Rock / Prism Armor, Multiscale / Shadow Shield, Fluffy, Heatproof / Water Bubble, Dry Skin, Punk Rock, Ice Scales); type immunity (Flash Fire, Lightning Rod / Storm Drain, Motor Drive, Volt / Water / Earth Absorb / Sap Sipper); Ruin abilities (Beads / Sword / Tablets / Vessel of Ruin); Embody Aspect (Ogerpon); on-hit triggers (Anger Shell, Anger Point, Berserk, Justified, Rattled, Steam Engine, Cotton Down, Wind Power, Wind Rider, Cursed Body); on-switch (Toxic Debris, Trace, Imposter, Disguise, Natural Cure, Pickpocket, Frisk); stat-drop blockers (Hyper Cutter / Clear Body / White Smoke / Full Metal Body / Big Pecks / Keen Eye / Inner Focus / Vital Spirit / Insomnia / Own Tempo / Sweet Veil / Limber / etc.); Unaware; status-as-boost (Guts / Quick Feet / Marvel Scale / Poison Heal); Soundproof / Bulletproof; doubles support (Friend Guard / Healer / Symbiosis / Telepathy / Damp); Pressure; passive penalties (Defeatist / Slow Start / Truant); type-swap (Color Change / Protean / Libero); Moody; Wonder Guard; Mummy / Wandering Spirit; Mirror Armor; Cute Charm / Effect Spore / Poison Touch / Liquid Ooze; Cloud Nine / Air Lock.

Magic Bounce flagged separately (Hatterene, top-50 mon).

Approx 60 entries.

### Items — `items.md`

Item-slot gaps. Smogon usage figures cited from the 2026-05 1760 corpus.

Covers: type-boost items (Mystic Water / Magnet / Black Belt / etc.), Expert Belt, Punching Glove, Wise Glasses / Muscle Band; Rocky Helmet, Eviolite, Heavy-Duty Boots, Safety Goggles, Ability Shield, Covert Cloak, Clear Amulet, Protective Pads; type-resist berries (Chople and the full set), status-cure berries (Lum / Cheri / etc.); Toxic Orb / Flame Orb / Black Sludge / Sticky Barb / Iron Ball; White Herb, Mental Herb, Power Herb, Throat Spray; Weakness Policy (status partial), Eject Button / Eject Pack / Red Card, Custap Berry; Light Clay, weather rocks (Damp / Heat / Smooth / Icy), Terrain Extender; Scope Lens / Razor Claw; Wide Lens, Bright Powder, Quick Claw, Lagging Tail; Shell Bell; Ogerpon masks; Adamant / Lustrous / Griseous Orb; Soul Dew.

Approx 35 entries.

### Architecture — `architecture.md`

Internal engine refactor debt that blocks mechanic clusters. Pure-Rust debt — no behavioral gap by itself.

Covers: volatile-system refactor, Tera state field, multi-turn move state, switch-in pipeline ordering, end-of-turn residual ordering audit, generalized action-queue access, per-source damage attribution, effective-ability accessor (for ability-swap moves), effective-stat accessor (for Unaware), crit-stage state, species weight in build dump, species evolution-stage flag, missing `MoveDef` fields (cannot_use_twice, max-HP recoil, flags.bite / .punch / .pulse / .bullet / .dance / .powder / .heal, full target enum), Oracle RNG damage-roll back-solver, EV-spread reconnaissance, singles-mode audit.

Approx 15 entries.

## Maintenance

When a PR lands:

1. Find the matching entry in the relevant `.md`.
2. Change `**Status**: not implemented.` to `**Status**: shipped — PR-N.`.
3. Move it from its current position into the corresponding "Shipped — for cross-reference" section if one exists; otherwise leave it in place.
4. Update the entry counts in this README if you have appetite (not load-bearing).

Partial implementations: keep the entry; update `Status` to `partial — <what's missing>`. The goal is that a partial entry tells you exactly which sub-bullet still needs work.

## Sort order

Within each file, entries are roughly sorted by VGC impact (highest-usage Pokémon's signature mechanic first, long tail toward the bottom). This is a soft sort — don't agonize over the order when adding new entries.

## Sources cited

- `data/smogon-stats/2026-05/gen9championsvgc2026regma-1760.txt` — Smogon Champions VGC 2026 Regulation A usage stats at the 1760 ladder cutoff. The current corpus.
- `/tmp/pokemon-showdown-research/data/moves.ts` etc. — PS source clone for file:slug citations.
- `/tmp/pokemon-showdown-research/data/abilities.ts`
- `/tmp/pokemon-showdown-research/data/items.ts`
- `/tmp/pokemon-showdown-research/data/conditions.ts`
- `docs/interactions.md` — gen-9 edge-case reference.
