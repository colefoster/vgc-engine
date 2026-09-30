#!/usr/bin/env python3
"""Summarise damage_eval.json: fit rates, envelope widths, miss taxonomy."""
import collections, json, statistics, sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from eval_damage import MOVES, slug, ALLY_ATK_MOD, legal_abilities

import os
D = json.load(open(Path(__file__).resolve().parent / os.environ.get("EVAL", "damage_eval.json")))
todo, R = D["todo"], D["results"]

def rate(ns, mode):
    rs = [R[mode][str(n)] for n in ns if str(n) in R[mode] and "err" in R[mode][str(n)] or False]
    ok = [R[mode][str(n)] for n in ns if str(n) in R[mode] and "fit" in R[mode][str(n)]]
    if not ok: return "n/a"
    f = sum(r["fit"] for r in ok)
    return f"{f}/{len(ok)} = {100*f/len(ok):.1f}%"

def width(ns, mode):
    w = [100*(R[mode][str(n)]["max"]-R[mode][str(n)]["min"]) for n in ns if "fit" in R[mode].get(str(n), {})]
    return f"median {statistics.median(w):.1f} pp, p90 {sorted(w)[int(.9*len(w))]:.1f} pp" if w else "n/a"

errs = collections.Counter(R["wide"][str(n)]["err"][:60] for n in range(len(todo)) if "err" in R["wide"].get(str(n), {}))
print("errors (wide):", errs.most_common(8))
allN = list(range(len(todo)))
ots = [n for n in allN if todo[n]["ots"]]
spec = [n for n in allN if not todo[n]["ots"]]
known = [n for n in allN if todo[n]["atk"]["item_known"] and todo[n]["def"]["item_known"] and todo[n]["atk"]["ability_known"] and todo[n]["def"]["ability_known"]]
for name, ns in [("ALL", allN), ("OTS", ots), ("spectator", spec), ("items+abilities known", known)]:
    print(f"\n== {name} (n={len(ns)})")
    for m in ("wide", "ext", "tight"):
        print(f"  {m:5s} fit {rate(ns, m):22s} width {width(ns, m)}")

cat = collections.defaultdict(list)
for n in allN: cat[MOVES[slug(todo[n]['move'])]['category']].append(n)
print("\n== by category")
for c, ns in cat.items():
    print(f"  {c:8s} n={len(ns):6d} wide {rate(ns,'wide')}  OTS-wide {rate([n for n in ns if todo[n]['ots']],'wide')}  tight {rate(ns,'tight')}")
for flag in ("crit", "spread"):
    ns = [n for n in allN if todo[n][flag]]
    print(f"  {flag:8s} n={len(ns):6d} wide {rate(ns,'wide')}  tight {rate(ns,'tight')}")
ns = [n for n in allN if "-Mega" in todo[n]["atk"]["species"]]
print(f"  mega-atk n={len(ns):6d} wide {rate(ns,'wide')}  tight {rate(ns,'tight')}")
ns = [n for n in allN if todo[n]["new"] == "0"]
print(f"  KO hits  n={len(ns):6d} wide {rate(ns,'wide')}  tight {rate(ns,'tight')}")

print("\n== top attacker species (wide / OTS-wide / tight)")
sp = collections.defaultdict(list)
for n in allN: sp[todo[n]["atk"]["species"]].append(n)
for s, ns in sorted(sp.items(), key=lambda x: -len(x[1]))[:15]:
    print(f"  {s:20s} n={len(ns):5d} wide {rate(ns,'wide'):22s} OTS {rate([n for n in ns if todo[n]['ots']],'wide'):20s} tight {rate(ns,'tight')}")

# Miss taxonomy on WIDE.
tax = collections.Counter(); ex = collections.defaultdict(list)
for n in allN:
    r = R["wide"].get(str(n), {})
    if "fit" not in r or r["fit"]: continue
    h = todo[n]; e = R["ext"].get(str(n), {})
    obs_lo, obs_hi = r["obs"]
    near = (r["max"] < obs_lo and obs_lo - r["max"] <= 0.01) or (r["min"] > obs_hi and r["min"] - obs_hi <= 0.01)
    ally_mod = any(any(slug(x) in ALLY_ATK_MOD for x in legal_abilities(s)) for s in h["ally_atk"]) and r["max"] < obs_lo
    fg = any("Friend Guard" in legal_abilities(s) for s in h["ally_def"]) and r["min"] > obs_hi
    if r["zero"]: k = "engine_zero_damage"
    elif e.get("fit"): k = "hidden_info (fits EXT: unrevealed item/ability/nature)"
    elif fg: k = "doubles_ally_mod (possible Friend Guard)"
    elif ally_mod: k = "doubles_ally_mod (Power Spot/Battery/Steely Spirit)"
    elif near: k = "near_miss_<=1pp"
    else: k = "unexplained"
    k2 = k + (" [under]" if r["max"] < obs_lo else " [over]")
    tax[k2] += 1; ex[k2].append(n)
tot = sum(tax.values())
print(f"\n== WIDE miss taxonomy (misses={tot})")
for k, v in tax.most_common(): print(f"  {v:5d}  {k}")
json.dump({k: v[:400] for k, v in ex.items()}, open(Path(__file__).resolve().parent / "miss_examples.json", "w"))
um = collections.Counter(todo[n]["move"] for k, v in ex.items() if "unexplained" in k or "zero" in k for n in v)
print("unexplained/zero by move:", um.most_common(15))
