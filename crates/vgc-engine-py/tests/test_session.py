"""PS-style sessions through the pyo3 binding: request / choose /
player_stream / observe(side) (docs/protocol.md, phase 2).

Build/install the extension first (`maturin develop`), then run
`python -m pytest crates/vgc-engine-py/tests/`.
"""

import json
import random
import re

import pytest
import vgc_engine

FORMAT = "gen9championsvgc2026regmc"

P1 = json.dumps([
    {"species": "garchomp", "gender": "M", "level": 50, "item": "lifeorb", "ability": "roughskin",
     "moves": ["earthquake", "dragonclaw", "protect", "rockslide"]},
    {"species": "incineroar", "gender": "M", "level": 50, "item": "sitrusberry", "ability": "intimidate",
     "moves": ["fakeout", "flareblitz", "partingshot", "knockoff"]},
    {"species": "rillaboom", "gender": "M", "level": 50, "item": "miracleseed", "ability": "grassysurge",
     "moves": ["woodhammer", "fakeout", "uturn", "grassyglide"]},
    {"species": "gengar", "gender": "M", "level": 50, "item": "focussash", "ability": "cursedbody",
     "moves": ["shadowball", "sludgebomb", "protect", "trickroom"]},
])
P2 = json.dumps([
    {"species": "tyranitar", "gender": "M", "level": 50, "item": "leftovers", "ability": "sandstream",
     "moves": ["rockslide", "crunch", "protect", "lowkick"]},
    {"species": "amoonguss", "gender": "F", "level": 50, "item": "rockyhelmet", "ability": "regenerator",
     "moves": ["spore", "ragepowder", "pollenpuff", "protect"]},
    {"species": "dragapult", "gender": "F", "level": 50, "item": "choicespecs", "ability": "clearbody",
     "moves": ["shadowball", "dracometeor", "uturn", "flamethrower"]},
    {"species": "whimsicott", "gender": "F", "level": 50, "item": "focussash", "ability": "prankster",
     "moves": ["tailwind", "moonblast", "encore", "protect"]},
])


def session(seed=1):
    return vgc_engine.Battle.from_teams(P1, P2, format=FORMAT, seed=seed, ps_session=True)


def random_choice(req, rng):
    """A random choice string answering `req` (not always legal)."""
    if req.get("teamPreview"):
        return "team " + "".join(str(i) for i in rng.sample(range(1, 5), 4))
    if req.get("forceSwitch"):
        return "default"
    parts = []
    for i, a in enumerate(req["active"]):
        mon = req["side"]["pokemon"][i]
        if a is None or mon["condition"].endswith(" fnt") or mon["commanding"]:
            parts.append("pass")
            continue
        if rng.random() < 0.1 and not a.get("trapped"):
            parts.append(f"switch {rng.choice([3, 4])}")
            continue
        k = rng.randrange(len(a["moves"])) + 1
        target = a["moves"][k - 1].get("target")
        s = f"move {k}"
        if target in ("normal", "any", "adjacentFoe"):
            s += f" {rng.choice([1, 2])}"
        elif target == "adjacentAlly":
            s += f" -{2 - i}"
        parts.append(s)
    return ", ".join(parts)


def play(b, rng, on_decision=None, max_rounds=500):
    for _ in range(max_rounds):
        if b.ended:
            return
        moved = False
        for side in ("p1", "p2"):
            r = b.request(side)
            if r is None:
                continue
            if on_decision:
                on_decision(b, side, json.loads(r))
            err = b.choose(side, random_choice(json.loads(r), rng))
            if err is not None:
                assert err.startswith("|error|["), err
                assert b.choose(side, "default") is None
            moved = True
        if not moved:
            return


def test_needs_a_session():
    b = vgc_engine.Battle.from_teams(P1, P2, format=FORMAT)
    with pytest.raises(ValueError):
        b.request("p1")
    s = session()
    with pytest.raises(ValueError):
        s.step_pass()


