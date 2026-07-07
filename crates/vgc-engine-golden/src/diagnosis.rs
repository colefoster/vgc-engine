//! Delta-cluster classifier for calc-oracle scenario failures.
//!
//! Given the engine's observed damage rolls and the calc's expected union,
//! computes per-observation deltas to the nearest calc value and labels
//! the cluster: rounding off-by-1, unaccounted EOT tick, residual-class
//! rounding, formula gap, or mixed. Diagnostic hint only — not
//! authoritative.

/// For each observed value, compute `observed - nearest_expected`, then
/// classify the cluster. Returns `(label, deltas)`.
///
/// - `observed`: engine's raw 16-roll damage array (values in HP units).
/// - `expected_union`: calc's expected union (non-crit ∪ crit).
/// - `target_max_hp`: defender max HP, used to size the ±HP/N buckets.
pub fn classify_deltas(
    observed: &[u16],
    expected_union: &[u32],
    target_max_hp: u16,
) -> (String, Vec<i32>) {
    if observed.is_empty() {
        return ("no observations".to_string(), vec![]);
    }
    if expected_union.is_empty() {
        return ("no calc expectation".to_string(), vec![]);
    }
    let deltas: Vec<i32> = observed
        .iter()
        .map(|&v| nearest_delta(v as i32, expected_union))
        .collect();

    if deltas.iter().all(|d| *d == 0) {
        return ("clean pass (all rolls in calc union)".to_string(), deltas);
    }

    // Formula gap: any observation >2x max calc, or <0.5x min calc (and nonzero).
    let calc_min = *expected_union.iter().min().unwrap_or(&0) as i32;
    let calc_max = *expected_union.iter().max().unwrap_or(&0) as i32;
    if calc_max > 0 {
        for &v in observed {
            let v = v as i32;
            if v > calc_max * 2 || (v > 0 && v * 2 < calc_min) {
                return (
                    format!(
                        "formula gap — obs {} outside 0.5x..2x calc [{}..{}] (missing type/STAB/ability?)",
                        v, calc_min, calc_max
                    ),
                    deltas,
                );
            }
        }
    }

    let abs: Vec<i32> = deltas.iter().map(|d| d.abs()).collect();
    let max_abs = *abs.iter().max().unwrap_or(&0);

    // Rounding step off-by-1: every delta is within ±1.
    if max_abs <= 1 {
        return (
            "rounding step off-by-1 (chainModify bias?)".to_string(),
            deltas,
        );
    }

    let mhp = target_max_hp as i32;
    if mhp > 0 {
        let tick16 = mhp / 16;
        let tick8 = mhp / 8;
        let tick32 = (mhp / 32).max(1);

        // EOT tick unaccounted: majority of deltas near ±mhp/16 (±1).
        if tick16 > 0 {
            let hits = abs.iter().filter(|a| (**a - tick16).abs() <= 1).count();
            if hits * 2 >= observed.len() {
                return (
                    format!(
                        "EOT tick unaccounted (~mhp/16 = {}; sand chip / weather / heal?)",
                        tick16
                    ),
                    deltas,
                );
            }
        }

        // Residual damage class: cluster near ±mhp/8.
        if tick8 > 0 {
            let hits = abs.iter().filter(|a| (**a - tick8).abs() <= 1).count();
            if hits * 2 >= observed.len() {
                return (
                    format!("residual damage class (~mhp/8 = {})", tick8),
                    deltas,
                );
            }
        }

        // Small residual class: cluster near ±mhp/32.
        if tick32 > 0 {
            let hits = abs.iter().filter(|a| (**a - tick32).abs() <= 1).count();
            if hits * 2 >= observed.len() {
                return (
                    format!("small residual damage class (~mhp/32 = {})", tick32),
                    deltas,
                );
            }
        }
    }

    (
        "mixed — probably compound rounding across multiple steps".to_string(),
        deltas,
    )
}

fn nearest_delta(v: i32, expected: &[u32]) -> i32 {
    let mut best = i32::MAX;
    let mut best_abs = i32::MAX;
    for &e in expected {
        let d = v - e as i32;
        let a = d.abs();
        if a < best_abs {
            best_abs = a;
            best = d;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_pass_all_in_union() {
        let obs = vec![100u16, 101, 102, 103];
        let expected = vec![100u32, 101, 102, 103, 104];
        let (label, deltas) = classify_deltas(&obs, &expected, 222);
        assert!(label.starts_with("clean pass"), "got: {label}");
        assert!(deltas.iter().all(|d| *d == 0));
    }

    #[test]
    fn rounding_off_by_one() {
        let obs = vec![100u16, 102, 104, 106];
        let expected = vec![101u32, 103, 105, 107];
        let (label, _deltas) = classify_deltas(&obs, &expected, 222);
        assert!(label.contains("rounding step off-by-1"), "got: {label}");
    }

    #[test]
    fn sand_tick_unaccounted() {
        // Deltas cluster around mhp/16 = 12 (mhp=192).
        let mhp = 192u16;
        let tick = (mhp / 16) as i32; // 12
        // Widely-spaced calc values (spacing > 2*tick to prevent aliasing to a
        // neighbouring bucket).
        let expected: Vec<u32> = vec![100, 140, 180, 220, 260, 300, 340, 380];
        // Each obs sits exactly +tick above its calc value.
        let obs: Vec<u16> = expected.iter().map(|&v| (v as i32 + tick) as u16).collect();
        let (label, _) = classify_deltas(&obs, &expected, mhp);
        assert!(label.contains("EOT tick"), "got: {label}");
    }

    #[test]
    fn formula_gap_flagged() {
        let obs = vec![300u16, 310, 320];
        let expected = vec![100u32, 105, 110];
        let (label, _) = classify_deltas(&obs, &expected, 222);
        assert!(label.contains("formula gap"), "got: {label}");
    }

    #[test]
    fn mixed_fallback() {
        let obs = vec![100u16, 105, 130, 90];
        let expected = vec![101u32, 108, 115];
        let (label, _) = classify_deltas(&obs, &expected, 222);
        assert!(label.contains("mixed"), "got: {label}");
    }
}
