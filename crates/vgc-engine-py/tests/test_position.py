"""Mid-game positions: Battle.from_position and position_from_board
(docs/protocol.md, "Start from a mid-game position").

Build/install the extension first (`maturin develop --features ps-rng`), then
run `python -m pytest crates/vgc-engine-py/tests/`.
"""

import json
import random
import re

import pytest
import vgc_engine

from test_session import FORMAT, P1, P2, random_choice

SEED = "sodium,0123456789abcdef0123456789abcdef"


def session(seed=SEED):
    return vgc_engine.Battle.from_teams(P1, P2, format=FORMAT, ps_seed=seed, ps_session=True,
                                        player_names=("Alice", "Bob"))


def norm(chunks):
    """|t:| carries wall-clock seconds."""
    return [re.sub(r"\|t:\|\d+", "|t:|", c) for c in chunks]


def without_side(lines):
    return [l for l in lines if not l.startswith(("|request|", "|error|"))]


def drive(b, rng, rounds=6, record=None):
    """Answer every open request `rounds` times with random choices."""
    for _ in range(rounds):
        if b.ended:
            return
        for side in ("p1", "p2"):
            r = b.request(side)
            if r is None:
                continue
            c = random_choice(json.loads(r), rng)
            err = b.choose(side, c)
            if record is not None:
                record.append((side, c))
            if err is not None:
                assert b.choose(side, "default") is None
                if record is not None:
                    record.append((side, "default"))


def replay(b, calls):
    for side, c in calls:
        b.choose(side, c)


def snapshot_at_turn(seed, turn, rng_seed=3):
    """Play to the move request of `turn`: (battle, json, per-side prefix,
    omniscient log)."""
    b = session(seed)
    rng = random.Random(rng_seed)
    assert b.choose("p1", "team 1234") is None and b.choose("p2", "team 1234") is None
    # One answer at a time, so the stop it ends at is fresh (nobody answered).
    while b.turn < turn - 1 and not b.ended:
        side = next(s for s in ("p1", "p2") if b.request(s) is not None)
        if b.choose(side, random_choice(json.loads(b.request(side)), rng)) is not None:
            assert b.choose(side, "default") is None
    prefix = {s: without_side(b.player_stream(s)) for s in ("p1", "p2")}
    return b, b.to_json(), prefix, b.protocol_log("omniscient")


def test_resume_continues_byte_identically():
    resumed = 0
    for seed_k in range(8):
        seed = f"sodium,{seed_k:032x}"
        orig, snap, prefix, omni = snapshot_at_turn(seed, 3, rng_seed=seed_k)
        if orig.ended or orig.request("p1") is None:
            continue
        n = {s: len(orig.player_chunks(s)) for s in ("p1", "p2", "spectator", "omniscient")}
        req = {s: orig.request(s) for s in ("p1", "p2")}
        calls = []
        drive(orig, random.Random(100 + seed_k), rounds=8, record=calls)
        # Player streams, or one raw list (here the omniscient view: every
        # player then sees it, so only the continuation is compared).
        for p in (prefix, omni):
            r = vgc_engine.Battle.from_position(snap, prefix=p)
            assert {s: r.request(s) for s in ("p1", "p2")} == req
            start = {s: len(r.player_chunks(s)) for s in n}
            replay(r, calls)
            for s in n:
                assert norm(r.player_chunks(s)[start[s]:]) == norm(orig.player_chunks(s)[n[s]:]), s
            assert r.to_json() == orig.to_json()
            resumed += 1
    assert resumed >= 8


def test_prefix_heads_every_stream_verbatim():
    orig, snap, prefix, _ = snapshot_at_turn(SEED, 2)
    r = vgc_engine.Battle.from_position(snap, prefix=prefix)
    for s in ("p1", "p2"):
        lines = r.player_stream(s)
        assert norm(lines[:len(prefix[s])]) == norm(prefix[s])
        assert lines[len(prefix[s])].startswith("|request|")
        assert r.request(s) == orig.request(s)
    # The merged prefix gives spectators the public halves.
    spec = r.player_stream("spectator")
    assert norm(spec) == norm(without_side(orig.player_stream("spectator")))
    assert r.observe("p1")["turn"] == orig.observe("p1")["turn"]


