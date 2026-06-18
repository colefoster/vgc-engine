//! Pokémon state.
//!
//! Stats follow the gen 3+ formula (Bulbapedia: "Stat — Determination of
//! stats", https://bulbapedia.bulbagarden.net/wiki/Stat). Phase 2 only
//! reads them; damage / boost application lands in later PRs.

use serde::{Deserialize, Serialize};

use vgc_engine_data as data;

/// Stable indexing of the six battle stats. Matches PS's order in
/// `sim/pokemon.ts` (StatsTable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stat {
    Hp = 0,
    Atk = 1,
    Def = 2,
    Spa = 3,
    Spd = 4,
    Spe = 5,
}

/// Persistent status condition. Volatile statuses (confusion, taunt, ...)
/// will live in a separate bitset on `Pokemon` once mechanics need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    None,
    Sleep,
    Freeze,
    Paralysis,
    Burn,
    Poison,
    Toxic,
}

/// Nature multiplier table. Lowercase slugs, matching PS.
///
/// Each entry: (plus_stat, minus_stat). Both `None` for neutral natures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nature {
    pub slug: &'static str,
    pub plus: Option<Stat>,
    pub minus: Option<Stat>,
}

const NATURES: &[Nature] = &[
    Nature { slug: "hardy",   plus: None,             minus: None },
    Nature { slug: "lonely",  plus: Some(Stat::Atk),  minus: Some(Stat::Def) },
    Nature { slug: "brave",   plus: Some(Stat::Atk),  minus: Some(Stat::Spe) },
    Nature { slug: "adamant", plus: Some(Stat::Atk),  minus: Some(Stat::Spa) },
    Nature { slug: "naughty", plus: Some(Stat::Atk),  minus: Some(Stat::Spd) },
    Nature { slug: "bold",    plus: Some(Stat::Def),  minus: Some(Stat::Atk) },
    Nature { slug: "docile",  plus: None,             minus: None },
    Nature { slug: "relaxed", plus: Some(Stat::Def),  minus: Some(Stat::Spe) },
    Nature { slug: "impish",  plus: Some(Stat::Def),  minus: Some(Stat::Spa) },
    Nature { slug: "lax",     plus: Some(Stat::Def),  minus: Some(Stat::Spd) },
    Nature { slug: "timid",   plus: Some(Stat::Spe),  minus: Some(Stat::Atk) },
    Nature { slug: "hasty",   plus: Some(Stat::Spe),  minus: Some(Stat::Def) },
    Nature { slug: "serious", plus: None,             minus: None },
    Nature { slug: "jolly",   plus: Some(Stat::Spe),  minus: Some(Stat::Spa) },
    Nature { slug: "naive",   plus: Some(Stat::Spe),  minus: Some(Stat::Spd) },
    Nature { slug: "modest",  plus: Some(Stat::Spa),  minus: Some(Stat::Atk) },
    Nature { slug: "mild",    plus: Some(Stat::Spa),  minus: Some(Stat::Def) },
    Nature { slug: "quiet",   plus: Some(Stat::Spa),  minus: Some(Stat::Spe) },
    Nature { slug: "bashful", plus: None,             minus: None },
    Nature { slug: "rash",    plus: Some(Stat::Spa),  minus: Some(Stat::Spd) },
    Nature { slug: "calm",    plus: Some(Stat::Spd),  minus: Some(Stat::Atk) },
    Nature { slug: "gentle",  plus: Some(Stat::Spd),  minus: Some(Stat::Def) },
    Nature { slug: "sassy",   plus: Some(Stat::Spd),  minus: Some(Stat::Spe) },
    Nature { slug: "careful", plus: Some(Stat::Spd),  minus: Some(Stat::Spa) },
    Nature { slug: "quirky",  plus: None,             minus: None },
];

pub fn nature_by_slug(slug: &str) -> Option<&'static Nature> {
    NATURES.iter().find(|n| n.slug == slug)
}

/// EV/IV spread. Defaults: 0 EVs / 31 IVs are exposed as named constants
/// for explicit construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatSpread {
    #[serde(default)] pub hp: u8,
    #[serde(default)] pub atk: u8,
    #[serde(default)] pub def: u8,
    #[serde(default)] pub spa: u8,
    #[serde(default)] pub spd: u8,
    #[serde(default)] pub spe: u8,
}

impl StatSpread {
    pub const ZERO: Self = Self { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 };
    pub const MAX_IV: Self = Self { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 };

    pub fn get(&self, s: Stat) -> u8 {
        match s {
            Stat::Hp => self.hp,
            Stat::Atk => self.atk,
            Stat::Def => self.def,
            Stat::Spa => self.spa,
            Stat::Spd => self.spd,
            Stat::Spe => self.spe,
        }
    }
}

/// Final, post-calculation stats. HP is current max; the 5 others are the
/// "level-50, EV/IV/nature applied" values used by the damage formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FinalStats {
    pub hp: u16,
    pub atk: u16,
    pub def: u16,
    pub spa: u16,
    pub spd: u16,
    pub spe: u16,
}

impl FinalStats {
    pub fn get(&self, s: Stat) -> u16 {
        match s {
            Stat::Hp => self.hp,
            Stat::Atk => self.atk,
            Stat::Def => self.def,
            Stat::Spa => self.spa,
            Stat::Spd => self.spd,
            Stat::Spe => self.spe,
        }
    }
}

