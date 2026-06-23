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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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

impl Nature {
    /// Neutral nature (no stat multipliers). Used as the spread default for
    /// recon / test builders that construct `Pokemon` with precomputed stats.
    pub const NEUTRAL: Self = Self { slug: "hardy", plus: None, minus: None };
}

pub fn nature_by_slug(slug: &str) -> Option<&'static Nature> {
    NATURES.iter().find(|n| n.slug == slug)
}

/// `Nature` for a stable table id (index into `NATURES`). Companion to the
/// `nature_id::*` constants; lets `Pokemon::nature_id` (a `u8`) be resolved
/// back to its multiplier table without storing an embedded `&'static str`.
#[inline]
pub fn nature_by_id(id: u8) -> &'static Nature {
    &NATURES[id as usize]
}

/// Stable id (index into `NATURES`) for a nature slug, mirroring
/// `nature_by_slug`. Used at team-build time to store the compact `u8` id
/// on `Pokemon`.
#[inline]
pub fn nature_id_by_slug(slug: &str) -> Option<u8> {
    NATURES.iter().position(|n| n.slug == slug).map(|i| i as u8)
}

/// Stable `NATURES` table indices, the nature analog of
/// `data::ability_id::*`. Hand-written (the nature table lives in this
/// crate, not the build.rs codegen) but kept in lockstep with `NATURES`
/// by `nature_ids_match_table` in tests.
pub mod nature_id {
    pub const HARDY: u8 = 0;
    pub const LONELY: u8 = 1;
    pub const BRAVE: u8 = 2;
    pub const ADAMANT: u8 = 3;
    pub const NAUGHTY: u8 = 4;
    pub const BOLD: u8 = 5;
    pub const DOCILE: u8 = 6;
    pub const RELAXED: u8 = 7;
    pub const IMPISH: u8 = 8;
    pub const LAX: u8 = 9;
    pub const TIMID: u8 = 10;
    pub const HASTY: u8 = 11;
    pub const SERIOUS: u8 = 12;
    pub const JOLLY: u8 = 13;
    pub const NAIVE: u8 = 14;
    pub const MODEST: u8 = 15;
    pub const MILD: u8 = 16;
    pub const QUIET: u8 = 17;
    pub const BASHFUL: u8 = 18;
    pub const RASH: u8 = 19;
    pub const CALM: u8 = 20;
    pub const GENTLE: u8 = 21;
    pub const SASSY: u8 = 22;
    pub const CAREFUL: u8 = 23;
    pub const QUIRKY: u8 = 24;
    /// Neutral default (Hardy) — the spread default for recon / test
    /// builders that synthesize stats directly.
    pub const NEUTRAL: u8 = HARDY;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum VolatileKind {
    #[default]
    None = 0,
    /// Taunt lockout (PS `data/moves.ts:taunt` `condition`, `duration: 3`).
    /// Applied to the target by the Taunt move. While present, every
    /// Status-category move (`MoveDef::category == 2`, except Me First) is
    /// undisplayable — filtered out of `legal_choices` (PS `onDisableMove`)
    /// and made to fail if somehow dispatched (PS `onBeforeMove`). The
    /// duration is bumped to 4 on apply when the target has already acted
    /// this turn (PS `onStart`: `activeTurns && !willMove`). Decremented each
    /// end of turn alongside Disable / Throat Chop (PS `onResidualOrder 15`);
    /// drops at 0. Payload unused.
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
    /// Charge (PS `data/conditions.ts:charge`). Set by the Charge move
    /// or Wind Power; while present, the holder's next Electric move
    /// gets ×2 BP (`damage.rs` reads it) and the volatile is removed
    /// once that Electric move resolves (`battle.rs`). Indefinite here
    /// (PS's native `duration: 2` self-expiry is deferred — the primary
    /// clear is the post-Electric-move removal). Cleared on switch-out.
    /// Payload unused.
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
    /// Smack Down / Thousand Arrows grounding (PS
    /// `data/conditions.ts:smackdown` / `data/moves.ts:thousandarrows`
    /// `volatileStatus: 'smackdown'`). Indefinite duration; cleared on
    /// switch-out. While set, the holder counts as grounded — Flying
    /// type, Levitate, Air Balloon, and Magnet Rise are all overridden.
    /// Payload unused.
    SmackdownGrounded,
    /// Gravity grounding (PS `data/moves.ts:gravity` pseudo-weather, read
    /// live by `Pokemon.isGrounded()` via `field.getPseudoWeather('gravity')`).
    /// We mirror PS's "grounded while the field condition is up" semantics
    /// with a per-mon marker that the battle keeps in sync with
    /// `gravity_turns`: added to every active mon when Gravity is set and
    /// re-added on switch-in while it's active; removed from all active
    /// mons when Gravity expires. While set, the holder counts as grounded
    /// — Flying type, Levitate, Air Balloon, Magnet Rise, and Telekinesis
    /// are all overridden. Payload unused.
    GravityGrounded,
    /// Infatuation (PS `data/moves.ts:706` attract `condition`). Set by the
    /// move Attract (and later Cute Charm / Destiny Knot) on a target whose
    /// gender is opposite and non-genderless relative to the source (M↔F).
    /// Indefinite duration. Cleared on switch-out (blanket reset), when the
    /// source leaves the field (PS `onUpdate`), by Oblivious (`onUpdate` /
    /// `onTryHit`), or by Mental Herb. `payload` records the source mon so
    /// the clear-on-source-leave check can find it after switches:
    ///   bits 0..7 → source team roster index (0..=5)
    ///   bit 8     → source side (0 = P1, 1 = P2)
    /// Each turn the infatuated mon acts there is a 50% chance it is
    /// "immobilized by love" and skips the move (PS `onBeforeMovePriority 2`,
    /// `randomChance(1, 2)`; no PP consumed).
    Attract,
    /// Throat Chop lockout (PS `data/moves.ts:throatchop` `condition`,
    /// `duration: 2`). Applied to the target after Throat Chop hits (a
    /// 100% secondary). While present, every sound-flagged move
    /// (`MoveDef::is_sound`) is undisplayable — filtered out of
    /// `legal_choices` (PS `onDisableMove`) and made to fail if somehow
    /// dispatched (PS `onBeforeMove` / `onModifyMove`). Decremented each
    /// end of turn alongside Disable / Encore (PS `onResidualOrder 22`);
    /// drops at 0. Payload unused.
    ThroatChop,
    /// Ally Switch consecutive-use tracker (PS `data/moves.ts:allyswitch`
    /// `condition`, `duration: 2`, `counterMax: 729`). Added/refreshed on
    /// every successful Ally Switch use. `payload` holds the failure-roll
    /// denominator for the NEXT consecutive use (PS `effectState.counter`):
    /// `3` after the first success, then ×3 per consecutive success, capped
    /// at `729` (= 3^6). While present, a fresh Ally Switch use is a
    /// `randomChance(1, payload)` success roll (PS `onRestart`); on a failed
    /// roll the volatile is deleted and the move fails. `turns_remaining`
    /// carries the duration so the chain breaks the moment a turn passes
    /// without re-using the move (ticked end-of-turn alongside Throat Chop).
    /// Cleared on switch-out (`volatiles.clear()`).
    AllySwitch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
/// in-corpus max (≈4). `items[..len]` is the data store (linear scan for
/// `get`/`position`); `present` is a presence bitmask kept in sync on every
/// insert/remove so `has()` — called frequently in `step()` — is O(1).
///
/// `present` bit `i` is set iff a volatile with discriminant `i` is in the
/// store. `VolatileKind` is `#[repr(u8)]` with 52 sequential variants
/// (0..=51), so a `u64` holds one bit per kind with room to spare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolatileSet {
    pub items: [Volatile; 8],
    pub len: u8,
    /// Presence bitmask: `1 << (kind as u8)` per stored kind. Derived
    /// purely from `items[..len]`; kept consistent by the mutators below.
    pub present: u64,
}

impl VolatileSet {
    /// Find the slot index of the given kind, or `None` if absent.
    #[inline]
    pub fn position(&self, k: VolatileKind) -> Option<usize> {
        // Fast reject via the presence bitmask before the linear scan.
        if !self.has(k) {
            return None;
        }
        (0..self.len as usize).find(|&i| self.items[i].kind == k)
    }