def test_no_prefix_writes_a_header_and_carries_the_turn():
    _, snap, _, _ = snapshot_at_turn(SEED, 3)
    r = vgc_engine.Battle.from_position(snap)
    lines = r.player_stream("p1")
    replace = any(json.loads(r.request(s) or "{}").get("forceSwitch") for s in ("p1", "p2"))
    assert "|start" in lines
    if not replace:
        assert lines.index("|start") < lines.index(f"|turn|{r.turn + 1}")
    assert any(l.startswith("|switch|p2a: ") for l in lines)
    assert any(json.loads(r.request(s))["side"]["id"] == s for s in ("p1", "p2") if r.request(s))
    rng = random.Random(1)
    drive(r, rng, rounds=1)
    assert f"|turn|{r.turn + 1}" in r.player_stream("p1") or r.ended


def test_ps_seed_reseeds_deterministically():
    _, snap, prefix, _ = snapshot_at_turn(SEED, 2)
    outs = []
    for seed in ("1,2,3,4", "1,2,3,4", "5,6,7,8"):
        r = vgc_engine.Battle.from_position(snap, ps_seed=seed, prefix=prefix)
        drive(r, random.Random(9), rounds=4)
        outs.append(norm(r.player_stream("omniscient")))
    assert outs[0] == outs[1]


def _intimidate_board():
    return {
        "own_active": [{"species": "Incineroar", "moves": ["Fake Out", "Flare Blitz"], "ability": "Intimidate", "hp": 1.0},
                       {"species": "Amoonguss", "moves": ["Spore", "Rage Powder"], "ability": "Regenerator", "hp": 0.5}],
        "opp_active": [{"species": "Garchomp", "moves": ["Earthquake", "Protect"], "ability": "Rough Skin"},
                       {"species": "Whimsicott", "moves": ["Tailwind", "Moonblast"], "ability": "Prankster"}],
        "field": {"turn": 4},
    }


def test_switch_in_effects_only_on_request():
    snap = vgc_engine.position_from_board(_intimidate_board())
    quiet = vgc_engine.Battle.from_position(snap)
    assert not any("Intimidate" in l for l in quiet.player_stream("p2"))
    loud = vgc_engine.Battle.from_position(snap, switch_in_effects=True)
    assert any(l.startswith("|-unboost|p2a: Garchomp|atk|1") for l in loud.player_stream("p2"))
    assert json.loads(loud.to_json())["p2"]["team"][0]["boosts"][0] == -1


def test_reveals_reach_observe_not_requests():
    snap = vgc_engine.position_from_board(_intimidate_board())
    plain = vgc_engine.Battle.from_position(snap)
    r = vgc_engine.Battle.from_position(snap, reveals={"p1": {"0": {"moves": ["Earthquake"], "item": "Life Orb", "tera": "Steel"}},
                                                       "p2": {1: {"ability": "Regenerator"}}})
    assert r.request("p1") == plain.request("p1") and r.request("p2") == plain.request("p2")
    foe = {m["name"]: m for m in r.observe("p1")["sides"][1]["pokemon"]}
    assert foe["Garchomp"]["moves"] == ["earthquake"]
    assert foe["Garchomp"]["item"] == "lifeorb" and foe["Garchomp"]["tera_type"] == "Steel"
    assert foe["Garchomp"]["ability"] is None
    assert foe["Whimsicott"]["moves"] == []
    me = {m["name"]: m for m in r.observe("p2")["sides"][0]["pokemon"]}
    assert me["Amoonguss"]["ability"] == "regenerator"
    with pytest.raises(ValueError):
        vgc_engine.Battle.from_position(snap, reveals={"p1": {"7": {"moves": []}}})


def _seen(stream, side):
    """What `side`'s stream showed about the opponent: move / item / ability ids."""
    foe = "p2" if side == "p1" else "p1"
    text = "\n".join(stream)
    ids = set()
    for l in stream:
        if foe in l:
            for part in l.split("|"):
                ids.add(re.sub(r"[^a-z0-9]", "", part.split(": ")[-1].lower()))
    return ids, text


