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

    /// PS move flags populate the new MoveDef bool fields.
    /// Verified against PS `data/moves.ts` flag entries.
    #[test]
    fn move_flag_fields_populated() {
        let icepunch = move_by_slug("icepunch").unwrap();
        assert!(icepunch.is_punch);
        assert!(icepunch.makes_contact);

        let crunch = move_by_slug("crunch").unwrap();
        assert!(crunch.is_bite);

        let darkpulse = move_by_slug("darkpulse").unwrap();
        assert!(darkpulse.is_pulse);

        let shadowball = move_by_slug("shadowball").unwrap();
        assert!(shadowball.is_bullet);

        let swordsdance = move_by_slug("swordsdance").unwrap();
        assert!(swordsdance.is_dance);

        let spore = move_by_slug("spore").unwrap();
        assert!(spore.is_powder);

        let recover = move_by_slug("recover").unwrap();
        assert!(recover.is_heal);

        // Tackle has none of these flags.
        let tackle = move_by_slug("tackle").unwrap();
        assert!(!tackle.is_punch);
        assert!(!tackle.is_bite);
        assert!(!tackle.is_pulse);
        assert!(!tackle.is_bullet);
        assert!(!tackle.is_dance);
        assert!(!tackle.is_powder);
        assert!(!tackle.is_heal);
    }

    /// `is_nfe` is set for species that can still evolve.
    /// Verified against PS `data/pokedex.ts` `evos` field.
    #[test]
    fn species_nfe_flag_populated() {
        // Ivysaur evolves to Venusaur → NFE.
        assert!(species_by_slug("ivysaur").unwrap().is_nfe);
        // Chansey evolves to Blissey → NFE (Eviolite Chansey is the
        // canonical Eviolite user).
        assert!(species_by_slug("chansey").unwrap().is_nfe);
        // Dusclops evolves to Dusknoir → NFE.
        assert!(species_by_slug("dusclops").unwrap().is_nfe);
        // Venusaur is fully evolved.
        assert!(!species_by_slug("venusaur").unwrap().is_nfe);
        // Pikachu evolves to Raichu → NFE (yes, even though it's the
        // mascot — Eviolite Pikachu is legal, however unwise).
        assert!(species_by_slug("pikachu").unwrap().is_nfe);
    }

    /// Weights round-trip from `@pkmn/dex` `weightkg` into decigrams.
    /// Verified against PS `data/pokedex.ts` weightkg field.
    #[test]
    fn species_weights_populated() {
        // Pikachu = 6.0 kg → 60 dg.
        assert_eq!(species_by_slug("pikachu").unwrap().weight_dg, 60);
        // Snorlax = 460.0 kg → 4600 dg.
        assert_eq!(species_by_slug("snorlax").unwrap().weight_dg, 4600);
        // Joltik = 0.6 kg → 6 dg. Confirms sub-kg precision survives.
        assert_eq!(species_by_slug("joltik").unwrap().weight_dg, 6);
    }
}
