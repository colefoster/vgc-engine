//! Build-time-generated dex tables. All data here comes from
//! `@pkmn/dex` (vendored at `~/Dev/localdex`) via the crate's `build.rs`.
//!
//! Phase 1 surface: enough to prove the codegen pipeline works. Phase 2 will
//! grow the structs as mechanics need them.

#![forbid(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/data_tables.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_chart_is_full() {
        assert_eq!(TYPE_NAMES.len(), 18);
        // Sanity: Ghost is immune to Normal.
        let ghost = TYPE_NAMES.iter().position(|t| *t == "Ghost").unwrap();
        let normal = TYPE_NAMES.iter().position(|t| *t == "Normal").unwrap();
        assert_eq!(TYPE_CHART[ghost][normal], 3, "Ghost should be immune to Normal");
        // Fire is weak to Water.
        let fire = TYPE_NAMES.iter().position(|t| *t == "Fire").unwrap();
        let water = TYPE_NAMES.iter().position(|t| *t == "Water").unwrap();
        assert_eq!(TYPE_CHART[fire][water], 1, "Fire should be weak to Water");
    }

    #[test]
    fn slug_lookup_finds_known_entries() {
        assert!(move_by_slug("tackle").is_some());
        assert!(species_by_slug("pikachu").is_some());
    }
}
