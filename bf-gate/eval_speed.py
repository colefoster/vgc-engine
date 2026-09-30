#!/usr/bin/env python3
"""Speed-facts check: observed move order within one priority bracket vs the
engine's effective-speed ordering (calc::effective_speed via the runner).

    python3 eval_speed.py recs.jsonl

A pair is two consecutive move actions in one turn by different active mons at
equal move priority. PS (gen 8+) re-sorts the queue after every action, so the
earlier mover was at least as fast as the later one at the earlier move's
snapshot (reversed under Trick Room).
"""
import collections, json, sys
from pathlib import Path
H = Path(__file__).resolve().parent
sys.path.insert(0, str(H))
import eval_damage as E
from extract import slug

PS = E.PSDEX
SKIP_ABIL = {"prankster", "galewings", "triage", "stall", "myceliummight", "quickdraw", "unburden",
             "protosynthesis", "quarkdrive", "speedboost", "slowstart"}
SKIP_ITEMS = {"quickclaw", "custapberry", "laggingtail", "fullincense", "ironball"}
SKIP_MOVES = {"grassyglide", "pursuit", "focuspunch", "beakblast", "shelltrap"}
PLUS, MINUS = "timid", "brave"


def prio(move):
    m = PS["moves"].get(slug(move))
    return None if m is None else m["priority"]


def bad_mon(s):
    abil = [s["ability"]] if s["ability_known"] and s["ability"] else E.legal_abilities(s["species"])
    if any(slug(a or "") in SKIP_ABIL for a in abil): return "ability"
    if s["item"] and slug(s["item"]) in SKIP_ITEMS: return "item"
    if s["boost_uncertain"] or s["transformed"]: return "state"
    if any(v.startswith(("protosynthesis", "quarkdrive")) for v in s["vol"]): return "state"
    if slug(s["species"]) not in E.POKEDEX: return "species"
    return None


def configs(s, mode):
    """(item, ability, nature, sp_spe) candidates at the low and high ends."""
    if s["item_known"] or "-Mega" in s["species"]:
        items = [None if "-Mega" in s["species"] else s["item"]]
    elif mode == "tight":
        t = E.stats_top(s["species"], "items"); items = [t[0] if t else None]
    else:
        base = E.stats_items(s["species"]) or []
        items = [None] + (["Choice Scarf"] if "Choice Scarf" in base or not base else [])
    if s["ability_known"] and s["ability"]:
        abil = [s["ability"]]
    elif "-Mega" in s["species"]:
        abil = [(PS["species"].get(slug(s["species"])) or {}).get("abilities", [None])[0]]
    elif mode == "tight":
        t = E.stats_top(s["species"], "abilities"); abil = [t[0] if t else None]
    else:
        abil = E.legal_abilities(s["species"]) or [None]
    if mode == "tight":
        t = E.stats_top(s["species"], "spreads")
        if not t: return None
        nat = slug(s["nature"]) if s["nature"] else slug(t["nature"])
        sp = int(t["stat_points"].split("/")[5])
        return [(i, a, nat, sp) for i in items for a in abil], None
    nat_lo = slug(s["nature"]) if s["nature"] else MINUS
    nat_hi = slug(s["nature"]) if s["nature"] else PLUS
    return [(i, a, nat_lo, 0) for i in items for a in abil], [(i, a, nat_hi, 32) for i in items for a in abil]


def q(s, cfg, tailwind, weather):
    i, a, nat, sp = cfg
    return {"kind": "spe", "tailwind": tailwind, "weather": weather,
            "atk": {"species": s["species"], "item": i, "ability": a, "nature": nat, "sp": [0, 0, 0, 0, 0, sp],
                    "boosts": s["boosts"], "status": s["status"]}}