def test_observe_never_leaks_hidden_opponent_info():
    """Gate 3: for positions restored with and without a prefix, every
    opponent move / item / ability / Tera in observe(side) was shown by the
    side's stream, and opponent HP is a percentage."""
    checked = 0
    for k in range(30):
        seed = f"sodium,{k + 7:032x}"
        orig, snap, prefix, _ = snapshot_at_turn(seed, 2 + k % 5, rng_seed=k)
        if orig.ended:
            continue
        for p in (prefix, None):
            r = vgc_engine.Battle.from_position(snap, prefix=p)
            drive(r, random.Random(k), rounds=2)
            for side in ("p1", "p2"):
                obs = r.observe(side)
                stream = r.player_stream(side)
                seen, text = _seen(without_side(stream), side)
                for m in obs["sides"][1 if side == "p1" else 0]["pokemon"]:
                    assert m["hp_exact"] is False
                    assert m["max_hp"] in (None, 0, 100), m
                    for mv in m["moves"]:
                        assert mv in seen, (mv, side)
                    for key in ("item", "ability"):
                        if m[key]:
                            assert m[key] in seen, (key, m[key])
                    if m["tera_type"]:
                        assert f"-terastallize|" in text
                checked += 1
    assert checked >= 100


# --------------------------------------------------------------------------
# position_from_board
# --------------------------------------------------------------------------

def mon(species, moves, ability="", item="", hp=1.0, status="", boosts=None, **kw):
    """mimikyu litmus `_common.mon` shape."""
    m = {"species": species, "hp": hp, "status": status, "moves": moves, "item": item, "ability": ability,
         "boosts": boosts or [0] * 7, "is_mega": False, "alive": hp > 0, "volatile_statuses": [],
         "substitute_hp_frac": 0.0, "sleep_turns_remaining": 0, "toxic_counter": 0, "locked_to_move": "",
         "last_move": "", "move_pp": [None] * 4}
    m.update(kw)
    return m


def board():
    return {
        "own_active": [mon("Incineroar", ["Fake Out", "Flare Blitz", "Parting Shot", "Knock Off"], "Intimidate", "Sitrus Berry", hp=0.5,
                           boosts=[-1, 0, 0, 0, 0, 0, 0], turns_on_field=2, last_move="Flare Blitz"),
                       mon("Kingambit", ["Kowtow Cleave", "Sucker Punch", "Protect", "Swords Dance"], "Defiant", "Black Glasses",
                           status="tox", toxic_counter=3, volatile_statuses=["TAUNT"], volatile_ages=[2] + [0] * 11)],
        "own_bench": [mon("Amoonguss", ["Spore", "Rage Powder", "Pollen Puff", "Protect"], "Regenerator", "Rocky Helmet", hp=0.0)],
        "opp_active": [mon("Charizard-Mega-Y", ["Heat Wave", "Solar Beam", "Protect", "Scorching Sands"], "Drought", "Charizardite Y"),
                       mon("Whimsicott", ["Tailwind", "Moonblast", "Encore", "Protect"], "Prankster", "", status="slp",
                           sleep_turns_remaining=2, item_confidence=0.0)],
        "opp_bench": [mon("Garchomp", ["Earthquake", "Dragon Claw", "Protect", "Rock Slide"], "Rough Skin", "")],
        "field": {"turn": 6, "weather": "SunnyDay", "weather_elapsed": 3, "trick_room": True, "trick_room_elapsed": 2,
                  "tailwind_opp": True, "screens_own": [False, True, False], "reflect_own_elapsed": 1,
                  "hazards_opp": {"stealth_rock": 1, "spikes": 2}},
    }


SETS = {"opp": {"Whimsicott": {"item": "Focus Sash", "nature": "Timid", "sps": {"hp": 2, "spe": 32, "spa": 32}},
                "Garchomp": {"item": "Life Orb", "nature": "Jolly", "evs": [4, 252, 0, 0, 0, 252]}}}