    /// O(1) presence test via the bitmask.
    #[inline]
    pub fn has(&self, k: VolatileKind) -> bool {
        self.present & (1u64 << (k as u8)) != 0
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
            // Bit already set (refresh); keep it set defensively.
            self.present |= 1u64 << (v.kind as u8);
            return true;
        }
        if (self.len as usize) >= self.items.len() {
            return false;
        }
        self.items[self.len as usize] = v;
        self.len += 1;
        self.present |= 1u64 << (v.kind as u8);
        true
    }

    /// Remove a volatile by kind. No-op if absent.
    pub fn remove(&mut self, k: VolatileKind) {
        if let Some(i) = self.position(k) {
            let last = self.len as usize - 1;
            self.items[i] = self.items[last];
            self.items[last] = Volatile::default();
            self.len -= 1;
            self.present &= !(1u64 << (k as u8));
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
                    let dropped_kind = self.items[i].kind;
                    let last = self.len as usize - 1;
                    self.items[i] = self.items[last];
                    self.items[last] = Volatile::default();
                    self.len -= 1;
                    self.present &= !(1u64 << (dropped_kind as u8));
                    continue;
                }
            }
            i += 1;
        }
    }
}

/// One Pokémon. Phase 2 carries the minimum a damage calc needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pokemon {
    pub species_id: u16,
    pub level: u8,
    /// This individual's gender. Resolved at team build via PS precedence
    /// (explicit set gender → species fixed gender → ratio'd). For a
    /// ratio'd species with no explicit gender this is `Gender::Random`
    /// until the battle constructor rolls it 50/50 (PS rolls unspecified
    /// gender at `>player` with `sample(['M','F'])`); a fully built
    /// battle never leaves a mon `Random`. Currently informational — the
    /// gender-reading mechanics (Attract / Cute Charm / Rivalry) land in
    /// later PRs. PS `sim/pokemon.ts:339-341`.
    pub gender: data::Gender,
    pub moves: [u16; 4],
    pub pp: [u8; 4],
    pub ability_id: u16,
    /// Effective-ability override (PS `Pokemon.ability` reassignment via
    /// `setAbility`). `u16::MAX` = no override → `effective_ability_slug`
    /// falls back to `ability_id`. Set by Skill Swap (and, later, Gastro
    /// Acid / Worry Seed / Entrainment / Role Play / Simple Beam /
    /// Doodle). Reset to the sentinel on switch-out.
    pub ability_override: u16,
    pub item_id: u16,
    pub stats: FinalStats,
    pub current_hp: u16,
    /// This individual's IV spread. Stored so a mid-battle forme change
    /// (`Battle::set_forme` with `recompute_stats = true`) can recompute
    /// the 5 non-HP stats from the new species' base stats — PS keeps the
    /// `set` reference and recalculates `storedStats` on `formeChange`.
    /// Set at team build from the `TeamMember`. Recon / test builders that
    /// synthesize `stats` directly use canonical defaults here; the spread
    /// is never read unless the mon actually forme-changes.
    pub ivs: StatSpread,
    /// This individual's EV spread. See `ivs`.
    pub evs: StatSpread,
    /// This individual's nature, stored as a compact `u8` id (index into
    /// `NATURES`). Resolve with `nature_by_id`. Stored as an id rather than
    /// an embedded `&'static str`+two `Option<Stat>` to shrink the struct;
    /// only read on forme-change recompute. See `ivs`.
    pub nature_id: u8,
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
    /// Encoded `(side_byte, slot_byte)` of the most recent *physical*
    /// attacker this turn, and the HP damage it dealt. Counter
    /// (`data/moves.ts:counter`) keys off these specifically — PS tracks
    /// physical and special hits in separate `counter` / `mirrorcoat`
    /// volatiles, so a special hit landing after a physical one does NOT
    /// overwrite the value Counter retaliates against. `(255, 255)` / `0`
    /// = no physical hit recorded this turn. Cleared each turn alongside
    /// `last_attacker`.
    pub last_phys_attacker: (u8, u8),
    pub last_phys_damage: u16,
    /// Same as `last_phys_*` but for the most recent *special* attacker
    /// this turn. Read by Mirror Coat (`data/moves.ts:mirrorcoat`).
    pub last_spec_attacker: (u8, u8),
    pub last_spec_damage: u16,
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
    /// Held-item EFFECT suppression flag. While `true`, `effective_item_id()`
    /// reports `u16::MAX` (no item) so every item-effect read short-circuits,
    /// but the underlying `item_id` is left intact (the item is NOT removed).
    /// Set on all active Pokémon while Magic Room is up — PS models this via
    /// `Pokemon.ignoringItem()` returning true when the `magicroom`
    /// pseudo-weather is active (`sim/pokemon.ts:888`); we collapse that live
    /// field read into a per-mon bool maintained on Magic Room start/end and
    /// on switch-in (mirrors the `GravityGrounded` volatile pattern). Item
    /// PRESENCE reads (Acrobatics, Fling, Knock Off, Poltergeist) keep using
    /// the raw `item_id`, exactly as PS reads `pokemon.item` for those.
    pub item_suppressed: bool,
    /// Slow Start turn counter. PS `data/abilities.ts:slowstart` adds
    /// a 5-turn `slowstart` volatile on switch-in; while the volatile
    /// is alive the holder's Atk and Spe are halved. We model that as
    /// a turn counter set to 5 on switch-in (in `on_switch_in`),
    /// decremented at the end of each turn the holder remains on
    /// field. 0 = inactive / expired. Reset on switch-out.
    pub slow_start_active_turns: u8,
    /// Truant flag — PS `data/abilities.ts:truant` flips a per-mon
    /// boolean each turn; while `true`, the holder's move is skipped
    /// ("loafing around"). Reset on switch-out. PS adds the volatile
    /// on switch-in with `effectState.loafing = false`; we initialise
    /// to false (uses move on turn 1) and flip in the before-move arm.
    pub truant_loafing: bool,
    /// Runtime battle-type override (Protean / Libero / Color Change /
    /// Reflect Type / Conversion). `[255, 255]` = no override (use the
    /// species' innate types). Otherwise `type_override[0]` is the
    /// primary type code (0..=17) and `type_override[1]` is the
    /// secondary (255 = mono-type override). When set, this wins over
    /// the species types in `effective_types` — driving both offensive
    /// STAB and defensive type-effectiveness — but NOT over an active
    /// Tera type (Tera locks typing in gen 9). Reset on switch-out.
    /// PS analog: `Pokemon.setType` writing `pokemon.types`.
    pub type_override: [u8; 2],
    /// Protean / Libero once-per-switch-in latch (gen-9 nerf). PS tracks
    /// this as `this.effectState.protean` / `.libero` on the ability,
    /// which resets when the holder switches out (the ability's effect
    /// state is re-created on switch-in). `true` once the holder has
    /// changed type via Protean/Libero this stint; blocks further
    /// re-types until it switches out. Reset on switch-out alongside
    /// `clear_type_override`. PS data/abilities.ts:3452 / :2273.
    pub protean_used: bool,
    /// Disguise (Mimikyu) busted latch. `false` while the disguise is
    /// intact; set `true` the first time a damaging move would deal HP
    /// damage to a Disguise holder, at which point the move's damage is
    /// negated, the holder takes 1/8 max-HP chip and forme-changes to
    /// Mimikyu-Busted. Persists across the rest of the battle (Disguise
    /// only blocks once). Unlike most volatiles this is NOT reset on
    /// switch-out — PS models it via the species itself (the Busted forme
    /// stays busted), and the forme change is permanent. Initialised
    /// `false`; only ever set `true`. PS data/abilities.ts:960.
    pub disguise_busted: bool,
    /// Supersweet Syrup (Dipplin / Hydrapple) once-per-battle latch. PS
    /// `data/abilities.ts:4704` keys this off `pokemon.syrupTriggered`: the
    /// FIRST time the holder switches in, it lowers every adjacent foe's
    /// evasion by 1, then sets the flag so it never fires again — even
    /// across later switch-outs and back in. Like `disguise_busted` this is
    /// stored on the mon itself (not an effectState volatile), so it is
    /// deliberately NOT cleared by the blanket switch-in reset. Initialised
    /// `false`; only ever set `true`. PS data/abilities.ts:4704.
    pub syrup_triggered: bool,
    /// Micle Berry one-shot accuracy latch. PS `data/items.ts:micleberry`
    /// (line 4067): on the HP-trigger eat, the holder gains the `micleberry`
    /// volatile, which on its NEXT non-OHKO move multiplies that move's
    /// accuracy by 4915/4096 (×1.2) and then removes itself. We model the
    /// volatile as this single Copy latch: set `true` when the berry is
    /// eaten, consumed (cleared) the next time the holder uses a non-OHKO
    /// move (in the accuracy block). Reset to `false` on switch-out
    /// alongside the other single-turn volatiles.
    pub micle_next_move: bool,
    /// Unburden (PS `data/abilities.ts:unburden`) — `true` once this mon's
    /// held item has been used up or taken away. While set AND the mon is
    /// currently itemless, `order::effective_speed` doubles its Speed
    /// (PS's `unburden` volatile + its `onModifySpe` chainModify(2), which
    /// only fires when `!pokemon.item`). Reset to `false` on switch-out
    /// (PS `onEnd`); naturally stops applying if the mon regains an item.
    pub unburden_active: bool,
    /// Commander (Tatsugiri) — `true` while this mon is inside its ally
    /// Dondozo's mouth. PS `data/conditions.ts:commanding` volatile.
    /// While set the holder cannot act (auto-passes — PS `onBeforeTurn`
    /// `cancelAction`), cannot be targeted or hit (PS
    /// `hitStepInvulnerabilityEvent` returns false + `onInvulnerability:
    /// false`), and cannot switch / be dragged out (PS `onTrapPokemon` /
    /// `onDragOut`). Set in `Battle::commander_update`; cleared when the
    /// Dondozo ally is no longer alive (release) and on switch-out.
    pub commanding: bool,
    /// Commander (Dondozo) — `true` once this mon has received the +2-to-
    /// all-stats command from a Tatsugiri ally. PS
    /// `data/conditions.ts:commanded` volatile (its `onStart` applies the
    /// boost). Acts as the once-per-pairing guard so the boost is not
    /// re-applied on subsequent switch-in updates. Cleared on switch-out.
    pub commanded: bool,
    /// Cud Chew (Farigiraf signature) — the item id of the Berry this mon
    /// ate, scheduled to be re-eaten one more time at the end of the NEXT
    /// turn. `u16::MAX` = nothing pending. PS `data/abilities.ts:732`
    /// stores the eaten berry on `effectState.berry`; we latch the id here.
    /// Cleared on switch-out and after the re-eat fires.
    pub cud_chew_berry: u16,
    /// Cud Chew countdown. Set to 2 when a Berry is eaten; decremented in
    /// `ability::on_residual` each end-of-turn. The re-eat fires when it
    /// reaches 0 (i.e. the end of the turn AFTER the one it was eaten on),
    /// mirroring PS `effectState.counter`.
    pub cud_chew_counter: u8,
}

