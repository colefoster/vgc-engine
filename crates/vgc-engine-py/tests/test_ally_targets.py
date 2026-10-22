"""Opt-in `ally_targets` (docs/protocol.md, "Ally targets").

Build/install the extension first (`maturin develop`), then run
`python -m pytest crates/vgc-engine-py/tests/`.
"""

import json

import pytest
import vgc_engine

P1 = json.dumps([
    {"species": "staraptor", "level": 50, "moves": ["tackle", "bravebird", "mefirst", "earthquake"]},
    {"species": "ribombee", "level": 50, "moves": ["helpinghand", "swordsdance", "rockslide", "pollenpuff"]},
])
P2 = json.dumps([
    {"species": "blissey", "level": 50, "moves": ["tackle"]},
    {"species": "chansey", "level": 50, "moves": ["tackle"]},
])


def ally(b, side=0, slot=0):
    return [c for c in b.legal_choices(side, slot) if c[3] == side and c[4] == 1 - slot]


def test_default_off_and_kwarg():
    off = vgc_engine.Battle.from_teams(P1, P2, format="doubles")
    assert off.ally_targets is False
    assert ally(off) == []
    on = vgc_engine.Battle.from_teams(P1, P2, format="doubles", ally_targets=True)
    assert on.ally_targets is True
    # Tackle (normal) and Brave Bird (any), each with its Tera variant; never
    # Me First (adjacentFoe) or Earthquake (spread).
    assert sorted((c[0], c[2]) for c in ally(on)) == [("move", 0), ("move", 1), ("tera", 0), ("tera", 1)]
    # Flag on only adds choices, after each move's foe choices.
    lc = on.legal_choices(0, 0)
    assert [c for c in lc if c not in ally(on)] == off.legal_choices(0, 0)
    assert lc[0][3] == 1


def test_property_setter_and_clone():
    b = vgc_engine.Battle.from_teams(P1, P2, format="doubles")
    b.ally_targets = True
    assert ally(b) and b.clone().ally_targets
    b.ally_targets = False
    assert ally(b) == []


def test_snapshots_need_the_kwarg():
    on = vgc_engine.Battle.from_teams(P1, P2, format="doubles", ally_targets=True)
    off = vgc_engine.Battle.from_teams(P1, P2, format="doubles")
    assert on.to_json() == off.to_json()
    assert vgc_engine.Battle.from_json(on.to_json()).ally_targets is False
    assert vgc_engine.Battle.from_json(on.to_json(), ally_targets=True).ally_targets is True


# to_bytes -> from_bytes fails on main for any battle: Battle.fairy_lock_turns
# is `skip_serializing_if`, which bincode (not self-describing) can't read back.
@pytest.mark.xfail(reason="bincode round-trip broken by skip_serializing_if fairy_lock_turns", raises=ValueError)
def test_from_bytes_kwarg():
    on = vgc_engine.Battle.from_teams(P1, P2, format="doubles", ally_targets=True)
    assert vgc_engine.Battle.from_bytes(on.to_bytes(), ally_targets=True).ally_targets is True
    assert vgc_engine.Battle.from_bytes(on.to_bytes()).ally_targets is False


def test_from_position_kwarg_and_session_setter():
    snap = vgc_engine.Battle.from_teams(P1, P2, format="doubles").to_json()
    b = vgc_engine.Battle.from_position(snap, ally_targets=True)
    assert b.ally_targets and ally(b)
    b = vgc_engine.Battle.from_position(snap)
    assert not b.ally_targets and ally(b) == []
    b.ally_targets = True
    assert ally(b)
    # The session keeps the flag after a turn.
    assert b.choose("p1", "move 1 -2, move 2") is None
    assert b.choose("p2", "move 1 1, move 1 1") is None
    assert b.ally_targets and ally(b)


def test_step_move_round_trips_an_ally_choice():
    b = vgc_engine.Battle.from_teams(P1, P2, format="doubles", ally_targets=True)
    hp = b.active_hp(0, 1)
    tackle_ally = next(c for c in ally(b) if c[0] == "move" and c[2] == 0)
    b.step_move([tackle_ally, ("move", 1, 1, -1, -1)], [("pass", 0, 0, -1, -1), ("pass", 1, 0, -1, -1)])
    assert b.active_hp(0, 1) < hp
