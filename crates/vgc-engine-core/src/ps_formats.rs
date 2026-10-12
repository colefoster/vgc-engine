//! Pokémon Showdown's gen 9 formats, as far as a battle start needs them:
//! the `|tier|` name, game type, `|rule|` lines (rule-table order), team
//! preview and its picked size, Open Team Sheets, debug (exact HP) and
//! Cancel Mod. `ps_formats.json` is generated from PS itself by
//! `tools/protocol/ps-formats.js` (one battle start per format; PS
//! config/formats.ts + data/rulesets.ts), restricted to the mods the engine
//! models: standard gen 9 and Champions.

use std::sync::OnceLock;

/// One PS format.
#[derive(Debug, Clone)]
pub struct PsFormat {
    pub id: &'static str,
    /// The format name PS prints as `|tier|`.
    pub name: &'static str,
    pub doubles: bool,
    /// The PS mod (`gen9`, `gen9predlc`, `champions`, `championsregmb`).
    pub mod_id: &'static str,
    pub rules: &'static [&'static str],
    pub team_preview: bool,
    /// `pickedTeamSize`; 0 = the whole team.
    pub picked_team_size: u8,
    pub open_team_sheets: bool,
    /// A `debug` format (exact HP for everyone).
    pub debug: bool,
    /// Cancel Mod (data/rulesets.ts cancelmod): a side may undo its choice.
    pub support_cancel: bool,
}

impl PsFormat {
    pub fn champions(&self) -> bool {
        self.mod_id.starts_with("champions")
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn table() -> &'static [PsFormat] {
    static T: OnceLock<Vec<PsFormat>> = OnceLock::new();
    T.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(include_str!("ps_formats.json")).expect("ps_formats.json");
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|f| {
                        let s = |k: &str| leak(f[k].as_str().unwrap_or(""));
                        let rules: Vec<&'static str> =
                            f["rules"].as_array().map(|r| r.iter().map(|x| leak(x.as_str().unwrap_or(""))).collect()).unwrap_or_default();
                        PsFormat {
                            id: s("id"),
                            name: s("name"),
                            doubles: f["gameType"].as_str() == Some("doubles"),
                            mod_id: s("mod"),
                            rules: Box::leak(rules.into_boxed_slice()),
                            team_preview: f["teamPreview"].as_bool().unwrap_or(false),
                            picked_team_size: f["pickedTeamSize"].as_u64().unwrap_or(0) as u8,
                            open_team_sheets: f["openTeamSheets"].as_bool().unwrap_or(false),
                            debug: f["debug"].as_bool().unwrap_or(false),
                            support_cancel: f["supportCancel"].as_bool().unwrap_or(false),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// PS `toID`.
fn to_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// The format with this id (PS `toID` applied), if the engine knows it.
pub fn lookup(id: &str) -> Option<&'static PsFormat> {
    let id = to_id(id);
    table().iter().find(|f| f.id == id)
}

/// Every known format id.
pub fn ids() -> impl Iterator<Item = &'static str> {
    table().iter().map(|f| f.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_the_vgc_formats() {
        let mc = lookup("gen9championsvgc2026regmc").unwrap();
        assert_eq!(mc.name, "[Gen 9 Champions] VGC 2026 Reg M-C");
        assert!(mc.doubles && mc.team_preview && mc.open_team_sheets && mc.champions());
        assert_eq!(mc.picked_team_size, 4);
        let i = lookup("gen9vgc2025regi").unwrap();
        assert_eq!(i.name, "[Gen 9] VGC 2025 Reg I");
        assert_eq!(i.rules, &["Species Clause: Limit one of each Pokémon", "Item Clause: Limit 1 of each item"]);
        assert!(!i.champions() && i.doubles && i.picked_team_size == 4);
        // PS toID()s the format name, so a name finds its id.
        assert_eq!(lookup("[Gen 9] Doubles Custom Game").unwrap().id, "gen9doublescustomgame");
        assert!(lookup("gen9doublescustomgame").unwrap().debug);
        assert!(!lookup("gen9ou").unwrap().doubles);
    }
}