def main():
    orders = collections.defaultdict(list)
    for l in open(sys.argv[1]):
        if '"kind": "order"' not in l: continue
        r = json.loads(l)
        orders[(r["game"], r["turn"])].append(r)
    pairs, skip = [], collections.Counter()
    for (g, t), rs in orders.items():
        if t == 0: continue
        rs.sort(key=lambda r: r["line"])
        for a, b in zip(rs, rs[1:]):
            if a["actor"] == b["actor"]: skip["same_actor"] += 1; continue
            if a["from"] not in ("", "lockedmove") or b["from"] not in ("", "lockedmove"): skip["called"] += 1; continue
            pa, pb = prio(a["move"]), prio(b["move"])
            if pa is None or pb is None or pa != pb: skip["diff_priority"] += 1; continue
            if slug(a["move"]) in SKIP_MOVES or slug(b["move"]) in SKIP_MOVES: skip["move"] += 1; continue
            sa = next((m for m in a["actives"].values() if m and m["key"] == a["actor"]), None)
            sb = next((m for m in a["actives"].values() if m and m["key"] == b["actor"]), None)
            if sa is None or sb is None: skip["not_active"] += 1; continue
            if sb["hp"] == "0": skip["fainted"] += 1; continue
            why = bad_mon(sa) or bad_mon(sb)
            if why: skip["mon_" + why] += 1; continue
            # Status moves can be Prankster/Mycelium-shifted even when the ability is hidden.
            if PS["moves"][slug(a["move"])]["category"] == "Status" or PS["moves"][slug(b["move"])]["category"] == "Status":
                skip["status_move"] += 1; continue
            pairs.append({"a": sa, "b": sb, "tw_a": a["tailwind"][sa["key"][:2]], "tw_b": a["tailwind"][sb["key"][:2]],
                          "tr": a["trick_room"], "weather": a["weather"], "ots": a["ots"], "game": g, "turn": t,
                          "moves": (a["move"], b["move"])})
    print("pairs", len(pairs), "skipped", dict(skip), file=sys.stderr)

    out = {}
    for mode in ("wide", "tight"):
        qs, idx = [], []
        for n, p in enumerate(pairs):
            ca = configs(p["a"], mode); cb = configs(p["b"], mode)
            if ca is None or cb is None: continue
            ent = {}
            for who, c, tw in (("a", ca, p["tw_a"]), ("b", cb, p["tw_b"])):
                s = p[who]
                lo, hi = c
                ent[who] = (len(qs), len(lo), len(hi or []))
                qs.extend(q(s, x, tw, p["weather"]) for x in lo)
                qs.extend(q(s, x, tw, p["weather"]) for x in (hi or []))
            idx.append((n, ent))
        res = E.run_queries(qs)
        c = collections.Counter(); per = collections.defaultdict(collections.Counter); fails = []
        for n, ent in idx:
            p = pairs[n]
            rng = {}
            for who, (st, nlo, nhi) in ent.items():
                vals = [r.get("spe") for r in res[st:st + nlo + nhi]]
                if any(v is None for v in vals): rng = None; break
                rng[who] = (min(vals), max(vals))
            if rng is None: c["error"] += 1; continue
            (alo, ahi), (blo, bhi) = rng["a"], rng["b"]
            if p["tr"]:  # slower first: flip by negating
                alo, ahi, blo, bhi = -ahi, -alo, -bhi, -blo
            if alo > bhi: v = "agree"
            elif ahi < blo: v = "disagree"
            elif alo == ahi == blo == bhi: v = "tie"
            else: v = "undecided"
            c[v] += 1
            for k in ("ots", "tr"):
                per[f"{k}={p[k]}"][v] += 1
            if p["tw_a"] or p["tw_b"]: per["tailwind"][v] += 1
            if p["a"]["status"] == "par" or p["b"]["status"] == "par": per["paralysis"][v] += 1
            if p["a"]["boosts"][4] or p["b"]["boosts"][4]: per["spe_boost"][v] += 1
            if v == "disagree": fails.append({"pair": p, "rng": rng})
        out[mode] = {"counts": dict(c), "per": {k: dict(v) for k, v in per.items()}, "fails": fails[:300]}
        dec = c["agree"] + c["disagree"]
        print(f"{mode}: {dict(c)}  decisive {dec}/{sum(c.values())}  agreement {100*c['agree']/max(1,dec):.2f}%", file=sys.stderr)
        for k, v in per.items():
            d = v["agree"] + v["disagree"]
            print(f"   {k:14s} {dict(v)} agree {100*v['agree']/max(1,d):.1f}%", file=sys.stderr)
    json.dump(out, open(H / "speed_eval.json", "w"))


if __name__ == "__main__":
    main()
