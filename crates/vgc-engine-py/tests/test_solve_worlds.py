"""`solve_endgame_worlds`: anytime solve over sampled opponent worlds, own
action chosen by expected value across them."""
import time

import pytest

import vgc_engine

P1 = """[
    {"species":"maushold","level":50,"ability":"technician","item":"widelens","nature":"jolly","moves":["populationbomb","followme","protect","bite"]},
    {"species":"typhlosion","level":50,"ability":"blaze","item":"choicescarf","nature":"timid","moves":["heatwave","eruption","scorchingsands","shadowball"]}
]"""
P2 = """[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","rockslide","dragonclaw","protect"]},
    {"species":"incineroar","level":50,"ability":"intimidate","item":"sitrusberry","nature":"careful","moves":["fakeout","flareblitz","knockoff","partingshot"]}
]"""
CHOMP = """[{"species":"garchomp","level":50,"ability":"roughskin","item":"","nature":"adamant",
             "evs":{"atk":252,"spe":252,"hp":4},"moves":["earthquake","fireblast"]}]"""


def bronzong(ability):
    p2 = ('[{"species":"bronzong","level":50,"ability":"%s","item":"","nature":"relaxed",'
          '"evs":{"hp":252,"def":252},"moves":["gyroball","trickroom"]}]' % ability)
    return vgc_engine.Battle.from_teams(CHOMP, p2, format="singles", seed=5)


@pytest.mark.parametrize("side", ["p1", "p2"])
def test_one_world_is_the_anytime_solve(side):
    b = vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=3)
    kw = dict(time_budget_ms=60_000, replay_budget=50_000)
    single = vgc_engine.solve_endgame_anytime(b, **kw)
    r = vgc_engine.solve_endgame_worlds([b], side=side, **kw)
    w = r["worlds"][0]
    assert r["k"] == 1 and r["worlds_used"] == (1 if single["depth_completed"] else 0)
    for k in ("value", "row_joint_policy", "col_joint_policy", "depth_completed", "truncated", "truncation_reason"):
        assert w[k] == single[k], k
    if single["depth_completed"]:
        pol = single["row_joint_policy" if side == "p1" else "col_joint_policy"]
        assert r["chosen"] == max(pol, key=lambda jp: jp[1])[0]


def test_hidden_ability_flips_the_pick():
    heat, lev = bronzong("heatproof"), bronzong("levitate")
    kw = dict(max_depth=1, time_budget_ms=60_000, replay_budget=1_000_000)
    slot = lambda r: r["chosen"][0][2]  # (kind, actor_slot, move_slot, ...)
    assert slot(vgc_engine.solve_endgame_worlds([heat], **kw)) == 0          # Earthquake
    assert slot(vgc_engine.solve_endgame_worlds([lev], **kw)) == 1           # Fire Blast
    assert slot(vgc_engine.solve_endgame_worlds([heat, lev], weights=[0.2, 0.8], **kw)) == 1
    r = vgc_engine.solve_endgame_worlds([heat, lev], weights=[0.9, 0.1], **kw)
    assert slot(r) == 0 and r["robust"] is not None and len(r["worlds"]) == 2


def test_total_budget_is_shared():
    b = vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=3)
    t0 = time.perf_counter()
    r = vgc_engine.solve_endgame_worlds([b] * 4, time_budget_ms=400)
    assert time.perf_counter() - t0 < 0.4 + 0.15
    assert r["truncated"] and len(r["worlds"]) == 4
    assert all(w["budget_ms"] <= 400 for w in r["worlds"])


def test_bad_input():
    b = bronzong("levitate")
    with pytest.raises(ValueError):
        vgc_engine.solve_endgame_worlds([])
    with pytest.raises(ValueError):
        vgc_engine.solve_endgame_worlds([b, b], weights=[1.0])
    with pytest.raises(ValueError):
        vgc_engine.solve_endgame_worlds([b], side="p3")
