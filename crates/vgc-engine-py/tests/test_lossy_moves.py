"""Opt-in lossy modes of `solve_endgame_anytime`: outcome classes, sampled
chance and root menus (docs/perf/lossy-moves-2026-11-12.md)."""
import vgc_engine

P1 = """[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["rockslide","dragonclaw"]},
    {"species":"incineroar","level":50,"ability":"blaze","nature":"adamant","moves":["flareblitz","fakeout"]}
]"""
P2 = """[
    {"species":"charizard","level":50,"ability":"blaze","nature":"timid","moves":["heatwave","protect"]},
    {"species":"rillaboom","level":50,"ability":"grassysurge","nature":"adamant","moves":["woodhammer","fakeout"]}
]"""


def _battle():
    return vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=9)


def _solve(**kw):
    # A replay budget makes the result deterministic; the deadline is far.
    return vgc_engine.solve_endgame_anytime(_battle(), time_budget_ms=60_000, max_depth=1, replay_budget=300_000, **kw)


def test_defaults_carry_no_new_keys():
    r = _solve()
    for k in ("class_cells", "sampled_cells", "menu_rows"):
        assert k not in r


def test_outcome_classes_reports_classes_and_fallbacks():
    r = _solve(outcome_classes=True)
    assert r["depth_completed"] >= 1
    assert r["class_cells"] + r["class_fallbacks"] >= 0
    assert set(r["class_fallback_reasons"]) == {"unclassified", "focus", "roll_coupled_hp", "damage_reading", "lazy_unclassified"}


def test_chance_samples_costs_at_most_n_per_cell():
    r = _solve(chance_samples=8)
    assert r["sampled_cells"] > 0
    assert r["samples"] <= 8 * r["sampled_cells"]


def test_root_menu_round_trips_policy_entries():
    free = _solve()
    rows = [j for j, _ in free["row_joint_policy"]]
    cols = [j for j, _ in free["col_joint_policy"]]
    # A menu holding the unrestricted support keeps the root support legal.
    r = _solve(row_menu=rows, col_menu=cols, row_support=rows[:1], col_support=cols[:1])
    assert r["menu_rows"] == len(rows) and r["menu_cols"] == len(cols)
    assert all(j in rows for j, _ in r["row_joint_policy"])
    assert all(j in cols for j, _ in r["col_joint_policy"])
    one = _solve(row_menu=rows[:1])
    assert one["menu_rows"] == 1
    assert [j for j, _ in one["row_joint_policy"]] == rows[:1]


def test_illegal_menu_is_ignored():
    r = _solve(row_menu=[[("pass", 0, 0, -1, -1), ("pass", 1, 0, -1, -1)]])
    assert r["menu_fallbacks"] == 1