/// PS-named identity for the per-Pokemon volatile registry. Each
/// kind maps 1:1 to a PS `data/conditions.ts` entry (the `id` of the
/// volatile, e.g. `'taunt'`, `'disable'`, ...). Pokemon carries a
/// fixed-capacity `volatiles: [Volatile; 8]` array (PS limits in
/// practice — corpus mons rarely carry more than 3-4 at a time);
/// callers look up by kind in O(1). The migration of the ad-hoc
/// boolean / counter fields (`is_protected_this_turn`,
/// `flinched_this_turn`, `helping_handed_this_turn`,
/// `damaged_this_turn`,
/// `crit_stage_volatile`, `semi_invuln`,
/// `charging_turns`, `must_recharge`, `lockin_turns`) into this
/// registry is staged per-volatile in follow-up PRs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum VolatileKind {
    #[default]
    None = 0,
    Taunt,
    Disable,
    Confusion,
    Torment,
    Yawn,
    HealBlock,
    Embargo,
    Imprison,
    MagicCoat,
    Snatch,
    LeechSeed,
    Curse,
    Nightmare,
    PerishSong,
    Ingrain,
    AquaRing,
    MagnetRise,
    Telekinesis,
    GastroAcid,
    PowderShield,
    FocusEnergy,
    LaserFocus,
    Charge,
    Stockpile,
    Roost,
    Foresight,
    MiracleEye,
    Tarshot,
    SyrupBomb,
    GlaiveRush,
    SaltCure,
    Endure,
    DragonCheer,
    /// Single-turn 'helpinghand' volatile (PS data/moves.ts:helpinghand,
    /// duration 1). Set by an adjacent ally's Helping Hand on this mon;
    /// read by `damage.rs` to multiply BP ×1.5 on the next damaging move
    /// this turn. Cleared at the per-turn volatile reset / on switch-out.
    /// Payload unused.
    HelpingHand,
    /// Single-turn 'flinch' volatile (PS data/conditions.ts:flinch).
    /// Set when struck by a flinching move; checked at the start of
    /// resolve_move to skip the action. Cleared at the per-turn
    /// volatile reset / on switch-in. Payload unused.
    Flinch,
    /// Single-turn 'protect' family volatile (PS data/conditions.ts:protect).
    /// Set when the mon successfully used Protect / Detect / Spiky Shield /
    /// Baneful Bunker / Burning Bulwark / Silk Trap. Causes targeting moves
    /// against this mon to fail; cleared at end of turn. Payload unused.
    Protect,
    /// Single-turn 'was damaged' marker. Set when an opposing damaging
    /// move actually lands HP damage on this mon. Read by Avalanche /
    /// Revenge for the ×2 BP bonus. Cleared at the per-turn volatile
    /// reset / on switch-in. Payload unused.
    DamagedThisTurn,
    /// Single-turn 'just switched in' marker. Set when this mon enters
    /// the field via a Switch action (NOT for battle-start sendouts).
    /// Read by ability residual hooks (Speed Boost et al.) to suppress
    /// the residual on the switch-in turn. PS analog: `activeTurns == 0`
    /// at end of turn. Cleared at the per-turn volatile reset.
    JustSwitchedIn,
    /// 'pending self-switch' marker (PS `pokemon.switchFlag`). Set by
    /// U-turn / Volt Switch / Flip Turn / Parting Shot / Teleport /
    /// Chilly Reception after a successful resolution; consumed by the
    /// engine's deferred-switch sweep. Cleared at end of step, on
    /// switch-out, or once the deferred switch is applied.
    PendingSelfSwitch,
    /// Choice-item move lock (PS `data/items.ts:choiceband` / scarf /
    /// specs). Indefinite duration; `payload` carries the locked move
    /// slot (0..=3). Set when the holder uses a move; cleared on
    /// switch-out or item swap.
    Locked,
    /// Stall counter for the Protect family (PS `data/conditions.ts:stall`).
    /// Indefinite duration; `payload` packs `(used_this_turn << 8) | counter`:
    /// the low byte is the current streak count (0..=6; success
    /// probability `1 / 3^counter`), bit 8 is `used_stall_this_turn`
    /// (set when the mon issues a stall move on the current turn, read
    /// at end of turn to reset the counter if the streak broke).
    Stall,
    /// Encore lock (PS `data/conditions.ts:encore`, duration 3). Payload
    /// packs `(turns << 8) | slot` — high 8 bits are remaining turns,
    /// low 8 bits are the locked move slot (0..=3). Decremented at end
    /// of step; cleared on switch-out or when the locked move's PP
    /// runs out.
    Encore,
    /// Sleep counter (PS `data/conditions.ts:slp`). Indefinite duration;
    /// `payload` carries the remaining sleep turns (1..=3 on gen 5+ apply,
    /// decremented at the start of each move attempt; the mon wakes when
    /// the decrement hits 0). Persists across switches (PS analog:
    /// `pokemon.statusState.time`). Cleared when Status leaves Sleep.
    Sleep,
    /// Substitute (PS `data/conditions.ts:substitute`). Indefinite
    /// duration (`turns_remaining == 0`); `payload` carries the current
    /// HP of the sub doll. Cleared on switch-out.
    Substitute,
    /// Toxic counter (PS `data/conditions.ts:tox`). Indefinite duration
    /// (`turns_remaining == 0`); `payload` carries the 1-based counter
    /// (1 on the turn Toxic is applied; +1 each end of turn, capped at
    /// 15). Damage per turn = max_hp * counter / 16. Cleared when the
    /// Toxic status clears.
    ToxicCounter,
    /// Single-turn redirection volatile (PS data/moves.ts:ragepowder /
    /// :followme conditions, duration 1). Set when this mon successfully
    /// uses Rage Powder or Follow Me. Read at single-target resolution
    /// to re-aim opposing moves at this mon. Payload bit 0 = `is_powder`
    /// (Rage Powder sets 1, Follow Me sets 0); used at the redirect
    /// site for the powder-gate check. Cleared at the per-turn volatile
    /// reset / on switch-out.
    Redirect,
    /// Partial-trap volatile (PS `data/conditions.ts:partiallytrapped`).
    /// Set by Whirlpool / Wrap / Bind / Fire Spin / Sand Tomb /
    /// Magma Storm / Infestation / Clamp / Snap Trap / Thunder Cage.
    /// Indefinite duration semantics (we use `payload` to encode both
    /// remaining turns and the source slot):
    ///   bits 0..7 → remaining turns (1..=6, decremented each end of
    ///               turn; volatile drops at 0)
    ///   bits 8..15 → source side (0 = P1, 1 = P2)
    ///   bits 16..23 → source slot (0 or 1)
    /// Each end of turn (PS onResidualOrder 13) the holder takes
    /// 1/8 max HP damage; Magic Guard blocks. Binding Band held by
    /// the source bumps the chip to 1/6 — deferred (no consumer in
    /// items.rs yet).
    PartialTrap,
    /// Flash Fire activation marker (PS `data/abilities.ts:flashfire`
    /// `onTryHit` adds the volatile when the holder absorbs a Fire move).
    /// Indefinite duration; cleared on switch-out. While set, the holder's
    /// outgoing Fire-type damaging moves get x1.5 BP (damage.rs reads
    /// this flag). Payload unused.
    FlashFire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Volatile {
    pub kind: VolatileKind,
    /// Turns until the volatile is cleared at end of turn. Set on
    /// add (PS `duration`); decremented at end of `step`; the
    /// volatile is removed when it reaches 0. `0` here means
    /// "indefinite" — only ticks via an explicit clear (Leech Seed
    /// drains until switch-out, Ingrain until switch-out, ...).
    pub turns_remaining: u8,
    /// Free-form 32-bit payload — Disable holds the disabled move
    /// slot (0..=3, 4..=7 reserved); Encore holds the locked slot;
    /// LeechSeed holds the source (side|slot) tuple; etc. Each
    /// kind's payload encoding is documented at its consumer site.
    pub payload: u32,
}

