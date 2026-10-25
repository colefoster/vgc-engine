"""Real replay -> position: vgc_engine.position_from_replay
(docs/protocol.md, "Start from a real replay").

Build/install the extension first (`maturin develop --features ps-rng`), then
run `python -m pytest crates/vgc-engine-py/tests/`.
"""

import json
import random

import pytest
import vgc_engine

from test_position import drive, norm, replay, snapshot_at_turn, without_side
from test_session import P1, P2

SEEDS = [f"sodium,{k:032x}" for k in range(24)]


def stops(turn=4):
    """(original battle at turn `turn`'s request, its requests, omniscient log) per seed."""
    for k, seed in enumerate(SEEDS):
        orig, _snap, _prefix, omni = snapshot_at_turn(seed, turn, rng_seed=k)
        if orig.ended or orig.request("p1") is None or orig.request("p2") is None:
            continue
        if omni[-1] != f"|turn|{turn}":
            continue  # a faint-replacement stop, not the turn's move request
        yield k, orig, {s: orig.request(s) for s in ("p1", "p2")}, omni


def test_full_info_rebuild_gives_the_same_requests():
    n = 0
    for _k, _orig, reqs, omni in stops():
        b = vgc_engine.position_from_replay(omni, 4, request={s: json.loads(r) for s, r in reqs.items()},
                                            sets={"p1": P1, "p2": P2})
        assert {s: b.request(s) for s in ("p1", "p2")} == reqs
        rep = b.reconstruction_report()
        for side in ("p1", "p2"):
            for mon in rep["sides"][side]["team"]:
                # Only the hidden counters (no log or request shows them).
                hidden = {"sleep_turns_remaining", "confusion_turns_remaining"}
                assert set(mon["prior_fields"]) <= hidden, (side, mon["name"], mon["prior_fields"])
        n += 1
    assert n >= 8


def test_full_info_continuation_with_the_true_prng():
    """With every set given and the real PRNG state put back, replaying the
    logged choices reproduces the logged streams."""
    n = 0
    for k, orig, reqs, omni in stops():
        state = json.loads(orig.to_json())
        b = vgc_engine.position_from_replay(omni, 4, request={s: json.loads(r) for s, r in reqs.items()},
                                            sets={"p1": P1, "p2": P2})
        rebuilt = json.loads(b.to_json())
        rebuilt["rng"] = state["rng"]
        prefix = {s: without_side(orig.player_stream(s)) for s in ("p1", "p2")}
        r = vgc_engine.Battle.from_position(json.dumps(rebuilt), prefix=prefix)
        start = {s: len(r.player_chunks(s)) for s in ("p1", "p2")}
        n0 = {s: len(orig.player_chunks(s)) for s in ("p1", "p2")}
        calls = []
        drive(orig, random.Random(100 + k), rounds=6, record=calls)
        replay(r, calls)
        if all(norm(r.player_chunks(s)[start[s]:]) == norm(orig.player_chunks(s)[n0[s]:]) for s in ("p1", "p2")):
            n += 1
    assert n >= 8


def test_spectator_log_with_a_viewer_reports_provenance():
    for _k, orig, _reqs, _omni in stops():
        spect = orig.protocol_log("spectator")
        b = vgc_engine.position_from_replay("\n".join(spect), 4, viewer="p1")
        rep = b.reconstruction_report()
        assert rep["viewer"] == "p1" and rep["input"] == "spectator" and rep["turn"] == 4
        assert set(rep["active_prior_fields"]) == {"p1a", "p1b", "p2a", "p2b"}
        assert rep["sides"]["p2"]["knowledge"] == "viewer"
        for side in ("p1", "p2"):
            for mon in rep["sides"][side]["team"]:
                for f in ("species", "hp", "item", "ability", "nature", "spread", "status"):
                    assert mon[f]["src"] in ("exact", "inferred", "prior", "default")
                for mv in mon["moves"]:
                    assert mv["src"] in ("exact", "prior", "default")
                    if mv["src"] == "prior":
                        assert 0.0 <= mv["p"] <= 1.0
        assert rep["n_prior_fields"] > 0
        # The real history heads every stream.
        assert b.player_stream("p2")[: len(spect)] == spect
        return
    pytest.fail("no stop")


