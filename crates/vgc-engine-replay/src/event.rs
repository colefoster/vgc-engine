//! PS protocol events.
//!
//! Coverage is intentionally narrow: only the message types we need to drive
//! the replay-differential harness. Unrecognized lines map to [`Event::Other`]
//! so the parser is lossless at the line level.

use serde::{Deserialize, Serialize};

/// A position-bound Pokémon reference, e.g. `p1a: Sneasler` → `PokeSlot { player: 1, slot: 'a', nickname: "Sneasler" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeSlot {
    pub player: u8,
    pub slot: char,
    pub nickname: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// `|turn|N`
    Turn(u32),
    /// `|start`
    Start,
    /// `|upkeep`
    Upkeep,
    /// `|win|<player-name>`
    Win(String),
    /// `|tie`
    Tie,

    /// `|move|<user>|<move-name>|<target>`
    Move {
        user: PokeSlot,
        move_name: String,
        target: Option<PokeSlot>,
    },
    /// `|switch|<slot>|<details>|<hp>`
    Switch {
        slot: PokeSlot,
        details: String,
        hp: String,
    },
    /// `|drag|<slot>|<details>|<hp>`
    Drag {
        slot: PokeSlot,
        details: String,
        hp: String,
    },
    /// `|faint|<slot>`
    Faint(PokeSlot),

    /// `|-damage|<slot>|<hp>|...`
    Damage {
        slot: PokeSlot,
        hp: String,
        from: Option<String>,
    },
    /// `|-crit|<slot>` — PS emits this immediately after the `|move|`
    /// line for the hit, before the `|-damage|` line. Used by the
    /// Oracle harness to record each damaging hit's crit outcome.
    Crit(PokeSlot),
    /// `|-miss|<source>|<target>` — accuracy roll failed for `source`
    /// hitting `target`. PS only emits this when the accuracy gate
    /// fires; status/Protect/immunity refusals use other markers.
    /// Used by the Oracle harness to record per-hit accuracy outcomes.
    Miss { source: PokeSlot, target: Option<PokeSlot> },
    /// `|-heal|<slot>|<hp>|...`
    Heal {
        slot: PokeSlot,
        hp: String,
        from: Option<String>,
    },
    /// `|-boost|<slot>|<stat>|<amount>`
    Boost { slot: PokeSlot, stat: String, amount: i8 },
    /// `|-unboost|<slot>|<stat>|<amount>` (amount is positive in the protocol)
    Unboost { slot: PokeSlot, stat: String, amount: i8 },
    /// `|-status|<slot>|<status>`
    Status { slot: PokeSlot, status: String },
    /// `|-ability|<slot>|<ability>|...` — ability reveal (e.g. trace, weather trigger, Stamina proc).
    Ability {
        slot: PokeSlot,
        ability: String,
        from: Option<String>,
    },
    /// `|-item|<slot>|<item>|...` — item reveal (Frisk, Pickup, etc.).
    Item {
        slot: PokeSlot,
        item: String,
        from: Option<String>,
    },
    /// `|-enditem|<slot>|<item>|...` — item consumed/lost (Sitrus, Focus Sash, Knock Off).
    EndItem {
        slot: PokeSlot,
        item: String,
        from: Option<String>,
    },
    /// `|-cureststatus|<slot>|<status>`
    CureStatus { slot: PokeSlot, status: String },

    /// `|-weather|<weather>|...`
    Weather {
        weather: String,
        from: Option<String>,
        of: Option<PokeSlot>,
    },
    /// `|-fieldstart|move: <terrain>|...`
    FieldStart {
        effect: String,
        from: Option<String>,
        of: Option<PokeSlot>,
    },
    /// `|-fieldend|move: <terrain>`
    FieldEnd { effect: String },
    /// `|-sidestart|<side>|<effect>` (Reflect / Light Screen / Tailwind / etc.)
    SideStart { side: String, effect: String },
    /// `|-sideend|<side>|<effect>`
    SideEnd { side: String, effect: String },

    /// Team-preview entry: `|poke|<player>|<details>|<item-marker?>`
    Poke {
        player: u8,
        details: String,
    },
    /// `|teamsize|<player>|<n>`
    TeamSize { player: u8, size: u8 },
    /// `|teampreview` or `|teampreview|<n>`
    TeamPreview(Option<u8>),
    /// `|clearpoke`
    ClearPoke,

    /// Any line the parser doesn't model. The raw line is preserved (sans the
    /// leading `|`) so downstream code can grep it or escalate.
    Other(String),
}