def test_team_preview_request():
    b = session()
    for side in ("p1", "p2"):
        req = json.loads(b.request(side))
        assert req["teamPreview"] is True
        assert req["maxChosenTeamSize"] == 4
        assert [p["ident"] for p in req["side"]["pokemon"]][:1] == [f"{side}: " + ("Garchomp" if side == "p1" else "Tyranitar")]
        assert "noCancel" not in req  # Cancel Mod, both sides asked
    # Key order is PS's (JSON.stringify of getRequests' object).
    raw = b.request("p1")
    assert raw.startswith('{"teamPreview":true,"maxChosenTeamSize":4,"side":{"name":"Player 1","id":"p1","pokemon":[{"ident":"p1: Garchomp","details":"Garchomp, L50, M","condition":')
    assert b.choose("p1", "team 3412") is None
    assert b.request("p1") is None  # answered, waiting for p2
    assert b.choose("p2", "team 1234") is None
    req = json.loads(b.request("p1"))
    # The picks are the new side order: Rillaboom and Gengar lead.
    assert [p["ident"] for p in req["side"]["pokemon"]] == ["p1: Rillaboom", "p1: Gengar", "p1: Garchomp", "p1: Incineroar"]
    assert [m["id"] for m in req["active"][0]["moves"]] == ["woodhammer", "fakeout", "uturn", "grassyglide"]


# Inputs and PS's replies, from PS `a5df8274` (same teams and format, p1's
# choices sent one by one; p2 answers team preview first).
PREVIEW_INPUTS = ["move 1", "team 12345", "team 1123", "team 9", "pass", "switch 3", "team 1234"]
TURN1_INPUTS = ["move 5", "move 2", "move 2 1, move 2 3", "switch 1", "switch 3, switch 3", "move 1 1", "foo",
                "move 1, pass", "move 1", "move 1 mega", "move 2 1 terastallize, move 1 1",
                "switch Rillaboom, move 1 1", "switch 9", "team 1234", "move 2 -1", "move 2 1 2",
                "move dragonclaw 1, move fakeout 2", "move 3, move nonexistent 1", "move 1, move 1 1, move 1",
                "default", "move 1 zmove", "move 1 dynamax", "shift", "pass 1"]
PS_ERRORS = [
    "|error|[Invalid choice] Can't move: You need a teampreview response",
    "|error|[Invalid choice] Can't choose for Team Preview: You're not in a Team Preview phase",
    "|error|[Invalid choice] Can't choose for Team Preview: You're not in a Team Preview phase",
    "|error|[Invalid choice] Can't pass: Your Garchomp must make a move (or switch)",
    "|error|[Invalid choice] Incomplete choice: switch 3 - missing other pokemon",
    "|error|[Invalid choice] Can't choose for Team Preview: You're not in a Team Preview phase",
    "|error|[Invalid choice] Can't move: Your Garchomp doesn't have a move 5",
    "|error|[Invalid choice] Can't move: Dragon Claw needs a target",
    "|error|[Invalid choice] Can't move: Invalid target for Flare Blitz",
    "|error|[Invalid choice] Can't switch: You can't switch to an active Pokémon",
    "|error|[Invalid choice] Can't switch: The Pokémon in slot 3 can only switch in once",
    "|error|[Invalid choice] Can't move: You can't choose a target for Earthquake",
    "|error|[Invalid choice] Unrecognized choice: foo",
    "|error|[Invalid choice] Can't pass: Your Incineroar must make a move (or switch)",
    "|error|[Invalid choice] Incomplete choice: move 1 - missing other pokemon",
    "|error|[Invalid choice] Can't move: Garchomp can't mega evolve",
    "|error|[Invalid choice] Can't move: Garchomp can't Terastallize.",
    "|error|[Invalid choice] Can't switch: You do not have a Pokémon in slot 9 to switch to",
    "|error|[Invalid choice] Can't choose for Team Preview: You're not in a Team Preview phase",
    "|error|[Invalid choice] Can't move: Invalid target for Dragon Claw",
    "|error|[Invalid choice] Conflicting arguments for \"move\": 2 1 2",
    "|error|[Invalid choice] Can't move: Your Incineroar doesn't have a move matching nonexistent",
    "|error|[Invalid choice] Can't make choices: You sent choices for 3 Pokémon, but this is a doubles game!",
    "|error|[Invalid choice] Can't move: Garchomp can't use Earthquake as a Z-move",
    "|error|[Invalid choice] Can't move: Earthquake needs a target",
    "|error|[Invalid choice] Can't shift: You can only shift to the center in triples",
    "|error|[Invalid choice] Unrecognized data after \"pass\": 1",
]


