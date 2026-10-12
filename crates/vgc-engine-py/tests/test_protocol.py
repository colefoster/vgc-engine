"""The Showdown protocol log through the pyo3 binding (docs/protocol.md).

Build/install the extension first (`maturin develop`), then run
`python -m pytest crates/vgc-engine-py/tests/`.
"""

import json

import pytest
import vgc_engine

P1 = json.dumps([
    {"species": "garchomp", "level": 50, "item": "lifeorb", "ability": "roughskin",
     "moves": ["earthquake", "dragonclaw", "protect", "rockslide"]},
    {"species": "incineroar", "level": 50, "item": "sitrusberry", "ability": "intimidate",
     "moves": ["fakeout", "flareblitz", "partingshot", "knockoff"]},
])
P2 = json.dumps([
    {"species": "tyranitar", "level": 50, "item": "leftovers", "ability": "sandstream",
     "moves": ["rockslide", "crunch", "protect", "lowkick"]},
    {"species": "amoonguss", "level": 50, "item": "rockyhelmet", "ability": "regenerator",
     "moves": ["spore", "ragepowder", "pollenpuff", "protect"]},
])


def test_no_log_unless_asked():
    assert vgc_engine.Battle.from_teams(P1, P2).protocol_log() is None


def test_ps_log_has_preamble_and_leads():
    b = vgc_engine.Battle.from_teams(P1, P2, protocol="ps", player_names=("Alice", "Bob"))
    log = b.protocol_log()
    assert "|player|p1|Alice||" in log
    assert "|start" in log
    assert "|switch|p1a: Garchomp|Garchomp, L50, M|183/183" in log
    assert log[-1] == "|turn|1"


def test_views_and_compact():
    b = vgc_engine.Battle.from_teams(P1, P2, protocol="ps")
    spectator = b.protocol_log(view="spectator")
    assert not any(l.startswith("|split|") for l in spectator)
    assert "|switch|p1a: Garchomp|Garchomp, L50, M|100/100" in spectator
    assert "|switch|p1a: Garchomp|Garchomp, L50, M|183/183" in b.protocol_log(view="p1")
    assert "|switch|p1a: Garchomp|Garchomp, L50, M|183/183" not in b.protocol_log(view="p2")
    compact = vgc_engine.Battle.from_teams(P1, P2, protocol="compact").protocol_log()
    assert not any(l.startswith(("|t:|", "|player|", "|gametype|")) for l in compact)


def test_bad_arguments():
    with pytest.raises(ValueError):
        vgc_engine.Battle.from_teams(P1, P2, protocol="verbose")
    with pytest.raises(ValueError):
        vgc_engine.Battle.from_teams(P1, P2, protocol="ps").protocol_log(view="p3")