/// Fixed-cap volatile registry. 8 slots is comfortably more than the
/// in-corpus max (≈4). Lookup is a linear scan — at this size, that's
/// faster than the branchier alternatives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VolatileSet {
    pub items: [Volatile; 8],
    pub len: u8,
}

impl VolatileSet {
    /// Find the slot index of the given kind, or `None` if absent.
    #[inline]
    pub fn position(&self, k: VolatileKind) -> Option<usize> {
        (0..self.len as usize).find(|&i| self.items[i].kind == k)
    }

    pub fn has(&self, k: VolatileKind) -> bool {
        self.position(k).is_some()
    }

    pub fn get(&self, k: VolatileKind) -> Option<&Volatile> {
        self.position(k).map(|i| &self.items[i])
    }

    /// Add a fresh volatile or refresh the duration on an existing one
    /// (PS adds replace silently — Taunt re-application resets the
    /// turn counter). Returns `false` and silently drops the add if
    /// the registry is full (8 slots — never observed full in corpus,
    /// but the bound is defensive). Caller is responsible for
    /// per-volatile gating (e.g. Substitute fails if a sub already
    /// exists).
    pub fn add(&mut self, v: Volatile) -> bool {
        if let Some(i) = self.position(v.kind) {
            self.items[i] = v;
            return true;
        }
        if (self.len as usize) >= self.items.len() {
            return false;
        }
        self.items[self.len as usize] = v;
        self.len += 1;
        true
    }

    /// Remove a volatile by kind. No-op if absent.
    pub fn remove(&mut self, k: VolatileKind) {
        if let Some(i) = self.position(k) {
            let last = self.len as usize - 1;
            self.items[i] = self.items[last];
            self.items[last] = Volatile::default();
            self.len -= 1;
        }
    }

    /// Reset to empty. Used on switch-out (PS drops every volatile
    /// except `mustrecharge` / `partiallytrapped` per move; we
    /// blanket-clear since each migrated mechanic re-implements its
    /// own switch-out rule).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Decrement every volatile's `turns_remaining` (if non-zero); drop
    /// those that reach 0. Called once at end of step from
    /// `resolve_end_of_turn` after the per-volatile residual phases.
    /// Indefinite volatiles (`turns_remaining == 0`) are untouched.
    pub fn tick(&mut self) {
        let mut i = 0;
        while (i as u8) < self.len {
            if self.items[i].turns_remaining > 0 {
                self.items[i].turns_remaining -= 1;
                if self.items[i].turns_remaining == 0 {
                    let last = self.len as usize - 1;
                    self.items[i] = self.items[last];
                    self.items[last] = Volatile::default();
                    self.len -= 1;
                    continue;
                }
            }
            i += 1;
        }
    }
}