def test_choose_errors_are_ps_text():
    b = session()
    assert b.choose("p2", "team 1234") is None
    errors = []
    for c in PREVIEW_INPUTS + TURN1_INPUTS:
        e = b.choose("p1", c)
        if e is not None:
            errors.extend(e.split("\n"))
    assert errors == PS_ERRORS
    # The same lines are on p1's stream, and only there.
    assert [l for l in b.player_stream("p1") if l.startswith("|error|")] == PS_ERRORS
    assert not any(l.startswith("|error|") for l in b.player_stream("p2"))


def test_choose_advances_and_accepts_suffixes():
    b = session()
    assert b.choose("p1", "team 1234") is None
    assert b.choose("p2", "team 1234") is None
    assert b.turn == 0 or b.turn == 1
    assert b.choose("p1", "move 3, move fakeout 1") is None
    assert b.choose("p2", "move protect, move 4") is None
    assert '"|turn|2"' not in b.player_stream("p1")
    assert "|turn|2" in b.player_stream("p1")
    assert b.request("p1") is not None


def test_player_stream_views_and_offsets():
    b = session()
    p1 = b.player_stream("p1")
    assert p1[0].startswith("|t:|")
    assert p1[-1].startswith('|request|{"teamPreview":true')
    assert b.player_stream("p1", since=len(p1)) == []
    b.choose("p1", "team 1234")
    b.choose("p2", "team 1234")
    new = b.player_stream("p1", since=len(p1))
    assert "|switch|p1a: Garchomp|Garchomp, L50, M|183/183" in new  # own HP exact
    assert any(l.startswith("|switch|p2a: Tyranitar|Tyranitar, L50, M|100/100") for l in new)  # foe's as a percentage
    assert new[-1].startswith('|request|{"active":[')
    spec = b.player_stream("spectator")
    assert not any(l.startswith("|request|") for l in spec)
    assert "|switch|p1a: Garchomp|Garchomp, L50, M|100/100" in spec
    omni = b.player_stream("omniscient")
    assert "|switch|p2a: Tyranitar|Tyranitar, L50, M|207/207" in omni or any(
        re.match(r"\|switch\|p2a: Tyranitar\|Tyranitar, L50, M\|\d+/\d+$", l) for l in omni)
    # Chunks: the p1 stream ends with an update, then its request.
    chunks = b.player_chunks("p1")
    assert chunks[-1].startswith("|request|") and chunks[-2].endswith("|turn|1")


def test_mid_turn_switch_request():
    # Turn 1: p1's Rillaboom (slot 1) U-turns; PS stops the turn and asks p1
    # alone (p2 gets a wait request).
    b = session()
    b.choose("p1", "team 3412")
    b.choose("p2", "team 1234")
    assert b.choose("p1", "move 3 1, move 3") is None
    assert b.choose("p2", "move 2 2, move 1 2") is None
    req = json.loads(b.request("p1"))
    assert req["forceSwitch"] == [True, False]
    assert req["noCancel"] is True
    assert b.request("p2") is None
    assert b.player_stream("p2")[-1].startswith('|request|{"wait":true,"side":')
    assert any("|move|p1a: Rillaboom|U-turn|" in l for l in b.player_stream("p1"))
    assert b.choose("p1", "switch 1") is not None  # active
    assert b.choose("p1", "switch 3") is None
    lines = b.player_stream("p1")
    assert any(l.startswith("|switch|p1a: Garchomp|Garchomp, L50, M|") and l.endswith("|[from] U-turn") for l in lines)
    assert "|turn|2" in lines


