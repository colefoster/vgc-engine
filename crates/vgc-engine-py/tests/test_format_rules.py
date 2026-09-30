"""Champions rules follow the `format` passed to `Battle.from_teams`."""

import json

import pytest
import vgc_engine

TEAM = json.dumps([{"species": "pikachu", "level": 50, "moves": ["tackle"]}] * 2)


@pytest.mark.parametrize(
    "fmt, expected",
    [
        ("gen9championsvgc2026regmc", True),
        ("gen9championsvgc2026regmb", True),
        ("gen9vgc2025regh", False),
        ("gen9doublescustomgame", False),
        ("doubles", True),  # bare game type keeps the engine's target format
    ],
)
def test_champions_derived_from_format(fmt, expected):
    assert vgc_engine.Battle.from_teams(TEAM, TEAM, format=fmt).champions is expected


def test_explicit_champions_overrides_format():
    b = vgc_engine.Battle.from_teams(TEAM, TEAM, format="gen9vgc2025regh", champions=True)
    assert b.champions is True
    b = vgc_engine.Battle.from_teams(TEAM, TEAM, format="gen9championsvgc2026regmc", champions=False)
    assert b.champions is False


PP_TEAM = json.dumps([{"species": "garchomp", "level": 50, "moves": ["earthquake", "protect", "scratch"]}] * 2)


def _pp(battle):
    moves = battle.observe()["sides"][0]["active"][0]["moves"]
    return [(m["pp"], m["max_pp"]) for m in moves]


@pytest.mark.parametrize(
    "kwargs, expected",
    [
        # PS data/mods/champions/scripts.ts: base PP capped at 20, Protect 5,
        # calculatePP (pp / 5 + 1) * 4.
        ({"format": "gen9championsvgc2026regmc"}, [(12, 12), (8, 8), (20, 20)]),
        ({"format": "doubles"}, [(12, 12), (8, 8), (20, 20)]),
        ({"format": "gen9vgc2025regh", "champions": True}, [(12, 12), (8, 8), (20, 20)]),
        # Standard gen 9: three PP Ups, pp * 8 / 5.
        ({"format": "gen9vgc2025regh"}, [(16, 16), (16, 16), (56, 56)]),
    ],
)
def test_team_pp_follows_the_format(kwargs, expected):
    assert _pp(vgc_engine.Battle.from_teams(PP_TEAM, PP_TEAM, **kwargs)) == expected
