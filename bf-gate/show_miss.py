#!/usr/bin/env python3
"""Print a miss: record, envelope, and surrounding log lines.  show_miss.py <species> <move> [k]"""
import json, sys
from pathlib import Path
H = Path(__file__).resolve().parent
D = json.load(open(H / "damage_eval.json")); ex = json.load(open(H / "miss_examples.json"))
todo, R = D["todo"], D["results"]
sp, mv = sys.argv[1], sys.argv[2]; k = int(sys.argv[3]) if len(sys.argv) > 3 else 2
idx = {str(p) for p in (H / "data").rglob("*.json")}
shown = 0
for cat, ns in ex.items():
    for n in ns:
        h = todo[n]
        if h["atk"]["species"] != sp or h["move"] != mv: continue
        print("==", cat, h["game"], "turn", h["turn"], "ots", h["ots"])
        for s in ("atk", "def"):
            x = h[s]; print(f"  {s}: {x['species']} item={x['item']}({x['item_known']}) ab={x['ability']}({x['ability_known']}) nat={x['nature']} boosts={x['boosts']} st={x['status']} hp={x['hp']}")
        print("  field", h["weather"], h["terrain"], "spread", h["spread"], "crit", h["crit"], "prev", h["prev"], "new", h["new"], "eff", h["eff"])
        for m in ("wide", "tight"):
            r = R[m].get(str(n)); print("  ", m, r)
        f = next(p for p in idx if h["game"] in p)
        lines = json.load(open(f))["log"].split("\n")
        print("   " + "\n   ".join(lines[max(0, h["line"] - 6):h["line"] + 3]))
        shown += 1
        if shown >= k: sys.exit()