def test_observe_side_shows_only_what_the_player_saw():
    rng = random.Random(5)
    checks = {"n": 0, "hidden": 0}

    def check(b, side, req):
        me = 0 if side == "p1" else 1
        o = b.observe(side)
        stream = b.player_stream(side)
        text = "\n".join(stream)
        truth = b.observe()  # omniscient engine state, for comparison only
        own = o["sides"][me]
        foe = o["sides"][1 - me]
        assert own["is_self"] and not foe["is_self"]
        # Own side: exactly the request's team.
        assert [p["name"] for p in own["pokemon"]] == [p["ident"].split(": ")[1] for p in req["side"]["pokemon"]]
        for p in own["pokemon"]:
            assert p["hp_exact"]
        foe_ident = "p2" if me == 0 else "p1"
        for p in foe["pokemon"]:
            assert not p["hp_exact"]
            if p["max_hp"] is not None and not p["fainted"]:
                assert p["max_hp"] == 100  # PS shows the foe's HP as a percentage
            name = p["name"]
            if name is None:
                assert p["moves"] == [] and p["item"] is None and p["ability"] is None
                continue
            # Every move shown was announced on the stream by that mon.
            for mv in p["moves"]:
                assert re.search(rf"\|(move|cant)\|{foe_ident}[ab]: {re.escape(name)}\|", text), (name, mv)
                announced = [l for l in stream if re.match(rf"\|(move|cant)\|{foe_ident}[ab]: {re.escape(name)}\|", l)]
                assert any(re.sub(r"[^a-z0-9]", "", l.split("|")[3].lower()) == mv or
                           re.sub(r"[^a-z0-9]", "", (l.split("|")[4] if len(l.split("|")) > 4 else "").lower()) == mv
                           for l in announced), (name, mv)
            # A known item / ability was named on a line about that mon.
            for key in ("item", "ability"):
                if p[key]:
                    assert re.search(re.escape(name), text) and any(
                        re.sub(r"[^a-z0-9]", "", l.lower()).find(p[key]) >= 0 for l in stream if name in l
                    ), (name, key, p[key])
        # The engine state knows the foes' whole movesets; the observation
        # holds only the revealed part of them.
        full = {m["species"]: {x["id"] for x in m["moves"]} for m in truth["sides"][1 - me]["active"]}
        for p in foe["pokemon"]:
            moves = set(p["moves"])
            known = full.get(re.sub(r"[^a-z0-9]", "", p["species"].lower()))
            if p["active"] and known is not None:
                assert moves <= known
                if len(moves) < len(known):
                    checks["hidden"] += 1
        checks["n"] += 1

    for seed in (1, 2, 3):
        b = session(seed)
        play(b, rng, on_decision=check)
        assert b.ended
    assert checks["n"] > 20
    assert checks["hidden"] > 0


def test_observe_side_hides_unrevealed_items_and_abilities():
    b = session()
    o = b.observe("p1")
    foe = o["sides"][1]
    # Team preview: species only.
    assert [p["species"] for p in foe["pokemon"]] == ["Tyranitar", "Amoonguss", "Dragapult", "Whimsicott"]
    assert all(p["item"] is None and p["ability"] is None and p["moves"] == [] for p in foe["pokemon"])
    b.choose("p1", "team 1234")
    b.choose("p2", "team 1234")
    o = b.observe("p1")
    ttar = next(p for p in o["sides"][1]["pokemon"] if p["species"] == "Tyranitar")
    # Sand Stream announced itself on switch-in; Leftovers hasn't shown yet.
    assert ttar["ability"] == "sandstream"
    assert ttar["item"] is None
    assert ttar["hp"] == 100 and ttar["max_hp"] == 100
    own = o["sides"][0]["pokemon"][0]
    assert own["item"] == "lifeorb" and own["moves"] == ["earthquake", "dragonclaw", "protect", "rockslide"]
    assert own["hp"] == own["max_hp"] == 183
    assert o["weather"] == "Sandstorm"


