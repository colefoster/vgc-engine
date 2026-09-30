#!/usr/bin/env python3
"""TOP test: exact-integer fit against any combination of each species' common
Reg M-B sets (spreads up to 75% cumulative usage, cap 8; items/abilities known or
top-2 by usage). This is the "range across common sets" a calculator could show.

    WORKAROUND=1 python3 eval_top.py
"""
import collections, json, os, sys
from pathlib import Path
H = Path(__file__).resolve().parent
sys.path.insert(0, str(H))
import eval_damage as E
from extract import slug

SRC = os.environ.get("EVAL", "damage_eval_wa.json")
D = json.load(open(H / SRC))
todo = D["todo"]


def top_spreads(species, cum=75.0, cap=5):
    k = E.base_stats_key(species)
    if not k: return None
    out, tot = [], 0.0
    for s in E.STATS[k]["spreads"]:
        out.append(s); tot += s["pct"]
        if tot >= cum or len(out) >= cap: break
    return out


def opts(m, field, n=2):
    if field == "items":
        if m["item_known"] or "-Mega" in m["species"]:
            return [None if "-Mega" in m["species"] else m["item"]]
    else:
        if m["ability_known"] and m["ability"]: return [m["ability"]]
        if "-Mega" in m["species"]: return [None]
    k = E.base_stats_key(m["species"])
    v = [x for x, _ in E.STATS[k][field][:n]] if k else []
    return v or [None]


qs, index = [], []
for n, h in enumerate(todo):
    a, d = h["atk"], h["def"]
    sa, sd = top_spreads(a["species"]), top_spreads(d["species"])
    if not sa or not sd: continue
    base = {"move": h["move"], "weather": h["weather"], "terrain": h["terrain"], "spread": h["spread"], "crit": h["crit"],
            **E.field_extras(h), **E.ally_abilities(h, "tight", None)}
    start = len(qs)
    for x in sa:
        for y in sd:
            for ai in opts(a, "items"):
                for di in opts(d, "items"):
                    if di and di.endswith("Berry") and not d["item_known"]: di = None
                    for aa in opts(a, "abilities"):
                        for da in opts(d, "abilities"):
                            qs.append(dict(base,
                                atk=dict(E.mon(a["species"], ai, aa, slug(a["nature"] or x["nature"]), E.parse_sp(x["stat_points"]), a["boosts"], a["status"]), hp_pct=E.hp_pct(a["hp"])),
                                **{"def": E.mon(d["species"], di, E.fix_def_ability(da, h["prev"]), slug(d["nature"] or y["nature"]),
                                                E.parse_sp(y["stat_points"]), d["boosts"], d["status"])}))
    index.append((n, start, len(qs)))
print("queries", len(qs), file=sys.stderr)
res = []
for i in range(0, len(qs), 400000):  # chunked: one 9M-line pipe thrashed memory
    res.extend({"hp": r.get("hp"), "rolls": r.get("rolls")} if "err" not in r else r for r in E.run_queries(qs[i:i + 400000]))
    qs[i:i + 400000] = [None] * len(qs[i:i + 400000])
fit = {}
for n, s, e in index:
    h = todo[n]
    ok = False; lo, hi = 9, -1
    for r in res[s:e]:
        if "err" in r: continue
        lo = min(lo, min(r["rolls"]) / r["hp"]); hi = max(hi, max(r["rolls"]) / r["hp"])
        if not ok and E.fits_exact(r["rolls"], r["hp"], h["prev"], h["new"]): ok = True
    fit[n] = (ok, hi - lo)
json.dump({str(k): v for k, v in fit.items()}, open(H / "top_eval.json", "w"))
import statistics
for name, sel in (("ALL", lambda h: True), ("OTS", lambda h: h["ots"]), ("spectator", lambda h: not h["ots"])):
    ns = [n for n in fit if sel(todo[n])]
    f = sum(fit[n][0] for n in ns)
    w = [100 * fit[n][1] for n in ns]
    print(f"{name}: {f}/{len(ns)} = {100*f/max(1,len(ns)):.1f}%  width median {statistics.median(w):.1f} pp", file=sys.stderr)