def test_prior_ids_are_pinnable():
    for _k, orig, _reqs, _omni in stops():
        spect = "\n".join(orig.protocol_log("spectator"))
        rep = vgc_engine.position_from_replay(spect, 4).reconstruction_report()
        assert rep["prior"] == "smogon:2026-09/gen9championsvgc2026regmc-1500+bringfour:2026-09/regmc-reveals"
        assert all(len(h) == 64 for h in rep["prior_sha256"].split("+"))
        pinned = vgc_engine.position_from_replay(spect, 4, prior=rep["prior"]).reconstruction_report()
        assert pinned == rep
        old = "smogon:2026-08/gen9championsvgc2026regmb-0+bringfour:2026-09/regmc-reveals"
        assert vgc_engine.position_from_replay(spect, 4, prior=old).reconstruction_report()["prior"] == old
        with pytest.raises(ValueError):
            vgc_engine.position_from_replay(spect, 4, prior="smogon:2026-07/nope")
        return
    pytest.fail("no stop")


def test_prior_sample_seed():
    for _k, orig, _reqs, _omni in stops():
        spect = "\n".join(orig.protocol_log("spectator"))
        argmax = vgc_engine.position_from_replay(spect, 4).reconstruction_report()
        assert "prior_sample_seed" not in argmax
        a = vgc_engine.position_from_replay(spect, 4, prior_sample_seed=3).reconstruction_report()
        assert a == vgc_engine.position_from_replay(spect, 4, prior_sample_seed=3).reconstruction_report()
        assert a["prior_sample_seed"] == 3
        spreads = {json.dumps([m["spread"]["value"] for m in vgc_engine.position_from_replay(
            spect, 4, prior_sample_seed=s).reconstruction_report()["sides"]["p2"]["team"]]) for s in range(10)}
        assert len(spreads) > 1
        return
    pytest.fail("no stop")


def test_ps_seed_play_out_is_deterministic():
    for k, orig, _reqs, _omni in stops():
        log = "\n".join(orig.protocol_log("spectator"))
        outs = []
        for _ in range(2):
            b = vgc_engine.position_from_replay(log, 4, ps_seed="sodium,00000000000000000000000000000042")
            drive(b, random.Random(7), rounds=4)
            outs.append([norm(b.player_chunks(s)) for s in ("p1", "p2")])
        assert outs[0] == outs[1]
        return
    pytest.fail("no stop")


def test_inputs_replay_json_and_player_stream():
    for _k, orig, reqs, _omni in stops():
        spect = orig.protocol_log("spectator")
        a = vgc_engine.position_from_replay({"id": "x", "log": "\n".join(spect)}, 4)
        b = vgc_engine.position_from_replay(spect, 4)
        assert a.to_json() == b.to_json()
        # A ps_bot decision file: p2's own stream and request.
        p2 = without_side(orig.player_stream("p2"))
        dec = {"turn": 4, "request": json.loads(reqs["p2"]), "decide_payload": {"live": {"protocol_log": p2}}}
        c = vgc_engine.position_from_replay(dec)
        rep = c.reconstruction_report()
        assert rep["input"] == "per_side:p2"
        assert c.request("p2") == reqs["p2"], "own side exact from the request"
        assert all(m["hp"]["src"] == "exact" for m in rep["sides"]["p2"]["team"])
        # The p1 stream does not see p2's exact HP.
        assert not any(l.startswith("|switch|p2") and not l.endswith("/100") and "fnt" not in l
                       for l in c.player_stream("p1") if l.count("|") >= 4 and l.split("|")[4][:1].isdigit())
        return
    pytest.fail("no stop")


def test_errors_are_value_errors():
    with pytest.raises(ValueError, match="turn"):
        vgc_engine.position_from_replay("|turn|1", 9)
    with pytest.raises(ValueError, match="viewer"):
        vgc_engine.position_from_replay("|turn|1", 1, viewer="p3")


def test_request_stats_of_a_temporary_forme():
    """A move request reports Aegislash-Blade's stats under "Aegislash"
    details (Stance Change); spread solving tries the other formes of the
    dex number instead of refusing the position."""
    import json
    import os
    fx = json.load(open(os.path.join(os.path.dirname(__file__), "fixtures", "aegislash_blade_request.json")))
    b = vgc_engine.position_from_replay(fx["lines"], fx["turn"], request=fx["request"], viewer=fx["side"])
    rep = b.reconstruction_report()
    assert "aegislash" in json.dumps(rep).lower()
