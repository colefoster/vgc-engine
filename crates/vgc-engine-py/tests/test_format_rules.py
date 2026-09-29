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
