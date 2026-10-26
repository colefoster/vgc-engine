"""`solve_safe_ko`: shallow search for a KO that every opponent reply allows
while none of ours faints."""
import time

import vgc_engine

P1 = """[
    {"species":"garchomp","level":100,"ability":"roughskin","item":"lifeorb","nature":"adamant","evs":{"atk":252,"hp":252},"moves":["earthquake","dragonclaw","protect","swordsdance"]},
    {"species":"corviknight","level":100,"ability":"pressure","item":"leftovers","nature":"impish","evs":{"hp":252,"def":252},"moves":["roost","tailwind","protect","irondefense"]}
]"""
P2 = """[
    {"species":"pikachu","level":50,"ability":"static","item":"","nature":"hardy","moves":["thunderbolt","protect","quickattack","growl"]},
    {"species":"pichu","level":50,"ability":"static","item":"","nature":"hardy","moves":["thundershock","charm","sweetkiss","growl"]},
    {"species":"raichu","level":50,"ability":"static","item":"","nature":"hardy","moves":["thunderbolt","protect","quickattack","growl"]}
]"""


def test_spread_ko_through_protect():
    b = vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=7)
    r = vgc_engine.solve_safe_ko(b, side="p1", time_budget_ms=60_000, horizon=1)
    assert r["horizon_completed"] == 1 and not r["truncated"]
    assert r["chosen"] is not None and r["chosen_p"] >= 0.9
    eq = lambda j: any(c[0] in ("move", "tera") and c[1] == 0 and c[2] == 0 for c in j)
    claw_pika = lambda j: any(c[0] in ("move", "tera") and c[1] == 0 and c[2] == 1 and c[3] == 1 and c[4] == 0 for c in j)
    assert eq(r["chosen"])
    for joint, p, passed in r["actions"]:
        if eq(joint):
            assert passed, (joint, p)
        if claw_pika(joint):      # Pikachu's Protect refutes it
            assert not passed, (joint, p)


def test_budget_and_no_result():
    b = vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=7)
    t0 = time.perf_counter()
    r = vgc_engine.solve_safe_ko(b, side="p1", time_budget_ms=60_000, replay_budget=5)
    assert (time.perf_counter() - t0) < 5
    assert r["horizon_completed"] == 0 and r["chosen"] is None and r["truncation_reason"] == "replay_budget"
