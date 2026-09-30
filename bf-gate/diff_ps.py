#!/usr/bin/env python3
"""Differential: engine vs PS (Champions mod) on the real-hit scenarios, same inputs.
Uses the TIGHT query per eligible hit (most-common Reg M-B spread; a role default
32 HP / 32 offence when the species has no M-B stats). Writes diff_ps.json."""
import collections, json, subprocess, sys, os
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
H = Path(__file__).resolve().parent
sys.path.insert(0, str(H))
import eval_damage as E
from extract import slug

D = json.load(open(H / "damage_eval.json"))
todo = D["todo"]
orig_top = E.stats_top
def top(species, field):
    t = orig_top(species, field)
    if t is None and field == "spreads":
        return {"nature": "serious", "stat_points": "32/32/0/32/0/0"}
    return t
E.stats_top = top
qs, idx = [], []
for n, h in enumerate(todo):
    b = E.build_queries(h, "tight")
    if not b: continue
    qs.append(b[0][1]); idx.append(n)
print("queries", len(qs), file=sys.stderr)
eng = E.run_queries(qs)
def ps_chunk(chunk):
    if E.WORKAROUND:
        chunk = [E.neutralise(json.loads(json.dumps(q))) for q in chunk]
    out = subprocess.run(["node", str(H / "ps_calc.js")], input="\n".join(json.dumps(q) for q in chunk) + "\n",
                         capture_output=True, text=True, check=True).stdout
    return [json.loads(l) for l in out.splitlines()]
K = 12; size = len(qs) // K + 1
with ThreadPoolExecutor(K) as ex:
    parts = list(ex.map(ps_chunk, [qs[i:i + size] for i in range(0, len(qs), size)]))
ps = [r for p in parts for r in p]
assert len(ps) == len(qs)
res = []
for n, q, e, p in zip(idx, qs, eng, ps):
    res.append({"n": n, "q": q, "eng": e, "ps": p})
json.dump(res, open(H / ("diff_ps_wa.json" if E.WORKAROUND else "diff_ps.json"), "w"))
c = collections.Counter()
for r in res:
    e, p = r["eng"], r["ps"]
    if slug(r["q"]["move"]) in ("electroshot", "meteorbeam"): c["oracle_skip_charge_move"] += 1
    elif "err" in e or "err" in p: c["error"] += 1
    elif e["hp"] != p["hp"]: c["hp_diff"] += 1
    elif e["rolls"] == p["rolls"]: c["exact"] += 1
    elif max(abs(a - b) for a, b in zip(e["rolls"], p["rolls"])) <= 1: c["off_by_1"] += 1
    else: c["diff"] += 1
print(dict(c), file=sys.stderr)
