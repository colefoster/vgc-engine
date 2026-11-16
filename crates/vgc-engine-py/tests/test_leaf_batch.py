"""`solve_endgame_anytime` with a Python leaf and per-node menus: the leaf is
scored from the player's stream and request, one call per matrix cell."""
import json

import pytest
import vgc_engine

FORMAT = "gen9championsvgc2026regmc"

P1 = json.dumps([
    {"species": "garchomp", "gender": "M", "level": 50, "item": "lifeorb", "ability": "roughskin",
     "moves": ["dragonclaw", "protect"]},
    {"species": "incineroar", "gender": "M", "level": 50, "item": "sitrusberry", "ability": "intimidate",
     "moves": ["flareblitz", "knockoff"]},
    {"species": "rillaboom", "gender": "M", "level": 50, "item": "miracleseed", "ability": "grassysurge",
     "moves": ["woodhammer", "grassyglide"]},
    {"species": "gengar", "gender": "M", "level": 50, "item": "focussash", "ability": "cursedbody",
     "moves": ["shadowball", "protect"]},
])
P2 = json.dumps([
    {"species": "tyranitar", "gender": "M", "level": 50, "item": "leftovers", "ability": "sandstream",
     "moves": ["crunch", "protect"]},
    {"species": "amoonguss", "gender": "F", "level": 50, "item": "rockyhelmet", "ability": "regenerator",
     "moves": ["pollenpuff", "protect"]},
    {"species": "dragapult", "gender": "F", "level": 50, "item": "choicespecs", "ability": "clearbody",
     "moves": ["shadowball", "dracometeor"]},
    {"species": "whimsicott", "gender": "F", "level": 50, "item": "focussash", "ability": "prankster",
     "moves": ["moonblast", "protect"]},
])


def session():
    b = vgc_engine.Battle.from_teams(P1, P2, format=FORMAT, seed=3, ps_session=True)
    for side in ("p1", "p2"):
        if json.loads(b.request(side)).get("teamPreview"):
            assert b.choose(side, "team 1234") is None
    assert json.loads(b.request("p1")).get("active")
    return b


def root_lines(b, side):
    return [l for l in b.player_stream(side) if not l.startswith(("|request|", "|error|"))]


def top2(_l1, _r1, _l2, _r2, rows, cols):
    return list(range(min(2, len(rows)))), list(range(min(2, len(cols))))


def solve(b, **kw):
    kw.setdefault("node_menu", top2)
    return vgc_engine.solve_endgame_anytime(b, time_budget_ms=120_000, max_depth=1, **kw)


def test_leaf_sees_the_stream_delta_and_request():
    b = session()
    root = root_lines(b, "p1")
    seen = []

    def leaf(items):
        seen.extend(items)
        return [0.25] * len(items)

    r = solve(b, leaf=leaf, leaf_side="p1")
    assert r["depth_completed"] == 1
    assert r["leaf_batches"] >= 1 and r["leaf_states"] == len(seen)
    assert r["leaf_singles"] == 0
    assert r["leaf_batches"] <= r["cells"]
    # Every non-terminal leaf scored 0.25, so the value is 0.25 unless a line
    # ends the game (none does at depth 1 here).
    assert r["value"] == pytest.approx(0.25)
    for lines, req in seen:
        assert all(isinstance(l, str) and not l.startswith("|request|") for l in lines)
        assert any(l.startswith("|move|") or l.startswith("|switch|") or l.startswith("|cant|") for l in lines)
        req = json.loads(req)
        assert req["side"]["id"] == "p1"
        # The delta continues the root stream: a full replayable log.
        assert (root + lines)[0] == root[0]


def test_leaf_side_p2_gets_p2s_view():
    b = session()
    sides = set()

    def leaf(items):
        sides.update(json.loads(req)["side"]["id"] for _, req in items)
        return [0.0] * len(items)

    solve(b, leaf=leaf, leaf_side="p2")
    assert sides == {"p2"}


def test_constant_leaf_values_are_p1_perspective():
    b = session()
    lo = solve(b, leaf=lambda items: [-0.5] * len(items))
    hi = solve(b, leaf=lambda items: [0.5] * len(items))
    assert lo["value"] == pytest.approx(-0.5) and hi["value"] == pytest.approx(0.5)


def test_node_menu_restricts_the_root_and_reports_values():
    b = session()
    calls = []

    def menu(l1, r1, l2, r2, rows, cols):
        calls.append((l1, json.loads(r1)["side"]["id"], json.loads(r2)["side"]["id"], len(rows), len(cols)))
        return [1, 0], [0]

    r = solve(b, node_menu=menu, leaf=lambda items: [0.0] * len(items))
    assert calls and calls[0][0] == [] and calls[0][1:3] == ("p1", "p2")   # the root: no delta yet
    assert calls[0][3] > 2 and calls[0][4] > 1
    assert r["node_menu_calls"] == len(calls) and r["node_menu_fallbacks"] == 0
    assert len(r["row_values"]) == 2 and len(r["col_values"]) == 1
    rows = [j for j, _ in r["row_values"]]
    assert all(j in rows for j, _ in r["row_joint_policy"])


def test_node_menu_without_leaf_uses_hp_ratio():
    b = session()
    r = solve(b)
    assert r["depth_completed"] == 1 and r["leaf_states"] > 0 and r["node_menu_calls"] >= 1


def test_depth_two_gives_interior_nodes_menus():
    b = session()
    deltas = []

    def menu(l1, r1, l2, r2, rows, cols):
        deltas.append(len(l1))
        return [0], [0]

    r = vgc_engine.solve_endgame_anytime(b, time_budget_ms=120_000, max_depth=2, node_menu=menu,
                                         leaf=lambda items: [0.0] * len(items), leaf_after_replacements=True)
    assert r["depth_completed"] == 2
    assert deltas[0] == 0 and any(d > 0 for d in deltas[1:])


def test_callback_error_is_raised_after_the_solve():
    b = session()

    def leaf(items):
        raise RuntimeError("boom")

    with pytest.raises(RuntimeError, match="boom"):
        solve(b, leaf=leaf)
    with pytest.raises(ValueError, match="values for"):
        solve(b, leaf=lambda items: [])


def test_needs_a_session_battle():
    plain = vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=9)
    with pytest.raises(ValueError, match="ps_session"):
        vgc_engine.solve_endgame_anytime(plain, max_depth=1, leaf=lambda items: [0.0] * len(items))


def test_defaults_are_unchanged():
    plain = vgc_engine.Battle.from_teams(P1, P2, format="doubles", seed=9)
    r = vgc_engine.solve_endgame_anytime(plain, time_budget_ms=60_000, max_depth=1, replay_budget=50_000)
    for k in ("leaf_batches", "node_menu_calls", "row_values"):
        assert k not in r