impl Pokemon {
    /// Construct a `Pokemon` from its identity fields, with every
    /// volatile/runtime field initialised to its inert battle-start
    /// default. This is the **single source of truth** for those defaults:
    /// adding a new runtime field means giving it a default here only,
    /// instead of touching every `Pokemon { .. }` literal across the
    /// codebase (team / recon / damage builders).
    ///
    /// The inert defaults below are exactly what the old hand-written
    /// literals set, so this is behavior-preserving. Callers that need a
    /// non-default runtime field (a pre-statused test mon, etc.) construct
    /// via this and then assign the field.
    #[allow(clippy::too_many_arguments)]
    pub fn with_identity(
        species_id: u16,
        level: u8,
        gender: data::Gender,
        moves: [u16; 4],
        pp: [u8; 4],
        ability_id: u16,
        item_id: u16,
        stats: FinalStats,
        current_hp: u16,
        ivs: StatSpread,
        evs: StatSpread,
        nature_id: u8,
        tera_type: u8,
    ) -> Self {
        Pokemon {
            // ---- identity (from params) ----
            species_id,
            level,
            gender,
            moves,
            pp,
            ability_id,
            item_id,
            stats,
            current_hp,
            ivs,
            evs,
            nature_id,
            tera_type,
            // ---- inert runtime / volatile defaults (single source) ----
            ability_override: u16::MAX,
            status: Status::None,
            boosts: [0; 7],
            fainted: false,
            turns_active: 0,
            last_used_move_slot: 255,
            boosted_stat: 255,
            booster_locked: false,
            ability_suppressed: false,
            item_suppressed: false,
            crit_stage_volatile: 0,
            last_attacker: (255, 255),
            last_attacker_category: 255,
            last_damage_taken: 0,
            last_phys_attacker: (255, 255),
            last_phys_damage: 0,
            last_spec_attacker: (255, 255),
            last_spec_damage: 0,
            terastallized: false,
            stellar_boosted_types: 0,
            semi_invuln: 0,
            charging_turns: 0,
            charging_move_slot: 255,
            must_recharge: false,
            lockin_turns: 0,
            lockin_move_slot: 255,
            volatiles: VolatileSet::default(),
            slow_start_active_turns: 0,
            truant_loafing: false,
            type_override: [255, 255],
            protean_used: false,
            disguise_busted: false,
            syrup_triggered: false,
            micle_next_move: false,
            unburden_active: false,
            commanding: false,
            commanded: false,
            cud_chew_berry: u16::MAX,
            cud_chew_counter: 0,
        }
    }

