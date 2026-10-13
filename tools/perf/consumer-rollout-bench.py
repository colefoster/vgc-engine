#!/usr/bin/env python3
"""Consumer rollout benchmark: mimikyu RolloutOracle._one_rollout vs pinned PS.

Times the real consumer operation -- ``RolloutOracle._one_rollout(battle,
joint_action, own_side, seed, opp_reply)`` from mimikyu's
``ml/vgc_model/inference/rollout_oracle.py``, imported read-only from its file
-- against a matched Pokemon Showdown rollout run by a steady-state Node
process (``consumer-rollout-ps.js``). Both sides get the same synthetic
Champions doubles teams (literal level 50, zero Stat Points, the same
abilities), the same explicit initial joint action and opponent reply, the
same ``max_turns`` bound and winner perspective.

Engine timings include everything ``_one_rollout`` does per rollout: the
``reseed_battle`` JSON round trip, the Python policy and every ``step_move``
crossing. PS timings include the equivalent serialize / reseed / deserialize
of the base battle and the policy; Node starts once and is not timed. PS is
reported both in-process (``internal``) and as Python wall time around each
batch request (one pipe round trip per batch).

Fidelity runs first and untimed: every seed's terminal result, step count,
value, final HP and per-turn choices on both sides. The engine choices come
from the real ``first_move_policy`` (recorded through a wrapper); PS uses an
explicit, independent rule (move 1, first living foe). Any difference is
reported, and the speedup is withheld unless fidelity is 100%.

No models, datasets, network or package installs. The engine wheel comes from
``--engine-pythonpath`` (or the caller's PYTHONPATH).

Example:
  PYTHONDONTWRITEBYTECODE=1 python3 tools/perf/consumer-rollout-bench.py \\
      --engine-pythonpath <dir with vgc_engine> --out /tmp/rollout-bench.json
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import statistics
import subprocess
import sys
import time

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
# mimikyu is assumed to be checked out next to this repo unless $MIMIKYU_DIR says otherwise.
MIMIKYU_DIR = os.environ.get("MIMIKYU_DIR", os.path.join(os.path.dirname(REPO), "mimikyu"))
DEFAULT_ORACLE = os.path.join(MIMIKYU_DIR, "ml", "vgc_model", "inference", "rollout_oracle.py")
_U64 = (1 << 64) - 1


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--workload", default=os.path.join(HERE, "consumer-rollout-workload.json"),
                   help="workload JSON (teams, joint action, bounds, batch sizes)")
    p.add_argument("--engine-pythonpath", action="append", default=[],
                   help="directory prepended to sys.path to import vgc_engine (repeatable)")
    p.add_argument("--oracle", default=DEFAULT_ORACLE,
                   help="path to mimikyu rollout_oracle.py (imported read-only; "
                        "default $MIMIKYU_DIR/ml/vgc_model/inference/rollout_oracle.py)")
    p.add_argument("--ps-path", default=os.environ.get("PS_PATH"),
                   help="pinned pokemon-showdown checkout (env PS_PATH)")
    p.add_argument("--ps-dist", default=os.environ.get("PS_DIST"),
                   help="built PS dist/sim (env PS_DIST; default <ps-path>/dist/sim)")
    p.add_argument("--node", default="node", help="node executable")
    p.add_argument("--node-arg", action="append", default=[],
                   help="extra node argument, e.g. --node-arg=--require=<offline guard> (repeatable)")
    p.add_argument("--rollouts", type=int, help="rollouts per batch, 1-32 (overrides workload)")
    p.add_argument("--timed-batches", type=int, help="timed batches, 1-5 (overrides workload)")
    p.add_argument("--warmup-batches", type=int, help="warmup batches, 0-1 (overrides workload)")
    p.add_argument("--engine-source-head",
                   help="vgc-engine commit the wheel was built from, as stated by the caller; "
                        "recorded as provided, not verified")
    p.add_argument("--out", help="also write the JSON report here")
    a = p.parse_args()
    if not a.ps_path and not a.ps_dist:
        p.error("set --ps-path/$PS_PATH (a pinned pokemon-showdown checkout) or --ps-dist/$PS_DIST")
    return a


def sha256(path: str) -> str | None:
    try:
        with open(path, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return None


def git_head(path: str) -> str | None:
    try:
        return subprocess.run(["git", "-C", path, "rev-parse", "HEAD"], capture_output=True,
                              text=True, timeout=10, check=True).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None


def splitmix64(x: int) -> int:
    x = (x + 0x9E3779B97F4A7C15) & _U64
    z = x
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & _U64
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & _U64
    return z ^ (z >> 31)


def ps_seed(seed: int) -> str:
    """Gen 5 PS seed string ``a,b,c,d`` (16-bit parts) derived from an int."""
    z = splitmix64(seed)
    return ",".join(str((z >> s) & 0xFFFF) for s in (48, 32, 16, 0))


def load_oracle(path: str):
    name = "rollout_oracle_ro"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot import rollout oracle from {path}")
    mod = importlib.util.module_from_spec(spec)
    # dataclasses resolves annotations through sys.modules[cls.__module__]
    # (Python 3.9 dataclasses._is_type), so register before executing.
    sys.modules[name] = mod
    try:
        spec.loader.exec_module(mod)
    except BaseException:
        del sys.modules[name]
        raise
    return mod


MAX_ROLLOUTS, MAX_TIMED, MAX_WARMUP, MAX_TURNS = 32, 5, 1, 8


def bounded(name: str, v, lo: int, hi: int) -> int:
    if isinstance(v, bool) or not isinstance(v, int) or not lo <= v <= hi:
        raise SystemExit(f"{name} must be an integer in [{lo}, {hi}], got {v!r}")
    return v


def validate_workload(w: dict, n: int) -> None:
    """Reject workloads outside what the matched comparison supports: a
    Champions doubles 2v2 with no bench, literal level 50, zero Stat Points
    (no evs/ivs/item keys, since both sides build them as zero / 31 / none)
    and explicit per-slot actions that name a real move slot."""
    def fail(msg: str):
        raise SystemExit(f"workload: {msg}")

    if not isinstance(w.get("format"), str) or not w["format"].startswith("gen9champions"):
        fail(f"format must be a gen9champions* id, got {w.get('format')!r}")
    bounded("workload own_side", w.get("own_side"), 0, 1)
    bounded("workload init_seed", w.get("init_seed"), 0, _U64)
    bounded("workload seed0", w.get("seed0"), 0, _U64 - (n - 1))
    for side in ("p1", "p2"):
        team = w.get(side)
        if not isinstance(team, list) or len(team) != 2:
            fail(f"{side} must list exactly 2 sets (doubles, no bench)")
        for i, s in enumerate(team):
            where = f"{side}[{i}]"
            if not isinstance(s, dict):
                fail(f"{where} must be an object")
            extra = set(s) - {"species", "gender", "ability", "nature", "level", "moves"}
            if extra:
                fail(f"{where} has unsupported keys {sorted(extra)} (zero Stat Points, no items)")
            for k in ("species", "ability", "nature"):
                if not isinstance(s.get(k), str) or not s[k]:
                    fail(f"{where}.{k} must be a non-empty string")
            if s.get("level") != 50:
                fail(f"{where}.level must be 50")
            if s.get("gender") not in (None, "M", "F", "N"):
                fail(f"{where}.gender must be M, F or N")
            mv = s.get("moves")
            if not isinstance(mv, list) or not 1 <= len(mv) <= 4 or not all(isinstance(m, str) and m for m in mv):
                fail(f"{where}.moves must list 1-4 move names")
    for key, side in (("joint_action", w["own_side"]), ("opp_reply", 1 - w["own_side"])):
        acts = w.get(key)
        team = w["p1" if side == 0 else "p2"]
        if not isinstance(acts, list) or len(acts) != 2:
            fail(f"{key} must give exactly 2 slot actions")
        for slot, act in enumerate(acts):
            if not isinstance(act, dict) or set(act) != {"move", "target"}:
                fail(f"{key}[{slot}] must be {{'move': int, 'target': null | [kind, slot]}}")
            bounded(f"workload {key}[{slot}].move", act["move"], 0, len(team[slot]["moves"]) - 1)
            t = act["target"]
            if t is not None and (not isinstance(t, list) or len(t) != 2 or t[0] not in ("foe", "ally")
                                  or isinstance(t[1], bool) or t[1] not in (0, 1)):
                fail(f"{key}[{slot}].target must be null or ['foe'|'ally', 0|1], got {t!r}")


def engine_team(sets: list) -> str:
    return json.dumps([{
        "species": s["species"], "ability": s["ability"], "nature": s["nature"],
        "level": s["level"], "moves": s["moves"], "gender": s.get("gender"),
        "evs": {"hp": 0, "atk": 0, "def": 0, "spa": 0, "spd": 0, "spe": 0},
    } for s in sets])


def resolve_engine_action(battle, side: int, spec: list) -> list:
    """Workload slot specs -> legal engine 5-tuples (error if not legal)."""
    out = []
    for slot, s in enumerate(spec):
        if s["target"] is None:
            want_side, want_slot = -1, -1
        else:
            kind, tslot = s["target"]
            want_side = 1 - side if kind == "foe" else side
            want_slot = tslot
        legal = [tuple(c) for c in battle.legal_choices(side, slot)]
        moves = [c for c in legal if c[0] == "move" and c[2] == s["move"]]
        match = [c for c in moves if (c[3], c[4]) == (want_side, want_slot)]
        if not match and s["target"] is None and len(moves) == 1:
            match = moves  # untargeted move the engine encodes with a target
        if not match:
            raise SystemExit(f"side {side} slot {slot}: {s} not among legal {legal}")
        out.append(match[0])
    return out


def ps_action(spec: list) -> str:
    toks = []
    for s in spec:
        tok = f"move {s['move'] + 1}"
        if s["target"] is not None:
            kind, tslot = s["target"]
            tok += f" {tslot + 1}" if kind == "foe" else f" -{tslot + 1}"
        toks.append(tok)
    return ", ".join(toks)


def tuple_to_ps(c, side: int) -> str:
    """Engine choice tuple -> PS choice token, for the per-turn comparison."""
    kind, _slot, arg, ts, tl = c
    if kind == "pass":
        return "pass"
    if kind != "move":
        return f"<{kind} {arg} {ts} {tl}>"
    tok = f"move {arg + 1}"
    if ts >= 0:
        tok += f" {tl + 1}" if ts != side else f" -{tl + 1}"
    return tok


def engine_hp(battle) -> list:
    obs = battle.observe()
    out = []
    for s in sorted(obs["sides"], key=lambda s: s["side"]):
        out.append([[m["species"], m["hp"], m["max_hp"]] for g in ("active", "bench") for m in s.get(g, [])])
    return out


def dist(xs: list) -> dict:
    q = statistics.quantiles(xs, n=4, method="inclusive") if len(xs) > 1 else [xs[0]] * 3
    return {"median": statistics.median(xs), "min": min(xs), "max": max(xs),
            "q1": q[0], "q3": q[2], "iqr": q[2] - q[0], "samples": xs}


class PsProc:
    def __init__(self, node: str, node_args: list, script: str, ps_dist: str):
        env = dict(os.environ, PS_DIST=ps_dist)
        self.p = subprocess.Popen([node, *node_args, script], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, text=True, env=env, bufsize=1)

    def call(self, req: dict) -> dict:
        self.p.stdin.write(json.dumps(req) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise SystemExit(f"PS process exited (code {self.p.poll()})")
        resp = json.loads(line)
        if not resp.get("ok"):
            raise SystemExit(f"PS error: {resp.get('error')}")
        return resp

    def close(self):
        if self.p.poll() is None:
            try:
                self.p.stdin.write(json.dumps({"op": "exit"}) + "\n")
                self.p.stdin.close()
                self.p.wait(timeout=10)
            except (OSError, subprocess.TimeoutExpired):
                self.p.kill()
                self.p.wait()


def main() -> int:
    a = parse_args()
    for d in reversed(a.engine_pythonpath):
        sys.path.insert(0, d)
    import vgc_engine as ve  # noqa: E402  (path set above)

    with open(a.workload) as f:
        w = json.load(f)
    if not isinstance(w, dict):
        raise SystemExit("workload must be a JSON object")
    n = bounded("rollouts", a.rollouts if a.rollouts is not None else w.get("rollouts_per_batch"),
                1, MAX_ROLLOUTS)
    timed = bounded("timed batches", a.timed_batches if a.timed_batches is not None else w.get("timed_batches"),
                    1, MAX_TIMED)
    warm = bounded("warmup batches",
                   a.warmup_batches if a.warmup_batches is not None else w.get("warmup_batches"),
                   0, MAX_WARMUP)
    max_turns = bounded("max_turns", w.get("max_turns"), 1, MAX_TURNS)
    validate_workload(w, n)
    own = w["own_side"]
    oracle_mod = load_oracle(a.oracle)
    seeds = [w["seed0"] + i for i in range(n)]
    ps_dist = a.ps_dist or os.path.join(a.ps_path, "dist", "sim")

    base = ve.Battle.from_teams(engine_team(w["p1"]), engine_team(w["p2"]), w["format"], w["init_seed"])
    joint = resolve_engine_action(base, own, w["joint_action"])
    opp = resolve_engine_action(base, 1 - own, w["opp_reply"])
    p_own, p_opp = ps_action(w["joint_action"]), ps_action(w["opp_reply"])
    p1_init, p2_init = (p_own, p_opp) if own == 0 else (p_opp, p_own)
    eng_p1, eng_p2 = (joint, opp) if own == 0 else (opp, joint)

    ps = PsProc(a.node, a.node_arg, os.path.join(HERE, "consumer-rollout-ps.js"), ps_dist)
    try:
        init = ps.call({"op": "init", "format": w["format"], "p1": w["p1"], "p2": w["p2"],
                        "init_seed": ps_seed(w["init_seed"])})
        run_req = {"op": "run", "p1_initial": p1_init, "p2_initial": p2_init,
                   "own_side": own, "max_turns": max_turns}

        # --- fidelity (untimed) ------------------------------------------
        rec: list = []

        def recording_policy(battle, side):
            out = oracle_mod.first_move_policy(battle, side)
            rec.append((side, [tuple(c) for c in out], battle))
            return out

        rec_oracle = oracle_mod.RolloutOracle(policy=recording_policy, max_turns=max_turns)
        ps_fid = ps.call({**run_req, "seeds": [ps_seed(s) for s in seeds], "record": True})["rollouts"]
        mismatches = []
        engine_traj = []
        initial_match = engine_hp(base) == init["final_hp"]
        if not initial_match:
            mismatches.append({"initial_state": {"engine": engine_hp(base), "ps": init["final_hp"]}})
        for seed, pr in zip(seeds, ps_fid):
            rec.clear()
            val, dec = rec_oracle._one_rollout(base, joint, own, seed, opp)
            turns = [[", ".join(tuple_to_ps(c, 0) for c in eng_p1), ", ".join(tuple_to_ps(c, 1) for c in eng_p2)]]
            for i in range(0, len(rec), 2):
                turns.append([", ".join(tuple_to_ps(c, s) for c in ch) for s, ch, _ in rec[i:i + 2]])
            final = engine_hp(rec[-1][2]) if rec else None
            if not dec:
                res = "truncated"
            elif val == 0.5:
                res = "tie"
            else:
                res = "p1_win" if (val == 1.0) == (own == 0) else "p2_win"
            e = {"seed": seed, "result": res, "steps": 1 + len(rec) // 2, "value": val,
                 "decisive": dec, "final_hp": final, "choices": turns}
            engine_traj.append(e)
            p = {"seed": seed, "result": pr["result"] if pr["decisive"] else "truncated",
                 "steps": pr["steps"], "value": pr["value"], "decisive": pr["decisive"],
                 "final_hp": pr["final_hp"], "choices": pr["choices"]}
            diff = {k: {"engine": e[k], "ps": p[k]} for k in
                    ("result", "steps", "value", "decisive", "final_hp", "choices") if e[k] != p[k]}
            if diff:
                mismatches.append({"seed": seed, "ps_seed": pr["seed"], "diff": diff})
        fidelity_ok = not mismatches

        # --- timing --------------------------------------------------------
        oracle = oracle_mod.RolloutOracle(max_turns=max_turns)  # default first_move_policy
        ps_req = {**run_req, "seeds": [ps_seed(s) for s in seeds], "record": False}

        def engine_batch() -> int:
            t0 = time.perf_counter_ns()
            for s in seeds:
                oracle._one_rollout(base, joint, own, s, opp)
            return time.perf_counter_ns() - t0

        def ps_batch() -> tuple[int, int]:
            t0 = time.perf_counter_ns()
            r = ps.call(ps_req)
            return time.perf_counter_ns() - t0, r["internal_ns"]

        for _ in range(warm):
            engine_batch()
            ps_batch()
        eng_ns, ps_wall_ns, ps_int_ns = [], [], []
        for _ in range(timed):  # interleaved to share any machine drift
            eng_ns.append(engine_batch())
            wall, internal = ps_batch()
            ps_wall_ns.append(wall)
            ps_int_ns.append(internal)
    finally:
        ps.close()

    ext = getattr(ve, "vgc_engine", None)
    ext_file = getattr(ext, "__file__", None)
    per = lambda xs: [x / n / 1e3 for x in xs]  # µs per rollout
    eng_d, wall_d, int_d = dist(per(eng_ns)), dist(per(ps_wall_ns)), dist(per(ps_int_ns))
    speedup = None
    if fidelity_ok:
        speedup = {"ps_wall_over_engine": wall_d["median"] / eng_d["median"],
                   "ps_internal_over_engine": int_d["median"] / eng_d["median"]}
    report = {
        "benchmark": "consumer-rollout",
        "status": "diagnostic",
        "fidelity": {"ok": fidelity_ok, "rollouts": len(seeds), "initial_state_match": initial_match,
                     "mismatches": mismatches, "engine_example": engine_traj[0] if engine_traj else None,
                     "ps_example": ps_fid[0] if ps_fid else None},
        "timing_us_per_rollout": {"engine_one_rollout": eng_d, "ps_wall": wall_d, "ps_internal": int_d},
        "speedup": speedup,
        "speedup_withheld_reason": None if fidelity_ok else "fidelity mismatch; no speed claim",
        "config": {"rollouts_per_batch": n, "warmup_batches": warm, "timed_batches": timed,
                   "max_turns": max_turns, "own_side": own, "seeds": seeds,
                   "ps_seeds": [ps_seed(s) for s in seeds], "engine_joint_action": joint,
                   "engine_opp_reply": opp, "ps_p1_initial": p1_init, "ps_p2_initial": p2_init,
                   "workload": w},
        "provenance": {
            "python": sys.version, "platform": platform.platform(),
            # The package __init__ only re-exports the compiled extension
            # (`from .vgc_engine import *`); hash both, the extension is what ran.
            "vgc_engine_package_file": getattr(ve, "__file__", None),
            "vgc_engine_package_sha256": sha256(getattr(ve, "__file__", "") or ""),
            "vgc_engine_extension_file": ext_file,
            "vgc_engine_extension_sha256": sha256(ext_file) if ext_file else None,
            "vgc_engine_version": getattr(ve, "__version__", None),
            # Caller-stated build source, not checked against the binary; the
            # harness_head below is this script's checkout, not the wheel's.
            "engine_source_head_provided": a.engine_source_head,
            "engine_source_head_verified": False,
            "oracle_path": a.oracle, "oracle_sha256": sha256(a.oracle),
            "mimikyu_head": git_head(os.path.dirname(a.oracle)),
            "ps_path": a.ps_path, "ps_head": git_head(a.ps_path) if a.ps_path else None, "ps_dist": ps_dist,
            "ps_init": {k: init.get(k) for k in ("node", "format", "mod", "stats", "turn")},
            "node_args": a.node_arg, "harness_head": git_head(REPO),
            "harness_sha256": sha256(os.path.abspath(__file__)),
            "ps_script_sha256": sha256(os.path.join(HERE, "consumer-rollout-ps.js")),
            "workload_sha256": sha256(a.workload),
        },
        "limitations": [
            "Synthetic quiet 2v2: fixed-damage Seismic Toss vs Splash. The outcome does not depend on RNG, so "
            "seeds exercise reseeding cost, not outcome variety, and the two sides' different RNG streams "
            "(engine Splitmix vs PS Gen 5 seed) cannot cause divergence here.",
            "Seismic Toss is a contact move; no contact-triggered ability or item is present.",
            "Per-rollout cost is dominated by fixed overhead (reseed JSON round trip, Python policy, "
            "PS state serialization); it says nothing about damage-heavy or wide-branching positions.",
            "The PS policy is an explicit rule (move 1, first living foe), not first_move_policy; "
            "a target-order difference shows up as a choices mismatch.",
            "PS wall time includes one stdin/stdout JSON round trip per batch; PS internal excludes it. "
            "Engine time includes every Python<->Rust crossing _one_rollout makes.",
            "Single machine, small sample (default 5 batches x 32 rollouts); no CPU pinning.",
        ],
    }
    text = json.dumps(report, indent=2, default=list)
    print(text)
    if a.out:
        with open(a.out, "w") as f:
            f.write(text + "\n")
    return 0 if fidelity_ok else 1


if __name__ == "__main__":
    sys.exit(main())
