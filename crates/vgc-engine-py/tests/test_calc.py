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


def test_calc_format_defaults_to_champions():
    # Trop Kick is 85 BP in Champions (PS data/mods/champions/moves.ts),
    # 70 BP in standard gen 9 (data/moves.ts).
    atk, dfn = "Tsareena / Adamant / 252 Atk", "Garchomp / 252 HP"
    default = vgc_engine.calc(atk, dfn, "tropkick")
    champions = vgc_engine.calc(atk, dfn, "tropkick", format="gen9championsvgc2026regmc")
    doubles = vgc_engine.calc(atk, dfn, "tropkick", format="doubles")
    standard = vgc_engine.calc(atk, dfn, "tropkick", format="gen9vgc2025regh")
    assert default["rolls"] == champions["rolls"] == doubles["rolls"]
    assert standard["max"] < champions["max"]
    assert abs(champions["max"] - standard["max"] * 85 / 70) <= 2


def test_calc_unknown_format_raises():
    try:
        vgc_engine.calc("chomp", "lando", "eq", format="notaformat")
    except ValueError:
        return
    raise AssertionError("expected ValueError for unknown format")


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


def test_calc_skips_switch_in_effects_unless_asked():
    # The defender's Intimidate is a switch-in effect, not part of the calc.
    atk = "Garchomp / Adamant / 252 Atk"
    plain = vgc_engine.calc(atk, "Incineroar / Blaze / 252 HP", "eq")
    intim = vgc_engine.calc(atk, "Incineroar / Intimidate / 252 HP", "eq")
    assert plain["rolls"] == intim["rolls"]
    replay = vgc_engine.calc(
        atk, "Incineroar / Intimidate / 252 HP", "eq", switch_in_effects=True
    )
    assert replay["max"] < intim["max"]


def test_calc_reports_uncapped_damage_into_focus_sash():
    atk = "Garchomp @ Choice Band / Adamant / 252 Atk"
    bare = vgc_engine.calc(atk, "Pikachu", "eq")
    sash = vgc_engine.calc(atk, "Pikachu @ Focus Sash", "eq")
    assert sash["rolls"] == bare["rolls"]
    assert sash["survived_by"] == "focus_sash"
    assert sash["ko"]["kind"] == "none"
    assert bare["survived_by"] is None


def test_calc_doubles_modifiers_match_ps():
    # PS champions sim getDamage rows (bf-gate/ps_calc.js), Doubles.
    atk, dfn = "Garchomp / Adamant / 252 Atk", "Incineroar / Blaze / 252 HP"
    r = vgc_engine.calc(atk, dfn, "eq", doubles=True, reflect=True)
    assert r["rolls"][0] == 137 and r["max"] == 164
    r = vgc_engine.calc(
        atk, dfn, "eq", spread=True, doubles=True, reflect=True,
        helping_hand=True, friend_guard=True,
    )
    assert r["min"] == 115 and r["max"] == 136
    assert vgc_engine.calc(atk, dfn, "eq", doubles=True, power_spot=True)["max"] == 318
    assert vgc_engine.calc(atk, dfn, "eq", doubles=True, aurora_veil=True)["max"] == 164
    assert vgc_engine.calc(atk, dfn, "eq", doubles=True, light_screen=True)["max"] == 246
    gross = vgc_engine.calc(
        "Metagross / Adamant / 252 Atk", "Garchomp / 252 HP", "Iron Head",
        doubles=True, steely_spirit=True,
    )
    assert gross["max"] == 144
    gengar = vgc_engine.calc(
        "Gengar / Modest / 252 SpA", "Garchomp / 252 HP", "Shadow Ball",
        doubles=True, battery=True,
    )
    assert gengar["max"] == 133
    low = vgc_engine.calc(
        "Incineroar / Blaze / Adamant / 252 Atk / 30%", "Garchomp / 252 HP", "Flare Blitz"
    )
    assert low["max"] == 95