    pub fn species(&self) -> &'static data::SpeciesDef {
        &data::SPECIES[self.species_id as usize]
    }

    /// Effective in-battle weight in hectograms (kg × 10), after held-item
    /// weight modifiers. Currently only Float Stone applies (halves weight).
    /// PS `data/items.ts:floatstone` (line 2172): `onModifyWeight(weight) {
    /// return this.trunc(weight / 2); }`, and `sim/pokemon.ts:getWeight()`
    /// clamps the result to `Math.max(weight, 0.1)` kg — i.e. ≥ 1 hg here —
    /// so Low Kick's BP table can't see a 0-weight target.
    /// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Float_Stone>.
    pub fn effective_weight_dg(&self) -> u32 {
        let base = self.species().weight_dg as u32;
        // `effective_item_id()` is `u16::MAX` under Magic Room — Float Stone's
        // weight halving is an item effect, so it is suppressed (item kept).
        if self.effective_item_id() == data::item_id::FLOATSTONE {
            (base / 2).max(1)
        } else {
            base
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.fainted && self.current_hp > 0
    }

    /// True if the holder negates its own TYPE-based immunities — i.e. it
    /// holds Ring Target. PS `data/items.ts:5222` sets `onNegateImmunity:
    /// false`, which makes `Battle.runEvent('NegateImmunity')` return a
    /// falsy value so `runImmunity` treats the type chart's 0× as "not
    /// immune". This negates ONLY type-chart immunities (Ground vs Flying,
    /// Normal/Fighting vs Ghost, Ghost vs Normal, Poison vs Steel, etc.).
    /// It does NOT negate ability/item immunities (Levitate, Air Balloon),
    /// which PS resolves separately in `isGrounded()` AFTER the Flying-type
    /// check — those are gated below in `is_grounded_internal`.
    /// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Ring_Target>.
    pub fn negates_type_immunity(&self) -> bool {
        // Ring Target's immunity-negation is an item effect — suppressed
        // under Magic Room (`effective_item_id()` reports no item).
        self.effective_item_id() == data::item_id::RINGTARGET
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
        } else if self.type_override[0] != 255 {
            // Runtime type override (Protean / Color Change / ...).
            if self.type_override[1] == 255 {
                ([self.type_override[0], 0], 1)
            } else {
                (self.type_override, 2)
            }
        } else {
            (s.types, s.num_types)
        }
    }

    /// Apply a runtime battle-type override (Protean / Libero / Color
    /// Change / Reflect Type / Conversion). `secondary == None` makes
    /// the mon mono-typed. Wins over the species types in
    /// `effective_types`, but not over an active Tera type. Cleared on
    /// switch-out. PS analog: `Pokemon.setType`.
    #[inline]
    pub fn set_type_override(&mut self, primary: u8, secondary: Option<u8>) {
        self.type_override = [primary, secondary.unwrap_or(255)];
    }

    /// Remove any runtime type override, reverting to the species types.
    #[inline]
    pub fn clear_type_override(&mut self) {
        self.type_override = [255, 255];
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
        if self.effective_ability_id() == data::ability_id::SUPERLUCK {
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
        // Skill Swap (and future ability-setting moves) reassign the
        // live ability via `ability_override`; it shadows `ability_id`
        // while present (PS reassigns `Pokemon.ability` directly).
        let id = if self.ability_override != u16::MAX {
            self.ability_override
        } else {
            self.ability_id
        };
        if id == u16::MAX {
            return "";
        }
        data::ABILITIES.get(id as usize).map(|a| a.slug).unwrap_or("")
    }

    /// Effective ability id — the integer-dispatch mirror of
    /// `effective_ability_slug()`. Returns `u16::MAX` (the "no ability"
    /// sentinel) when the ability is suppressed (Gastro Acid) or the slot
    /// is empty, otherwise the override-aware `ability_id`. Hot-path code
    /// should compare this against `data::ability_id::*` constants instead
    /// of materializing the slug and running a `strcmp`. The two accessors
    /// are kept in exact lockstep: `effective_ability_slug()` returns
    /// `""` iff this returns `u16::MAX`.
    #[inline]
    pub fn effective_ability_id(&self) -> u16 {
        if self.ability_suppressed {
            return u16::MAX;
        }
        if self.ability_override != u16::MAX {
            self.ability_override
        } else {
            self.ability_id
        }
    }

    /// Effective held-item id — `u16::MAX` when no item is held. Mirror of
    /// the inline `item_id == u16::MAX ? "" : ITEMS[item_id].slug` pattern
    /// used across the hot path, letting call sites compare against
    /// `data::item_id::*` constants without the slug round-trip. (There is
    /// for call-site symmetry with the ability accessor.) When
    /// `item_suppressed` is set (Magic Room — PS `Pokemon.ignoringItem()`
    /// true while the `magicroom` pseudo-weather is up, `sim/pokemon.ts:888`)
    /// this reports `u16::MAX` so item-EFFECT reads see no item, while the
    /// raw `item_id` field is left untouched (the item is suppressed, not
    /// removed). Presence reads (Acrobatics / Fling / Knock Off / Poltergeist)
    /// deliberately read `item_id` directly, matching PS's `pokemon.item`.
    #[inline]
    pub fn effective_item_id(&self) -> u16 {
        if self.item_suppressed {
            return u16::MAX;
        }
        self.item_id
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

    /// Remaining Disable turns; `0` when not disabled. Stored in the
    /// volatile's `turns_remaining`; the disabled move slot rides in
    /// `payload` (see the `Disable` VolatileKind doc).
    #[inline]
    pub fn disable_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Disable)
            .map(|v| v.turns_remaining)
            .unwrap_or(0)
    }

    /// Disabled move slot (0..=3). 255 if not disabled.
    #[inline]
    pub fn disabled_move_slot(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Disable)
            .map(|v| v.payload as u8)
            .unwrap_or(255)
    }

    /// Apply a Disable lock for `turns` on move slot `slot`. `turns == 0`
    /// clears the lock instead. PS data/moves.ts:disable `condition`
    /// uses `duration: 5` with an `onStart` decrement when the target
    /// still has its move queued — callers pass the already-resolved
    /// effective duration (4 in the common case).
    #[inline]
    pub fn set_disable(&mut self, turns: u8, slot: u8) {
        if turns == 0 {
            self.volatiles.remove(VolatileKind::Disable);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Disable,
                turns_remaining: turns,
                payload: slot as u32 & 0xFF,
            });
        }
    }

    /// Clear the Disable volatile.
    #[inline]
    pub fn clear_disable(&mut self) {
        self.volatiles.remove(VolatileKind::Disable);
    }

    /// `true` while infatuated (the Attract volatile is present).
    #[inline]
    pub fn is_attracted(&self) -> bool {
        self.volatiles.has(VolatileKind::Attract)
    }

    /// The Attract source as `(source_side, source_team_index)` where
    /// `source_side` is 0 (P1) / 1 (P2) and `source_team_index` is the
    /// source mon's roster slot. `None` when not infatuated. See the
    /// `Attract` `VolatileKind` doc for the payload encoding.
    #[inline]
    pub fn attract_source(&self) -> Option<(u8, u8)> {
        self.volatiles.get(VolatileKind::Attract).map(|v| {
            let side = ((v.payload >> 8) & 1) as u8;
            let idx = (v.payload & 0xFF) as u8;
            (side, idx)
        })
    }

    /// Apply the Attract (infatuation) volatile, recording the source mon
    /// by side (0 = P1, 1 = P2) + team roster index. Re-application
    /// replaces (PS `volatileStatus` add is idempotent; callers gate the
    /// already-attracted no-op themselves).
    #[inline]
    pub fn set_attract(&mut self, source_side: u8, source_team_index: u8) {
        let payload = (((source_side & 1) as u32) << 8) | (source_team_index as u32 & 0xFF);
        self.volatiles.add(Volatile {
            kind: VolatileKind::Attract,
            turns_remaining: 0,
            payload,
        });
    }

    /// Clear the Attract volatile.
    #[inline]
    pub fn clear_attract(&mut self) {
        self.volatiles.remove(VolatileKind::Attract);
    }

    /// `true` while the Charge volatile is up — the holder's next
    /// Electric move gets ×2 BP. PS `data/conditions.ts:charge`.
    #[inline]
    pub fn is_charged(&self) -> bool {
        self.volatiles.has(VolatileKind::Charge)
    }

    /// Set / clear the Charge volatile (Charge move / Wind Power).
    #[inline]
    pub fn set_charged(&mut self, on: bool) {
        if on {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Charge,
                turns_remaining: 0,
                payload: 0,
            });
        } else {
            self.volatiles.remove(VolatileKind::Charge);
        }
    }

    /// End-of-turn Disable countdown. Decrements the remaining turns and
    /// drops the volatile when it hits 0. Mirrors the Encore tick (the
    /// battle loop manages durations explicitly rather than via
    /// `VolatileSet::tick`). No-op when not disabled.
    #[inline]
    pub fn tick_disable(&mut self) {
        let Some(pos) = self.volatiles.position(VolatileKind::Disable) else { return };
        let rem = {
            let v = &mut self.volatiles.items[pos];
            if v.turns_remaining == 0 {
                return;
            }
            v.turns_remaining -= 1;
            v.turns_remaining
        };
        if rem == 0 {
            self.volatiles.remove(VolatileKind::Disable);
        }
    }

    /// Remaining Throat Chop lockout turns. `0` if not locked out. While
    /// > 0 the holder cannot use sound-flagged moves. PS
    /// `data/moves.ts:throatchop` condition `duration: 2`.
    #[inline]
    pub fn throat_chop_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::ThroatChop)
            .map(|v| v.turns_remaining)
            .unwrap_or(0)
    }

    /// Apply the Throat Chop lockout for `turns` (PS uses `duration: 2`).
    /// Re-application replaces (PS `addVolatile` resets the duration).
    /// `turns == 0` clears instead.
    #[inline]
    pub fn set_throat_chop(&mut self, turns: u8) {
        if turns == 0 {
            self.volatiles.remove(VolatileKind::ThroatChop);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::ThroatChop,
                turns_remaining: turns,
                payload: 0,
            });
        }
    }

    /// End-of-turn Throat Chop countdown — mirrors `tick_disable`. PS
    /// `data/moves.ts:throatchop` condition `onResidualOrder 22`.
    #[inline]
    pub fn tick_throat_chop(&mut self) {
        let Some(pos) = self.volatiles.position(VolatileKind::ThroatChop) else { return };
        let rem = {
            let v = &mut self.volatiles.items[pos];
            if v.turns_remaining == 0 {
                return;
            }
            v.turns_remaining -= 1;
            v.turns_remaining
        };
        if rem == 0 {
            self.volatiles.remove(VolatileKind::ThroatChop);
        }
    }

    /// Remaining Heal Block turns. `0` if not heal-blocked. While > 0 the
    /// holder cannot gain HP from any healing source (Recover-class moves,
    /// drain, Leftovers/Black Sludge, berries, Wish, Leech Seed, Poison
    /// Heal, …) and heal-flagged moves fail outright. PS Heal Block
    /// condition `data/moves.ts:healblock` (`duration: 5`, overridden to 2
    /// by Psychic Noise's `durationCallback`).
    #[inline]
    pub fn heal_block_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::HealBlock)
            .map(|v| v.turns_remaining)
            .unwrap_or(0)
    }

    /// True while the Heal Block volatile is active. Mirrors PS Heal Block's
    /// `onTryHeal` (returns `false`) — every heal into this mon is vetoed.
    #[inline]
    pub fn is_heal_blocked(&self) -> bool {
        self.volatiles.has(VolatileKind::HealBlock)
    }

    /// Apply the Heal Block lockout for `turns`. Re-application replaces
    /// (PS `addVolatile` resets the duration; Psychic Noise's `onRestart`
    /// is a no-op refresh). `turns == 0` clears instead.
    #[inline]
    pub fn set_heal_block(&mut self, turns: u8) {
        if turns == 0 {
            self.volatiles.remove(VolatileKind::HealBlock);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::HealBlock,
                turns_remaining: turns,
                payload: 0,
            });
        }
    }

    /// End-of-turn Heal Block countdown — mirrors `tick_throat_chop`. PS
    /// Heal Block condition `onResidualOrder 20` — counts down each end of
    /// turn, ends at 0. Cleared on switch-out via the blanket volatile reset.
    #[inline]
    pub fn tick_heal_block(&mut self) {
        let Some(pos) = self.volatiles.position(VolatileKind::HealBlock) else { return };
        let rem = {
            let v = &mut self.volatiles.items[pos];
            if v.turns_remaining == 0 {
                return;
            }
            v.turns_remaining -= 1;
            v.turns_remaining
        };
        if rem == 0 {
            self.volatiles.remove(VolatileKind::HealBlock);
        }
    }

    /// Remaining Perish Song count. `0` if not under Perish Song. PS stores
    /// this as the volatile's `duration` (`data/moves.ts:perishsong`
    /// condition, `duration: 4`). The value displayed to a player as the
    /// "perish count" equals this number AFTER each end-of-turn decrement,
    /// so it reads 3 → 2 → 1 → 0(faint) over the four residual phases that
    /// follow application (the use-turn residual ticks 4 → 3).
    #[inline]
    pub fn perish_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::PerishSong)
            .map(|v| v.turns_remaining)
            .unwrap_or(0)
    }

    /// Apply Perish Song to this mon. PS `data/moves.ts:perishsong`
    /// `onHitField` only adds the volatile when the mon does not already
    /// have it (`!pokemon.volatiles['perishsong']` — no reset), so this is
    /// a no-op if already perished. `duration: 4` (see `perish_turns`).
    #[inline]
    pub fn set_perish_song(&mut self) {
        if self.volatiles.has(VolatileKind::PerishSong) {
            return;
        }
        self.volatiles.add(Volatile {
            kind: VolatileKind::PerishSong,
            turns_remaining: 4,
            payload: 0,
        });
    }

    /// End-of-turn Perish Song countdown — mirrors `tick_throat_chop` but
    /// faints the holder when the counter reaches 0. PS
    /// `data/moves.ts:perishsong` condition `onResidualOrder: 24`,
    /// `onEnd`: `target.faint()`. Returns `true` if the mon fainted on
    /// this tick so the caller can run faint bookkeeping.
    #[inline]
    pub fn tick_perish_song(&mut self) -> bool {
        let Some(pos) = self.volatiles.position(VolatileKind::PerishSong) else {
            return false;
        };
        let rem = {
            let v = &mut self.volatiles.items[pos];
            if v.turns_remaining == 0 {
                return false;
            }
            v.turns_remaining -= 1;
            v.turns_remaining
        };
        if rem == 0 {
            self.volatiles.remove(VolatileKind::PerishSong);
            self.current_hp = 0;
            self.fainted = true;
            return true;
        }
        false
    }

    /// Failure-roll denominator for the next consecutive Ally Switch use,
    /// or `0` if no `AllySwitch` volatile is active (the next use is the
    /// start of a chain and always succeeds). PS `effectState.counter`.
    #[inline]
    pub fn ally_switch_counter(&self) -> u32 {
        self.volatiles.get(VolatileKind::AllySwitch).map(|v| v.payload).unwrap_or(0)
    }

    /// Add/refresh the Ally Switch consecutive-use volatile with the given
    /// next-use denominator (PS `addVolatile` resetting `duration` to 2 on
    /// `onStart`/`onRestart`). Re-application replaces.
    #[inline]
    pub fn set_ally_switch_volatile(&mut self, next_counter: u32) {
        self.volatiles.add(Volatile {
            kind: VolatileKind::AllySwitch,
            turns_remaining: 2,
            payload: next_counter,
        });
    }

    /// Drop the Ally Switch volatile (PS `delete pokemon.volatiles['allyswitch']`
    /// on a failed consecutive-use roll — the chain resets to 100%).
    #[inline]
    pub fn clear_ally_switch(&mut self) {
        self.volatiles.remove(VolatileKind::AllySwitch);
    }

    /// End-of-turn Ally Switch countdown — mirrors `tick_throat_chop`. PS
    /// `data/moves.ts:allyswitch` condition `duration: 2`: the volatile
    /// survives the use-turn and the immediately following turn, so using
    /// the move on two consecutive turns keeps the chain alive while a
    /// one-turn gap lets it expire (counter resets to 100%).
    #[inline]
    pub fn tick_ally_switch(&mut self) {
        let Some(pos) = self.volatiles.position(VolatileKind::AllySwitch) else { return };
        let rem = {
            let v = &mut self.volatiles.items[pos];
            if v.turns_remaining == 0 {
                return;
            }
            v.turns_remaining -= 1;
            v.turns_remaining
        };
        if rem == 0 {
            self.volatiles.remove(VolatileKind::AllySwitch);
        }
    }

    /// Remaining Taunt turns. `0` if not taunted. While > 0 the holder
    /// cannot select or use Status-category moves. PS
    /// `data/moves.ts:taunt` condition `duration: 3`.
    #[inline]
    pub fn taunt_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Taunt)
            .map(|v| v.turns_remaining)
            .unwrap_or(0)
    }

    /// Apply the Taunt lockout for `turns` (PS uses `duration: 3`, bumped
    /// to 4 on apply if the target has already acted this turn). Re-
    /// application replaces (PS `addVolatile` resets the duration).
    /// `turns == 0` clears instead.
    #[inline]
    pub fn set_taunt(&mut self, turns: u8) {
        if turns == 0 {
            self.volatiles.remove(VolatileKind::Taunt);
        } else {
            self.volatiles.add(Volatile {
                kind: VolatileKind::Taunt,
                turns_remaining: turns,
                payload: 0,
            });
        }
    }

    /// End-of-turn Taunt countdown — mirrors `tick_throat_chop`. PS
    /// `data/moves.ts:taunt` condition `onResidualOrder 15`.
    #[inline]
    pub fn tick_taunt(&mut self) {
        let Some(pos) = self.volatiles.position(VolatileKind::Taunt) else { return };
        let rem = {
            let v = &mut self.volatiles.items[pos];
            if v.turns_remaining == 0 {
                return;
            }
            v.turns_remaining -= 1;
            v.turns_remaining
        };
        if rem == 0 {
            self.volatiles.remove(VolatileKind::Taunt);
        }
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

    /// Remaining Confusion turns (stored in the volatile `payload`). `0`
    /// if not confused. Read-only accessor for the Python observation API.
    #[inline]
    pub fn confusion_turns(&self) -> u8 {
        self.volatiles
            .get(VolatileKind::Confusion)
            .map(|v| v.payload as u8)
            .unwrap_or(0)
    }

    /// `true` if this mon is seeded by Leech Seed. Read-only accessor for
    /// the Python observation API.
    #[inline]
    pub fn has_leech_seed(&self) -> bool {
        self.volatiles.has(VolatileKind::LeechSeed)
    }

    /// `true` if this mon is under Salt Cure. Read-only accessor for the
    /// Python observation API.
    #[inline]
    pub fn has_salt_cure(&self) -> bool {
        self.volatiles.has(VolatileKind::SaltCure)
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
        // Iron Ball — PS `data/items.ts:ironball` `onEffectiveness` and
        // `gravity`-style ground override: the holder ALWAYS counts as
        // grounded, suppressing Flying-type immunity, Levitate, Magnet
        // Rise, and Air Balloon. Mirrored in PS's
        // `sim/pokemon.ts:isGrounded()` which checks for `ironball` and
        // returns true before the standard untrue-grounding checks.
        // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Iron_Ball>.
        if self.effective_item_id() == data::item_id::IRONBALL {
            return true;
        }
        // Smack Down / Thousand Arrows grounding — PS
        // `data/conditions.ts:smackdown` checked early in PS's
        // `Pokemon.isGrounded()`. Overrides Flying type, Levitate,
        // Air Balloon, and Magnet Rise.
        if self.volatiles.has(VolatileKind::SmackdownGrounded) {
            return true;
        }
        // Gravity — while the field condition is up every Pokémon is
        // grounded (PS `Pokemon.isGrounded()` checks
        // `this.battle.field.getPseudoWeather('gravity')`). The battle
        // keeps this marker in sync with `gravity_turns`. Overrides
        // Flying type, Levitate, Air Balloon, Magnet Rise, Telekinesis.
        if self.volatiles.has(VolatileKind::GravityGrounded) {
            return true;
        }
        // Ring Target negates the Flying-TYPE airborne immunity (PS's
        // `isGrounded(negateImmunity)` skips the Flying-type branch when
        // `negateImmunity` is set), but does NOT bypass Levitate or Air
        // Balloon — those are checked after the Flying branch in PS and are
        // unaffected. So a Flying-type Ring Target holder grounds out, while
        // a Levitate / Air Balloon Ring Target holder stays airborne.
        let negate_type_immunity = self.effective_item_id() == data::item_id::RINGTARGET;
        let s = self.species();
        let flying = (0..s.num_types as usize).any(|i| s.types[i] == 9);
        if flying && !negate_type_immunity {
            return false;
        }
        // Eelevate (Pokémon Champions, Mega Eelektross) grants Levitate's
        // grounding immunity in addition to its on-KO boost. Serebii: "The
        // Pokémon floats off the ground, making it immune to Ground-type
        // moves, as well as the Spikes, Toxic Spikes, and Sticky Web statuses."
        // (serebii.net/pokemonchampions/newabilities.shtml)
        let ab = self.effective_ability_id();
        if (ab == data::ability_id::LEVITATE || ab == data::ability_id::EELEVATE)
            && !ignore_levitate
        {
            return false;
        }
        if self.effective_item_id() == data::item_id::AIRBALLOON {
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
        let mut mon = Pokemon::with_identity(
            species_idx, 50, data::Gender::Male,
            [u16::MAX; 4], [0; 4],
            u16::MAX, u16::MAX, FinalStats::default(), 1,
            StatSpread::MAX_IV, StatSpread::default(), nature_id::NEUTRAL,
            1, /* fire */
        );
        let (types, n) = mon.effective_types();
        assert_eq!(n, species.num_types);
        assert_eq!(types, species.types);
        // After Tera: single Fire type.
        mon.terastallized = true;
        let (types2, n2) = mon.effective_types();
        assert_eq!(n2, 1);
        assert_eq!(types2[0], 1);
    }

    /// The hand-written `nature_id::*` constants must match the `NATURES`
    /// table positions they name — the contract `nature_by_id` relies on.
    #[test]
    fn nature_ids_match_table() {
        assert_eq!(NATURES[nature_id::HARDY as usize].slug, "hardy");
        assert_eq!(NATURES[nature_id::ADAMANT as usize].slug, "adamant");
        assert_eq!(NATURES[nature_id::JOLLY as usize].slug, "jolly");
        assert_eq!(NATURES[nature_id::CAREFUL as usize].slug, "careful");
        assert_eq!(NATURES[nature_id::QUIRKY as usize].slug, "quirky");
        assert_eq!(nature_id::NEUTRAL, nature_id::HARDY);
        // Every constant resolves and round-trips through nature_id_by_slug.
        for (i, n) in NATURES.iter().enumerate() {
            assert_eq!(nature_id_by_slug(n.slug), Some(i as u8));
            assert_eq!(nature_by_id(i as u8).slug, n.slug);
        }
    }

    #[test]
    fn effective_ability_slug_respects_suppression() {
        let species = data::species_by_slug("garchomp").expect("garchomp");
        let ab_id = data::ABILITIES.iter().position(|a| a.slug == "roughskin").unwrap() as u16;
        let mon = Pokemon::with_identity(
            species.num, 50, data::Gender::Male,
            [u16::MAX; 4], [0; 4],
            ab_id, u16::MAX, FinalStats::default(), 100,
            StatSpread::MAX_IV, StatSpread::default(), nature_id::NEUTRAL,
            0,
        );
        assert_eq!(mon.effective_ability_slug(), "roughskin");
        // Id accessor must stay in lockstep with the slug accessor.
        assert_eq!(mon.effective_ability_id(), ab_id);
        assert_eq!(data::ABILITIES[mon.effective_ability_id() as usize].slug, "roughskin");
        let mut sup = mon.clone();
        sup.ability_suppressed = true;
        assert_eq!(sup.effective_ability_slug(), "");
        assert_eq!(sup.effective_ability_id(), u16::MAX);
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