BEAM = json.dumps([
    {"species": "snorlax", "gender": "M", "level": 50, "item": "leftovers", "ability": "thickfat",
     "moves": ["hyperbeam", "bodyslam", "protect", "crunch"]},
    {"species": "garchomp", "gender": "M", "level": 50, "item": "lifeorb", "ability": "roughskin",
     "moves": ["protect", "dragonclaw", "earthquake", "rockslide"]},
    {"species": "incineroar", "gender": "M", "level": 50, "item": "sitrusberry", "ability": "intimidate",
     "moves": ["fakeout", "flareblitz", "partingshot", "knockoff"]},
    {"species": "gengar", "gender": "M", "level": 50, "item": "focussash", "ability": "cursedbody",
     "moves": ["shadowball", "sludgebomb", "protect", "trickroom"]},
])
# Turn 1: Snorlax Hyper Beams p2a, Garchomp Protects; Tyranitar Rock Slide, Amoonguss Rage Powder.
BEAM_T1 = (("p1", "team 1234"), ("p2", "team 1234"), ("p1", "move 1 1, move 1"), ("p2", "move 1, move 2"))
P2_T2 = "move 1, move 2"


def recharge_seed():
    """A seed whose turn 2 has p1's Snorlax recharging."""
    for seed in range(1, 40):
        b = vgc_engine.Battle.from_teams(BEAM, P2, format=FORMAT, seed=seed, ps_session=True)
        for side, c in BEAM_T1:
            assert b.choose(side, c) is None
        if json.loads(b.request("p1"))["active"][0]["moves"][0]["id"] == "recharge":
            return seed
    raise AssertionError("Hyper Beam never connected")


def recharge_session(seed):
    b = vgc_engine.Battle.from_teams(BEAM, P2, format=FORMAT, seed=seed, ps_session=True)
    for side, c in BEAM_T1:
        assert b.choose(side, c) is None
    return b


def test_recharge_request_and_choices():
    seed = recharge_seed()
    b = recharge_session(seed)
    a = json.loads(b.request("p1"))["active"][0]
    # PS getMoveRequestData for a getLockedMove mon: one short-form move, trapped.
    assert a["moves"] == [{"move": "Recharge", "id": "recharge"}]
    assert a["trapped"] is True
    assert b.legal_choices(0, 0) == [("recharge", 0, 0, -1, -1)]
    assert len(b.legal_choices(0, 1)) > 1
    assert b.choose("p1", "move 1, move 2 1") is None
    assert b.choose("p2", P2_T2) is None
    log = "\n".join(b.player_stream("p1"))
    assert "|cant|p1a: Snorlax|recharge" in log

    b2 = recharge_session(seed)
    assert b2.choose("p1", "default") is None
    assert b2.choose("p2", P2_T2) is None
    assert "|cant|p1a: Snorlax|recharge" in "\n".join(b2.player_stream("p1"))


def test_recharge_legal_choice_round_trips_through_step_move():
    seed = recharge_seed()
    b = vgc_engine.Battle.from_teams(BEAM, P2, format=FORMAT, seed=seed)
    p2 = [("move", 0, 0, -1, -1), ("move", 1, 1, -1, -1)]
    for _ in range(10):
        if b.legal_choices(0, 0)[0][0] == "recharge":
            break
        b.step_move([("move", 0, 0, 1, 0), ("move", 1, 0, -1, -1)], p2)
    assert b.legal_choices(0, 0) == [("recharge", 0, 0, -1, -1)]
    b.step_move([("recharge", 0, 0, -1, -1), ("move", 1, 1, 1, 0)], p2)
    assert b.legal_choices(0, 0)[0][0] != "recharge"
