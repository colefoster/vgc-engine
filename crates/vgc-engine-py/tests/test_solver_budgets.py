"""Hard budget guarantees for `solve_endgame_exact` / `solve_endgame_anytime`.

Regression for the 2026-10-20 audit: `solve_endgame_exact(max_depth=2,
node_budget=10000)` ran 60 s+ on a real 2v2 (the node budget never bounded
chance enumeration). Both entry points now honour `time_budget_ms` inside
enumeration and return a well-formed result with `truncated` set.
"""
import threading
import time

import vgc_engine

P1 = """[
    {"species":"maushold","level":50,"ability":"technician","item":"widelens","nature":"jolly","moves":["populationbomb","followme","protect","bite"]},
    {"species":"typhlosion","level":50,"ability":"blaze","item":"choicescarf","nature":"timid","moves":["heatwave","eruption","scorchingsands","shadowball"]}
]"""
P2 = """[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","rockslide","dragonclaw","protect"]},
    {"species":"incineroar","level":50,"ability":"intimidate","item":"sitrusberry","nature":"careful","moves":["fakeout","flareblitz","knockoff","partingshot"]}
]"""

SLACK_S = 0.15


def _battle():
    return vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=3)


def _assert_well_formed(b, r):
    assert isinstance(r["value"], float) and -1.0 <= r["value"] <= 1.0
    for key, side in (("row_joint_policy", 0), ("col_joint_policy", 1)):
        pol = r[key]
        assert pol, key
        assert abs(sum(p for _, p in pol) - 1.0) < 1e-6
        legal = [set(map(tuple, b.legal_choices(side, slot))) for slot in range(2)]
        for joint, _ in pol:
            # Entries past the active slots are mid-turn picks (pivot switches).
            for slot, c in enumerate(joint[:2]):
                assert tuple(c) in legal[slot], (key, c)


def test_exact_depth2_node_budget_respects_time_budget():
    b = _battle()
    t0 = time.perf_counter()
    r = vgc_engine.solve_endgame_exact(b, max_depth=2, node_budget=10_000, time_budget_ms=300)
    el = time.perf_counter() - t0
    assert el < 0.3 + SLACK_S, el
    assert r["truncated"] and r["truncation_reason"] == "deadline"
    _assert_well_formed(b, r)


def test_anytime_respects_time_budget_and_never_returns_none():
    b = _battle()
    for budget_ms in (1, 50, 300):
        t0 = time.perf_counter()
        r = vgc_engine.solve_endgame_anytime(b, time_budget_ms=budget_ms)
        el = time.perf_counter() - t0
        assert el < budget_ms / 1000 + SLACK_S, (budget_ms, el)
        assert r["truncated"] and r["truncation_reason"] == "deadline"
        _assert_well_formed(b, r)


def test_replay_budget_is_deterministic():
    b = _battle()
    a = vgc_engine.solve_endgame_exact(b, max_depth=2, replay_budget=2000, time_budget_ms=None)
    c = vgc_engine.solve_endgame_exact(b, max_depth=2, replay_budget=2000, time_budget_ms=None)
    assert a["truncation_reason"] == "replay_budget"
    assert a["value"] == c["value"]


def test_exact_releases_the_gil():
    b = _battle()
    ticks = []
    stop = threading.Event()

    def spin():
        while not stop.is_set():
            ticks.append(1)
            time.sleep(0.005)

    t = threading.Thread(target=spin)
    t.start()
    vgc_engine.solve_endgame_exact(b, max_depth=2, time_budget_ms=300)
    stop.set()
    t.join()
    assert len(ticks) > 20, len(ticks)


def test_single_ply_solve_endgame_time_budget():
    b = _battle()
    t0 = time.perf_counter()
    r = vgc_engine.solve_endgame(b, lambda obs: [0.0] * len(obs), time_budget_ms=50)
    assert time.perf_counter() - t0 < 0.05 + SLACK_S
    assert r["truncated"] and r["truncation_reason"] == "deadline" and r["value"] is None