/// One Pokémon. Phase 2 carries the minimum a damage calc needs.
#[derive(Debug, Clone)]
pub struct Pokemon {
    pub species_id: u16,
    pub level: u8,
    pub moves: [u16; 4],
    pub pp: [u8; 4],
    pub ability_id: u16,
    pub item_id: u16,
    pub stats: FinalStats,
    pub current_hp: u16,
    pub status: Status,
    /// Stat boost stages in -6..=6 for [atk, def, spa, spd, spe, acc, eva].
    pub boosts: [i8; 7],
    pub fainted: bool,
    /// Number of turns this Pokémon has been on the field (0 on the turn
    /// it switched in / was sent out at battle start). Used by Fake Out,
    /// First Impression, Mat Block, etc. Incremented at end of step.
    pub turns_active: u8,
    /// Encoded `(side_byte, slot_byte)` of the most recent attacker
    /// that landed damaging-move HP damage on this mon this turn.
    /// `(255, 255)` = no attacker recorded. `side_byte`: 0 = P1,
    /// 1 = P2. Read by Stamina / Anger Point / Cotton Down to direct
    /// counter-effects at the actual attacker; Counter / Mirror Coat
    /// / Metal Burst will also consume this. Cleared at end of step
    /// alongside `damaged_this_turn`. PS analog: the last entry of
    /// `pokemon.attackedBy` filtered to the current turn.
    pub last_attacker: (u8, u8),
    /// Move category of the last damaging hit (0 = physical, 1 = special,
    /// 255 = none). Used by Counter (physical-only) and Mirror Coat
    /// (special-only) when those moves land. Cleared at end of step.
    pub last_attacker_category: u8,
    /// HP damage dealt by the last attacker this turn (0 = none).
    /// Used by Counter / Mirror Coat / Metal Burst / Bide payout
    /// calculation. Cleared at end of step.
    pub last_damage_taken: u16,
    /// Slot index of the most recent move this mon used (PP-consumed),
    /// or 255 if it hasn't moved yet on the field. Cleared on switch-
    /// out. Used by Encore to determine the lock target.
    pub last_used_move_slot: u8,
    /// Paradox booster stat index (0=atk, 1=def, 2=spa, 3=spd, 4=spe;
    /// 255 = no boost active). Set when Protosynthesis or Quark Drive
    /// activates via its trigger (Sun / Electric Terrain / Booster
    /// Energy); identifies which stat receives the ×1.3 (×1.5 for spe)
    /// multiplier. Cleared on switch-out or when the trigger expires
    /// (unless `booster_locked` is set — Booster-Energy-activated
    /// volatiles persist for as long as the mon stays in).
    pub boosted_stat: u8,
    /// `true` if the paradox booster volatile was activated via Booster
    /// Energy rather than weather/terrain. While set, `refresh_paradox_booster`
    /// will NOT deactivate the volatile when the natural trigger leaves —
    /// PS's `protosynthesis` / `quarkdrive` volatile, once added by the
    /// item path (`data/items.ts:boosterenergy onUpdate`), stays active
    /// until switch-out. Reset to false on switch-out.
    pub booster_locked: bool,
    /// PS-named volatile registry. Empty at start of battle. New
    /// per-Pokemon volatiles (Taunt, Disable, Confusion, ...) land
    /// here instead of growing more ad-hoc fields on `Pokemon`.
    /// Cleared blanket-style on switch-out. See `VolatileSet`.
    pub volatiles: VolatileSet,
    /// Two-turn-move semi-invulnerable state. Distinct positions hit
    /// through by distinct moves (PS gates each in the move's
    /// `onTryHit`): Dig is hit by Earthquake / Magnitude / Fissure;
    /// Dive by Surf / Whirlpool; Fly / Bounce / Sky Drop by Gust /
    /// Thunder / Twister / Sky Uppercut / Smack Down / Hurricane;
    /// Phantom Force / Shadow Force are hit by nothing.
    /// 0 = None, 1 = Dig, 2 = Dive, 3 = Fly, 4 = Bounce,
    /// 5 = PhantomForce, 6 = ShadowForce, 7 = SkyDrop.
    /// Cleared at the end of the second (attack) turn.
    pub semi_invuln: u8,
    /// Number of turns the mon has spent charging a multi-turn move
    /// (Solar Beam, Sky Attack, Electro Shot, Meteor Beam, Geomancy,
    /// Dig / Dive / Fly / Bounce / Phantom Force / Shadow Force). 0
    /// when not charging. Set to 1 on the first (charge) turn, the
    /// move resolves on the second turn and this resets to 0. PS
    /// `move.flags.charge` + a per-move `onTryMove` hook.
    pub charging_turns: u8,
    /// Slot index 0..=3 of the move currently being charged
    /// (`charging_turns > 0`). 255 = not charging. Used by the runner
    /// to dispatch the same move on turn 2 without re-reading
    /// `Choice`. PS `pokemon.moveThisTurn`.
    pub charging_move_slot: u8,
    /// `true` if the mon must spend its turn recharging after a
    /// recharge-flag move (Hyper Beam / Giga Impact / Blast Burn /
    /// Hydro Cannon / Frenzy Plant / Rock Wrecker / Roar of Time /
    /// Prismatic Laser / Eternabeam / Meteor Assault). Cleared at
    /// the end of the recharge turn. PS `flags.recharge`.
    pub must_recharge: bool,
    /// Lock-in counter for Outrage / Petal Dance / Thrash (separate
    /// from `locked_move_slot` which is reserved for Choice items).
    /// 0 = not locked, 1 = used the 1st turn of the lock, 2 = used
    /// the 2nd turn of the lock; at the end of turn 2 (or 3, PS rolls
    /// 2..=3), the mon becomes confused and the counter clears. PS
    /// `lockedmove` volatile.
    pub lockin_turns: u8,
    /// Move slot 0..=3 that the lock-in volatile is keying on
    /// (255 = none). The runner must dispatch this slot regardless
    /// of the player's Choice while the lock is active.
    pub lockin_move_slot: u8,
    /// Tera type. PS `pokemon.teraType` — encoded as a type code
    /// 0..=17 (same indexing as `species().types`). 255 = none assigned
    /// (legacy / set without teratype). Set at team load from the JSON
    /// `teratype` field; immutable thereafter (Tera Shell / Stellar
    /// re-typing is handled via `terastallized`).
    pub tera_type: u8,
    /// `true` if the mon has currently Terastallized. Set by the
    /// Terastallize action (separate `Choice::Terastallize` arm — TBD)
    /// or by the `tera: true` flag on a Move choice. Drives type
    /// override (offensive STAB read), Tera Blast BP gating, Stellar
    /// once-per-type bookkeeping. Persists across switch-out (Tera
    /// state survives switching in PS gen-9 doubles).
    pub terastallized: bool,
    /// Stellar once-per-type bookkeeping. Bit `i` is set the first time
    /// this mon lands a Stellar-bonus hit of move-type `i` (`i` in
    /// `0..18`, matching the data crate's type indexing). Subsequent
    /// Stellar attacks of the same type get no Stellar bonus per PS
    /// `sim/pokemon.ts` `runEffectiveness` Stellar branch. Persists
    /// across switch-out (Tera state — and the consumed-type list — does
    /// not reset on switching out in gen 9 doubles). Reset only at
    /// battle start.
    pub stellar_boosted_types: u32,
    /// Volatile crit-stage contributors from on-mon sources (Focus
    /// Energy / Laser Focus / Dire Hit). Held item, ability, and the
    /// move's high-crit-ratio flag are summed at damage time. Cleared
    /// on switch-out.
    pub crit_stage_volatile: u8,
    /// Ability suppression flag — when `true`, `effective_ability_slug()`
    /// returns `""` regardless of `ability_id`. Set by Gastro Acid (and
    /// Neutralizing Gas — TBD). PS models this as the `gastroacid`
    /// volatile + `Ability.isPermanent` gate; we collapse to a single
    /// bool for now. Cleared on switch-out (PS `onSwitchOut` /
    /// `onSwitchIn` reset paths). Abilities flagged "permanent" in PS
    /// (Multitype, RKS System, etc.) ignore the suppression — we keep
    /// the bool tight for the moment and gate at the consumer when
    /// those abilities are added.
    pub ability_suppressed: bool,
}

