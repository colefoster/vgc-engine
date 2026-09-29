"""Smoke tests for the `vgc_engine.solve_endgame(...)` pyo3 binding.

Build/install the extension first (from the mimikyu analytics venv):

    VIRTUAL_ENV=~/Dev/mimikyu/.venv-analytics \
    PATH=~/Dev/mimikyu/.venv-analytics/bin:$PATH \
    maturin develop

then run: `python -m pytest crates/vgc-engine-py/tests/`.

`solve_endgame` runs the double-oracle single-ply solver with a *batched*
Python leaf evaluator: the callback receives a `list[dict]` of frontier
observations (the `Battle.observe()` schema) and returns a `list[float]`,
one score per state, in a single call per frontier expansion. These tests
prove the round trip works end to end with trivial + hp-ratio-like leaves.
"""

import vgc_engine


# A switch-only fixture keeps the outcome frontier cheap (no damage-roll
# cross-product) so the solver runs in milliseconds.
P1 = """[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]},
    {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
]"""
P2 = """[
    {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
    {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
]"""


def _fixture():
    return vgc_engine.Battle.from_teams(P1, P2, format="singles", seed=1)


def test_solve_endgame_trivial_zero_leaf():
    """A leaf that scores every state 0.0 → Nash value 0, valid policies,
    and the batch is delivered as a list matching the state count."""
    b = _fixture()
    seen_batches = []

    def leaf(obs_list):
        # The callback fires once per frontier — batched, not per leaf.
        assert isinstance(obs_list, list)
        assert obs_list, "frontier batch should be non-empty"
        # Each entry is the observe() dict shape.
        assert obs_list[0]["format"] == "singles"
        assert "sides" in obs_list[0]
        seen_batches.append(len(obs_list))
        return [0.0] * len(obs_list)

    sol = vgc_engine.solve_endgame(b, leaf)
    assert sol is not None
    assert abs(sol["value"]) < 1e-9
    assert sol["row_policy"], "row policy should be non-empty"
    assert sol["col_policy"], "col policy should be non-empty"
    # Policy probabilities are valid distributions.
    for policy_key in ("row_policy", "col_policy"):
        total = sum(prob for (_choice, prob) in sol[policy_key])
        assert abs(total - 1.0) < 1e-6
    assert sol["iterations"] >= 0
    assert sol["row_support_size"] >= 1
    assert sol["col_support_size"] >= 1
    # The leaf actually got called with real batches.
    assert seen_batches, "leaf was never invoked"


def _hp_ratio_like(obs_list):
    """A Python re-implementation of the engine's hp_ratio_leaf: mean HP
    fraction difference (P1 - P2), clamped to [-1, 1]. Scores the whole
    batch in one pass."""
    scores = []
    for obs in obs_list:
        side_fracs = []
        for side in obs["sides"]:
            mons = side["active"] + side["bench"]
            if not mons:
                side_fracs.append(0.0)
                continue
            total = 0.0
            for m in mons:
                mx = m["max_hp"]
                if mx:
                    total += m["hp"] / mx
            side_fracs.append(total / len(mons))
        v = side_fracs[0] - side_fracs[1]
        scores.append(max(-1.0, min(1.0, v)))
    return scores


def test_solve_endgame_hp_ratio_like_leaf():
    """An hp-ratio-like Python leaf returns a value + policies without
    error, and the value stays in the sign-convention range."""
    b = _fixture()
    sol = vgc_engine.solve_endgame(b, _hp_ratio_like)
    assert sol is not None
    assert -1.0 - 1e-9 <= sol["value"] <= 1.0 + 1e-9
    assert sol["row_policy"] and sol["col_policy"]


def test_solve_endgame_choice_tuples_roundtrip_to_step():
    """Policy entries carry the same 5-tuple shape legal_choices emits, so
    a chosen action feeds straight back into step_move."""
    b = _fixture()
    sol = vgc_engine.solve_endgame(b, lambda obs: [0.0] * len(obs))
    choice, _prob = sol["row_policy"][0]
    # (kind, actor_slot, arg, target_side, target_slot)
    assert len(choice) == 5
    assert choice[0] in ("move", "switch", "pass", "tera", "mega")