def test_builder_maps_board_fields():
    js = json.loads(vgc_engine.position_from_board(board(), SETS, format=FORMAT))
    assert js["format_id"] == FORMAT and js["turn"] == 5
    inc, gambit, goon = js["p1"]["team"]
    assert inc["current_hp"] == round(0.5 * inc["stats"]["hp"]) and inc["boosts"][0] == -1
    assert inc["turns_active"] == 1 and inc["last_used_move_slot"] == 1
    assert goon["fainted"] and goon["current_hp"] == 0
    vols = {v["kind"]: v for v in gambit["volatiles"]["items"][:gambit["volatiles"]["len"]]}
    assert vols["ToxicCounter"]["payload"] == 3 and gambit["status"] == "Toxic"
    assert vols["Taunt"]["turns_remaining"] == 1  # 3 - age 2
    zard, whim, chomp = js["p2"]["team"]
    assert zard["species_id"] != zard["set_species_id"] and js["p2"]["conditions"]["mega_used"]
    vols = {v["kind"]: v for v in whim["volatiles"]["items"][:whim["volatiles"]["len"]]}
    assert vols["Sleep"]["payload"] == 2
    assert whim["consumed_item"] == 65535  # opponent's unknown item stays held
    assert js["weather"] == "Sun" and js["weather_turns"] == 3  # 5 + 1 - 3
    assert js["trick_room_turns"] == 4
    assert js["p2"]["conditions"]["tailwind_turns"] == 4 and js["p1"]["conditions"]["reflect_turns"] == 5
    assert js["p2"]["conditions"]["stealth_rock"] and js["p2"]["conditions"]["spikes_layers"] == 2
    b = vgc_engine.Battle.from_position(js)
    req = json.loads(b.request("p1"))
    # Taunted Kingambit can't pick Swords Dance / Protect.
    moves = {m["id"]: m["disabled"] for m in req["active"][1]["moves"]}
    assert moves["swordsdance"] and moves["protect"] and not moves["kowtowcleave"]
    assert b.turn == 5 and "|turn|6" in b.player_stream("p1")


def test_builder_side_of_own_and_item_extension():
    bd = board()
    bd["opp_active"][0] = mon("Torkoal", ["Eruption", "Protect", "Earth Power", "Sunny Day"], "Drought", "Heat Rock")
    bd["opp_active"][1]["sleep_turns_remaining"] = 0  # hidden: defaults to 3 - sleep_elapsed
    js = json.loads(vgc_engine.position_from_board(bd, SETS, format=FORMAT, side_of_own="p2"))
    assert js["p2"]["team"][0]["species_id"] == json.loads(vgc_engine.position_from_board(board(), SETS, format=FORMAT))["p1"]["team"][0]["species_id"]
    assert js["weather_turns"] == 6  # Heat Rock: 8 + 1 - 3
    whim = js["p1"]["team"][1]
    vols = {v["kind"]: v for v in whim["volatiles"]["items"][:whim["volatiles"]["len"]]}
    assert vols["Sleep"]["payload"] == 3


@pytest.mark.parametrize("patch,msg", [
    (lambda b: b["own_active"].append(b["own_active"][0]), "active slot"),
    (lambda b: b["own_active"][0].update(species="Missingno"), "unknown species"),
    (lambda b: b["own_active"][0]["moves"].append("Splash"), "not in its set"),
    (lambda b: b["own_active"][0].update(status="zzz"), "unknown status"),
    (lambda b: b["own_active"][1].update(volatile_statuses=["SKYDROP"]), "not supported"),
    (lambda b: b["own_bench"][0].update(hp=0.5, alive=True, boosts=[1, 0, 0, 0, 0, 0, 0]), "bench"),
    (lambda b: b["field"].update(weather="Fog"), "weather"),
    (lambda b: b["field"].update(trick_room_elapsed=7), "trick_room_elapsed"),
])
def test_builder_rejects_inconsistent_boards(patch, msg):
    bd = board()
    if msg == "not in its set":
        sets = {"own": {"Incineroar": {"moves": ["Fake Out", "Flare Blitz", "Parting Shot", "Knock Off"]}}}
    else:
        sets = SETS
    patch(bd)
    with pytest.raises(ValueError, match=msg):
        vgc_engine.position_from_board(bd, sets, format=FORMAT)