impl Pokemon {
    pub fn species(&self) -> &'static data::SpeciesDef {
        &data::SPECIES[self.species_id as usize]
    }

    pub fn is_alive(&self) -> bool {
        !self.fainted && self.current_hp > 0
    }

    /// True if the mon is grounded — i.e. terrain effects, Earthquake,
    /// Spikes etc. apply. False for Flying-type (type code 9),
    /// Levitate ability, or Air Balloon holder. Magnet Rise /
    /// Telekinesis / Roost / Gravity edge cases deferred.
    pub fn is_grounded(&self) -> bool {
        self.is_grounded_internal(false)
    }

    /// Variant used by Mold Breaker / Teravolt / Turboblaze: the
    /// breakable Levitate ability is treated as absent. Flying type and
    /// Air Balloon are NOT breakable, so they still ground-out the mon
    /// when set. Caller asserts that the attacker is currently breaking
    /// abilities on damaging moves.
    pub fn is_grounded_for_mold_breaker(&self) -> bool {
        self.is_grounded_internal(true)
    }

    /// Effective types after Tera. Returns the species' types unless
    /// `terastallized` is true, in which case the mon has a single
    /// type equal to `tera_type`. Stellar (`tera_type == 255`) returns
    /// the species' types — Stellar doesn't re-type the mon, it only
    /// boosts STAB and adds one-shot type matchups. Callers that need
    /// to know "is this mon currently Tera-active" read `terastallized`
    /// directly.
    pub fn effective_types(&self) -> ([u8; 2], u8) {
        let s = self.species();
        if self.terastallized && self.tera_type != 255 {
            ([self.tera_type, 0], 1)
        } else {
            (s.types, s.num_types)
        }
    }

    /// Effective crit stage from on-mon contributors (held item,
    /// ability, volatile). Caller adds the move's high-crit-ratio
    /// contribution (`+1`) before passing to `Rng::crit_with_stage`.
    /// PS gen-9:
    ///   Scope Lens / Razor Claw: +1 (item)
    ///   Lucky Punch on Chansey / Stick on Farfetch'd: +2 (item, species-gated)
    ///   Super Luck ability: +1
    ///   Focus Energy / Laser Focus / Dire Hit volatile: +2
    /// PS: `data/items.ts`:scopelens / `data/abilities.ts`:superluck /
    /// `data/conditions.ts`:focusenergy.
    pub fn effective_crit_stage(&self) -> u8 {
        let mut s = self.crit_stage_volatile;
        let ability = self.effective_ability_slug();
        if ability == "superluck" {
            s = s.saturating_add(1);
        }
        let item = if self.item_id == u16::MAX {
            ""
        } else {
            data::ITEMS[self.item_id as usize].slug
        };
        match item {
            "scopelens" | "razorclaw" => s = s.saturating_add(1),
            "luckypunch" => {
                // Chansey-only (PS species gate). dex num 113.
                if self.species().num == 113 {
                    s = s.saturating_add(2);
                }
            }
            "stick" | "leek" => {
                // Farfetch'd 83 / Sirfetch'd 865.
                let n = self.species().num;
                if n == 83 || n == 865 {
                    s = s.saturating_add(2);
                }
            }
            _ => {}
        }
        s
    }

    /// Effective ability slug. Returns `""` when the ability is
    /// suppressed (Gastro Acid) or the slot is the sentinel. All new
    /// code that branches on ability should call this instead of
    /// reading `ability_id` directly. PS analog: `Pokemon.getAbility()`
    /// in `sim/pokemon.ts`, which returns the suppressed ability when
    /// Gastro Acid'd / Neutralizing Gas'd unless the ability is
    /// `isPermanent`. We collapse to a single `ability_suppressed`
    /// bool; consumers needing the "permanent" exception read
    /// `ability_id` directly.
    pub fn effective_ability_slug(&self) -> &'static str {
        if self.ability_suppressed {
            return "";
        }
        if self.ability_id == u16::MAX {
            return "";
        }
        data::ABILITIES
            .get(self.ability_id as usize)
            .map(|a| a.slug)
            .unwrap_or("")
    }

    /// `true` while `VolatileKind::PendingSelfSwitch` is on this mon.
    /// Set by self-switch moves (U-turn etc.); consumed by the engine's
    /// deferred-switch sweep.
    #[inline]
    pub fn pending_self_switch(&self) -> bool {
        self.volatiles.has(VolatileKind::PendingSelfSwitch)
    }

    /// Set or clear the PendingSelfSwitch marker.
    #[inline]
    pub fn set_pending_self_switch(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::PendingSelfSwitch,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::PendingSelfSwitch);
        }
    }

    /// `true` while `VolatileKind::JustSwitchedIn` is on this mon. Set
    /// when entering via a mid-battle Switch action; read by ability
    /// residuals (Speed Boost) to skip the residual on the switch-in
    /// turn. PS analog: `pokemon.activeTurns == 0`.
    #[inline]
    pub fn switched_in_this_turn(&self) -> bool {
        self.volatiles.has(VolatileKind::JustSwitchedIn)
    }

    /// Set or clear the JustSwitchedIn marker.
    #[inline]
    pub fn set_switched_in_this_turn(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::JustSwitchedIn,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::JustSwitchedIn);
        }
    }

    /// `true` while `VolatileKind::DamagedThisTurn` is on this mon.
    /// Set by `battle.rs` when an opposing damaging move lands HP
    /// damage. Read by Avalanche / Revenge for ×2 BP.
    #[inline]
    pub fn damaged_this_turn(&self) -> bool {
        self.volatiles.has(VolatileKind::DamagedThisTurn)
    }

    /// Set or clear the DamagedThisTurn marker.
    #[inline]
    pub fn set_damaged_this_turn(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::DamagedThisTurn,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::DamagedThisTurn);
        }
    }

    /// `true` while `VolatileKind::Protect` is on this mon. Read at
    /// move-resolution time to fail targeting moves.
    #[inline]
    pub fn is_protected_this_turn(&self) -> bool {
        self.volatiles.has(VolatileKind::Protect)
    }

    /// Set or clear the Protect volatile (duration-1).
    #[inline]
    pub fn set_protected(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Protect,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::Protect);
        }
    }

    /// `true` while a `VolatileKind::Flinch` volatile is on this mon.
    /// Read in `battle.rs::resolve_move` to skip the action.
    #[inline]
    pub fn flinched_this_turn(&self) -> bool {
        self.volatiles.has(VolatileKind::Flinch)
    }

    /// Set or clear the Flinch volatile (duration-1).
    #[inline]
    pub fn set_flinched(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Flinch,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::Flinch);
        }
    }

    /// Choice-item locked move slot. 255 when not locked.
    #[inline]
    pub fn locked_move_slot(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Locked)
            .map(|v| v.payload as u8)
            .unwrap_or(255)
    }

    /// Set / clear the Choice-item lock. `slot == 255` removes it.
    #[inline]
    pub fn set_locked_move_slot(&mut self, slot: u8) {
        if slot == 255 {
            self.volatiles.remove(VolatileKind::Locked);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Locked,
                turns_remaining: 0,
                payload: slot as u32,
            });
        }
    }

    /// Consecutive-stall-success counter (Protect family). `0` if no
    /// streak in progress.
    #[inline]
    pub fn stall_counter(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Stall)
            .map(|v| (v.payload & 0xFF) as u8)
            .unwrap_or(0)
    }

    /// `true` if this mon has already issued a stall move this turn.
    #[inline]
    pub fn used_stall_this_turn(&self) -> bool {
        self.volatiles
            .get(VolatileKind::Stall)
            .map(|v| (v.payload >> 8) & 1 != 0)
            .unwrap_or(false)
    }

    /// Set the stall counter (0..=6). 0 removes the volatile unless
    /// `used_this_turn` is also set.
    #[inline]
    pub fn set_stall(&mut self, counter: u8, used_this_turn: bool) {
        if counter == 0 && !used_this_turn {
            self.volatiles.remove(VolatileKind::Stall);
        } else {
            let payload = ((used_this_turn as u32) << 8) | (counter as u32 & 0xFF);
            self.volatiles.add(Volatile {
                kind: VolatileKind::Stall,
                turns_remaining: 0,
                payload,
            });
        }
    }

    /// Mark that this mon issued a stall move on the current turn,
    /// preserving the streak counter.
    #[inline]
    pub fn mark_used_stall_this_turn(&mut self) {
        let c = self.stall_counter();
        self.set_stall(c, true);
    }

    /// Clear the "used stall this turn" bit, preserving the counter.
    #[inline]
    pub fn clear_used_stall_this_turn(&mut self) {
        let c = self.stall_counter();
        self.set_stall(c, false);
    }

    /// Remaining Encore turns; `0` when not encored.
    #[inline]
    pub fn encore_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Encore)
            .map(|v| (v.payload >> 8) as u8)
            .unwrap_or(0)
    }

    /// Encored move slot (0..=3). 255 if not encored.
    #[inline]
    pub fn encored_move_slot(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Encore)
            .map(|v| (v.payload & 0xFF) as u8)
            .unwrap_or(255)
    }

    /// Apply an Encore lock for `turns` (1..=3) on move slot `slot`. If
    /// `turns == 0`, clears the lock instead.
    #[inline]
    pub fn set_encore(&mut self, turns: u8, slot: u8) {
        if turns == 0 {
            self.volatiles.remove(VolatileKind::Encore);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Encore,
                turns_remaining: 0,
                payload: ((turns as u32) << 8) | (slot as u32 & 0xFF),
            });
        }
    }

    /// Clear the Encore volatile.
    #[inline]
    pub fn clear_encore(&mut self) {
        self.volatiles.remove(VolatileKind::Encore);
    }

    /// Remaining sleep turns. `0` if not asleep.
    #[inline]
    pub fn sleep_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Sleep)
            .map(|v| v.payload as u8)
            .unwrap_or(0)
    }

    /// Set / clear the Sleep counter volatile. `t == 0` removes it.
    #[inline]
    pub fn set_sleep_turns(&mut self, t: u8) {
        if t == 0 {
            self.volatiles.remove(VolatileKind::Sleep);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Sleep,
                turns_remaining: 0,
                payload: t as u32,
            });
        }
    }

    /// Substitute HP. `0` = no sub. When > 0, incoming damage is
    /// absorbed by the sub before reaching `current_hp` (sound moves
    /// bypass it — PR-51).
    #[inline]
    pub fn substitute_hp(&self) -> u16 {
        self.volatiles
            .get(VolatileKind::Substitute)
            .map(|v| v.payload as u16)
            .unwrap_or(0)
    }

    /// Set / clear the Substitute volatile. `hp == 0` removes it.
    #[inline]
    pub fn set_substitute_hp(&mut self, hp: u16) {
        if hp == 0 {
            self.volatiles.remove(VolatileKind::Substitute);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Substitute,
                turns_remaining: 0,
                payload: hp as u32,
            });
        }
    }

    /// Current Toxic counter (1-based). `0` if Toxic is not active.
    /// Read by the end-of-turn DOT phase and the Toxic-apply path.
    #[inline]
    pub fn toxic_counter(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::ToxicCounter)
            .map(|v| v.payload as u8)
            .unwrap_or(0)
    }

    /// Set the Toxic counter. `c == 0` removes the volatile.
    #[inline]
    pub fn set_toxic_counter(&mut self, c: u8) {
        if c == 0 {
            self.volatiles.remove(VolatileKind::ToxicCounter);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::ToxicCounter,
                turns_remaining: 0,
                payload: c as u32,
            });
        }
    }

    /// `true` while a `VolatileKind::Redirect` volatile is on this mon
    /// (this mon used Rage Powder / Follow Me this turn).
    #[inline]
    pub fn redirecting_this_turn(&self) -> bool {
        self.volatiles.has(VolatileKind::Redirect)
    }

    /// `true` if the active Redirect volatile was set by Rage Powder
    /// (powder-gated). Only meaningful when `redirecting_this_turn()`
    /// is true. Encoded in payload bit 0.
    #[inline]
    pub fn redirecting_is_powder(&self) -> bool {
        self.volatiles
            .get(VolatileKind::Redirect)
            .map(|v| (v.payload & 1) != 0)
            .unwrap_or(false)
    }

    /// Set the Redirect volatile (`is_powder=true` for Rage Powder,
    /// `false` for Follow Me) or clear it when `on == false`.
    #[inline]
    pub fn set_redirecting(&mut self, on: bool, is_powder: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Redirect,
                turns_remaining: 0,
                payload: if is_powder { 1 } else { 0 },
            });
        } else {
            self.volatiles.remove(VolatileKind::Redirect);
        }
    }

    /// `true` while a `VolatileKind::HelpingHand` volatile is on this mon
    /// (an ally Helping Hand'd this target on the current turn). Read by
    /// `damage.rs` for the ×1.5 BP multiplier. PS analog:
    /// `data/moves.ts:helpinghand` condition.
    #[inline]
    pub fn helping_handed_this_turn(&self) -> bool {
        self.volatiles.has(VolatileKind::HelpingHand)
    }

    /// Set or clear the Helping Hand volatile. Setting to `true` adds a
    /// duration-1 entry (PS clears it at end of turn via the per-turn
    /// volatile reset, which calls this with `false`).
    #[inline]
    pub fn set_helping_handed(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::HelpingHand,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::HelpingHand);
        }
    }

    fn is_grounded_internal(&self, ignore_levitate: bool) -> bool {
        let s = self.species();
        let flying = (0..s.num_types as usize).any(|i| s.types[i] == 9);
        if flying {
            return false;
        }
        let ability = self.effective_ability_slug();
        if ability == "levitate" && !ignore_levitate {
            return false;
        }
        let item = if self.item_id == u16::MAX {
            ""
        } else {
            data::ITEMS[self.item_id as usize].slug
        };
        if item == "airballoon" {
            return false;
        }
        true
    }
}

