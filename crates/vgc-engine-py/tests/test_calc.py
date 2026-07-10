"""Regression tests for the `vgc_engine.calc(...)` pyo3 binding.

Build/install the extension first (from the mimikyu analytics venv):

    VIRTUAL_ENV=~/Dev/mimikyu/.venv-analytics \
    PATH=~/Dev/mimikyu/.venv-analytics/bin:$PATH \
    maturin develop

then run: `python -m pytest crates/vgc-engine-py/tests/`.

The expected damage arrays are the @smogon/calc oracle values that the
Rust `calc.rs` unit tests already assert against — this test confirms the
Python dict shape carries them through unchanged.
"""

import vgc_engine


def test_calc_matches_smogon_cc_lifeorb():
    # Lucario Life Orb Close Combat into Garchomp (@smogon/calc corpus).
    atk = "Lucario @ Life Orb / Adamant / Inner Focus / 4 HP / 252 Atk / 252 Spe"
    dfn = "Garchomp / Impish / Sand Veil / 252 HP / 252 Def / 4 SpD"
    r = vgc_engine.calc(atk, dfn, "Close Combat")
    assert r["min"] == 99
    assert r["max"] == 117
    assert r["rolls"] == [
        99, 99, 101, 101, 103, 105, 105, 107, 107, 109, 110, 110, 113, 113, 114, 117
    ]
    assert r["defender_max_hp"] > 0
    # Non-OHKO here, so multi_hit reports a 2HKO.
    assert r["multi_hit"]["hits"] == 2
    assert "2HKO" in r["multi_hit"]["label"]
    # Crit companion present on a non-crit, non-zero calc.
    assert r["crit"] is not None
    assert r["crit"]["max"] >= r["max"]


def test_calc_aliases_and_immunity():
    # Garchomp Earthquake vs Landorus-Therian (Flying → Ground immune).
    r = vgc_engine.calc("chomp", "lando", "eq")
    assert r["max"] == 0
    assert r["ko"]["kind"] == "none"
    assert r["multi_hit"]["hits"] == 0
    assert r["multi_hit"]["label"] == "no KO"
    # Zero-damage calc has no crit companion.
    assert r["crit"] is None


def test_calc_spread_modifier():
    single = vgc_engine.calc("chomp", "Iron Hands / 252 HP / 252 SpD", "eq")
    spread = vgc_engine.calc(
        "chomp", "Iron Hands / 252 HP / 252 SpD", "eq", spread=True
    )
    # Spread applies the Doubles x0.75 modifier → strictly less damage.
    assert spread["max"] < single["max"]


def test_calc_unknown_species_raises():
    try:
        vgc_engine.calc("notamon", "chomp", "eq")
    except ValueError:
        return
    raise AssertionError("expected ValueError for unknown species")


if __name__ == "__main__":
    # Runnable without pytest: `python tests/test_calc.py`.
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("ALL CALC BINDING TESTS PASSED")