/// Gen 3+ stat formula. See Bulbapedia "Stat — Determination of stats".
pub fn compute_stats(
    species: &data::SpeciesDef,
    level: u8,
    ivs: &StatSpread,
    evs: &StatSpread,
    nature: &Nature,
) -> FinalStats {
    let level = level as u32;
    let bs = &species.base_stats;
    let calc = |base: u8, iv: u8, ev: u8| -> u32 {
        ((2 * base as u32 + iv as u32 + (ev as u32) / 4) * level) / 100
    };
    let hp = if bs[0] == 1 {
        // Shedinja special-case (PS sim/pokemon.ts: getStat).
        1
    } else {
        calc(bs[0], ivs.hp, evs.hp) + level + 10
    };
    let apply_nature = |base: u32, which: Stat| -> u32 {
        let mut x = base + 5;
        if nature.plus == Some(which) && nature.minus != Some(which) {
            x = (x * 11) / 10;
        } else if nature.minus == Some(which) && nature.plus != Some(which) {
            x = (x * 9) / 10;
        }
        x
    };
    FinalStats {
        hp: hp.min(u16::MAX as u32) as u16,
        atk: apply_nature(calc(bs[1], ivs.atk, evs.atk), Stat::Atk) as u16,
        def: apply_nature(calc(bs[2], ivs.def, evs.def), Stat::Def) as u16,
        spa: apply_nature(calc(bs[3], ivs.spa, evs.spa), Stat::Spa) as u16,
        spd: apply_nature(calc(bs[4], ivs.spd, evs.spd), Stat::Spd) as u16,
        spe: apply_nature(calc(bs[5], ivs.spe, evs.spe), Stat::Spe) as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_set_add_remove_tick_basic() {
        let mut v = VolatileSet::default();
        assert!(!v.has(VolatileKind::Taunt));
        // Add Taunt (duration 3).
        assert!(v.add(Volatile { kind: VolatileKind::Taunt, turns_remaining: 3, payload: 0 }));
        assert!(v.has(VolatileKind::Taunt));
        assert_eq!(v.get(VolatileKind::Taunt).unwrap().turns_remaining, 3);
        // Re-add refreshes duration.
        v.add(Volatile { kind: VolatileKind::Taunt, turns_remaining: 5, payload: 0 });
        assert_eq!(v.get(VolatileKind::Taunt).unwrap().turns_remaining, 5);
        // Add a second kind.
        v.add(Volatile { kind: VolatileKind::Disable, turns_remaining: 4, payload: 2 });
        assert_eq!(v.len, 2);
        // Tick — both decrement.
        v.tick();
        assert_eq!(v.get(VolatileKind::Taunt).unwrap().turns_remaining, 4);
        assert_eq!(v.get(VolatileKind::Disable).unwrap().turns_remaining, 3);
        // Remove by kind.
        v.remove(VolatileKind::Taunt);
        assert!(!v.has(VolatileKind::Taunt));
        assert!(v.has(VolatileKind::Disable));
        assert_eq!(v.len, 1);
    }

    #[test]
    fn volatile_set_tick_drops_at_zero() {
        let mut v = VolatileSet::default();
        v.add(Volatile { kind: VolatileKind::FocusEnergy, turns_remaining: 1, payload: 0 });
        v.tick();
        assert!(!v.has(VolatileKind::FocusEnergy));
        assert_eq!(v.len, 0);
    }

    #[test]
    fn volatile_set_indefinite_does_not_tick() {
        let mut v = VolatileSet::default();
        v.add(Volatile { kind: VolatileKind::LeechSeed, turns_remaining: 0, payload: 0 });
        v.tick();
        v.tick();
        v.tick();
        assert!(v.has(VolatileKind::LeechSeed));
    }

    #[test]
    fn effective_types_pre_tera_matches_species() {
        let species_idx = data::SPECIES.iter().position(|s| s.slug == "garchomp").unwrap() as u16;
        let species = &data::SPECIES[species_idx as usize];
        let mut mon = Pokemon {
            species_id: species_idx, level: 50, moves: [u16::MAX; 4], pp: [0; 4],
            ability_id: u16::MAX, item_id: u16::MAX, stats: FinalStats::default(),
            current_hp: 1, status: Status::None, boosts: [0; 7], fainted: false,
            turns_active: 0,
            last_used_move_slot: 255,
            boosted_stat: 255, booster_locked: false,
            ability_suppressed: false, crit_stage_volatile: 0,
            last_attacker: (255, 255), last_attacker_category: 255, last_damage_taken: 0,
            tera_type: 1 /* fire */, terastallized: false, stellar_boosted_types: 0,
            semi_invuln: 0, charging_turns: 0, charging_move_slot: 255,
            must_recharge: false, lockin_turns: 0, lockin_move_slot: 255,
            volatiles: VolatileSet::default(),
        };
        let (types, n) = mon.effective_types();
        assert_eq!(n, species.num_types);
        assert_eq!(types, species.types);
        // After Tera: single Fire type.
        mon.terastallized = true;
        let (types2, n2) = mon.effective_types();
        assert_eq!(n2, 1);
        assert_eq!(types2[0], 1);
    }

    #[test]
    fn effective_ability_slug_respects_suppression() {
        let species = data::species_by_slug("garchomp").expect("garchomp");
        let ab_id = data::ABILITIES.iter().position(|a| a.slug == "roughskin").unwrap() as u16;
        let mon = Pokemon {
            species_id: species.num,
            level: 50,
            moves: [u16::MAX; 4],
            pp: [0; 4],
            ability_id: ab_id,
            item_id: u16::MAX,
            stats: FinalStats::default(),
            current_hp: 100,
            status: Status::None,
            boosts: [0; 7],
            fainted: false,
            turns_active: 0,
            last_used_move_slot: 255,
            boosted_stat: 255,
            booster_locked: false,
            ability_suppressed: false,
            crit_stage_volatile: 0,
            last_attacker: (255, 255),
            last_attacker_category: 255,
            last_damage_taken: 0,
            tera_type: 0,
            terastallized: false, stellar_boosted_types: 0,
            semi_invuln: 0,
            charging_turns: 0,
            charging_move_slot: 255,
            must_recharge: false,
            lockin_turns: 0,
            lockin_move_slot: 255,
            volatiles: VolatileSet::default(),
        };
        assert_eq!(mon.effective_ability_slug(), "roughskin");
        let mut sup = mon.clone();
        sup.ability_suppressed = true;
        assert_eq!(sup.effective_ability_slug(), "");
    }

    #[test]
    fn adamant_garchomp_l50_31_252_atk() {
        // Garchomp base atk 130. Adamant + 31 IV + 252 EV at L50.
        //   inner = (2*130 + 31 + 63) * 50 / 100 = 177
        //   final = floor((177 + 5) * 1.1) = 200
        // Matches damage-calc.io.
        let species = data::species_by_slug("garchomp").expect("garchomp in dex");
        let ivs = StatSpread::MAX_IV;
        let evs = StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 };
        let stats = compute_stats(
            species,
            50,
            &ivs,
            &evs,
            nature_by_slug("adamant").unwrap(),
        );
        assert_eq!(stats.atk, 200, "Garchomp Adamant L50 31/252 atk");
        // HP: (2*108 + 31 + 0) * 50 / 100 + 50 + 10 = 183
        assert_eq!(stats.hp, 183, "Garchomp L50 31/0 hp");
    }
}
