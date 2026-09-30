#!/usr/bin/env python3
"""Damage gate: compare observed ladder hits with vgc-engine's predicted range.

    python3 eval_damage.py recs.jsonl > damage_report.json

Tests (see README.md for the envelope definitions):
  WIDE   documented plausible envelope (SP 0-32, natures, candidate items/abilities)
  EXT    WIDE + every damage item / legal ability / hindering natures (hidden-info probe)
  TIGHT  single most-common spread + item + ability per species (Reg M-B Smogon stats)
"""
import collections, json, math, os, re, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUNNER = HERE / "runner/target/release/bf-gate-runner"
DEX = Path.home() / "Dev/localdex/data"
MOVES = json.load(open(DEX / "moves.json"))
POKEDEX = json.load(open(DEX / "pokedex.json"))
SMOGON = json.load(open(Path.home() / "Dev/vgc-winrates/results/smogon_gen9championsvgc2026regmb_2026-08.json"))
STATS = {re.sub(r"[^a-z0-9]", "", k.lower()): v for k, v in SMOGON["cutoffs"]["0"]["species"].items()}
sys.path.insert(0, str(HERE))
from extract import EXCLUDE_MOVES, DMG_VOLATILES, slug  # noqa

EXCLUDE_MOVES |= {"foulplay"}  # uses the target's Attack: breaks the monotone envelope
SPECIAL_DEF_PHYS = {"psyshock", "psystrike", "secretsword"}
PINCH = {"blaze", "torrent", "overgrow", "swarm"}
FULL_HP_DEF = {"multiscale", "shadowshield", "terashell"}
ALLY_ATK_MOD = {"powerspot", "battery", "steelyspirit"}
TYPE_ITEM = {"Normal": "Silk Scarf", "Fire": "Charcoal", "Water": "Mystic Water", "Grass": "Miracle Seed",
             "Electric": "Magnet", "Ice": "Never-Melt Ice", "Fighting": "Black Belt", "Poison": "Poison Barb",
             "Ground": "Soft Sand", "Flying": "Sharp Beak", "Psychic": "Twisted Spoon", "Bug": "Silver Powder",
             "Rock": "Hard Stone", "Ghost": "Spell Tag", "Dragon": "Dragon Fang", "Dark": "Black Glasses",
             "Steel": "Metal Coat", "Fairy": "Fairy Feather"}
# Natures that move exactly one relevant stat (the minus/plus lands on Speed or an unused stat).
PLUS = {"atk": "brave", "spa": "quiet", "def": "relaxed", "spd": "sassy"}
MINUS = {"atk": "mild", "spa": "lonely", "def": "hasty", "spd": "naive"}  # Mild: +spa -def ... see note
# For attacker "-" we need a nature lowering the offensive stat without touching the
# defender-relevant stats: Timid (-atk), Jolly (-spa), and for def Hasty(-def), spd Naive(-spd).
MINUS = {"atk": "timid", "spa": "jolly", "def": "hasty", "spd": "naive"}
NATURES = {  # plus, minus
    "hardy": (None, None), "lonely": ("atk", "def"), "brave": ("atk", "spe"), "adamant": ("atk", "spa"),
    "naughty": ("atk", "spd"), "bold": ("def", "atk"), "docile": (None, None), "relaxed": ("def", "spe"),
    "impish": ("def", "spa"), "lax": ("def", "spd"), "timid": ("spe", "atk"), "hasty": ("spe", "def"),
    "serious": (None, None), "jolly": ("spe", "spa"), "naive": ("spe", "spd"), "modest": ("spa", "atk"),
    "mild": ("spa", "def"), "quiet": ("spa", "spe"), "bashful": (None, None), "rash": ("spa", "spd"),
    "calm": ("spd", "atk"), "gentle": ("spd", "def"), "sassy": ("spd", "spe"), "careful": ("spd", "spa"),
    "quirky": (None, None)}
SPI = {"hp": 0, "atk": 1, "def": 2, "spa": 3, "spd": 4, "spe": 5}
DMG_ITEMS_ATK = ["Choice Band", "Choice Specs", "Life Orb", "Expert Belt", "Muscle Band", "Wise Glasses"]


def hp_interval(tok):
    """Champions spectator HP: floor(100*hp/max) || 1, with y/r/g suffixes at 20/50."""
    if tok == "0": return (0.0, 0.0)
    if tok == "100": return (1.0, 1.0)
    if tok == "20r": return (0.20, 0.20)
    if tok == "50y": return (0.50, 0.50)
    if tok == "20y": return (0.20 + 1e-9, 0.21 - 1e-9)
    if tok == "50g": return (0.50 + 1e-9, 0.51 - 1e-9)
    p = int(re.sub(r"[a-z]+$", "", tok))
    if p == 1: return (1e-9, 0.02 - 1e-9)
    return (p / 100, (p + 1) / 100 - 1e-9)


def base_stats_key(species):
    s = slug(species)
    d = POKEDEX.get(s)
    if d is None: return None
    if s in STATS: return s
    b = d.get("baseSpecies")
    if b and slug(b) in STATS: return slug(b)
    return None


PSDEX = json.load(open(HERE / "ps_champions_dex.json"))


def legal_abilities(species):
    d = PSDEX["species"].get(slug(species)) or {}
    return sorted(set(d.get("abilities") or []))


def stats_items(species, min_pct=3.0, cap=5):
    k = base_stats_key(species)
    if not k: return None
    return [i for i, p in STATS[k]["items"] if p >= min_pct][:cap]


def stats_top(species, field):
    k = base_stats_key(species)
    if not k: return None
    v = STATS[k][field]
    return v[0] if v else None


def parse_sp(s):
    return [int(x) for x in s.split("/")]


def eligible(h):
    mv = PSDEX["moves"].get(slug(h["move"]))
    if not mv: return "unknown_move"
    if mv["category"] == "Status" or not mv.get("basePower"): return "no_bp"
    if slug(h["move"]) in EXCLUDE_MOVES: return "excluded_move"
    if h["multihit"] or mv.get("multihit"): return "multihit"
    if h["from"]: return "called_move"
    if h["friendly"]: return "ally_target"
    if h["censored"]: return "sash_sturdy"
    if h["prev"] == "0": return "parse"
    a, d = h["atk"], h["def"]
    if a["transformed"] or d["transformed"] or a["boost_uncertain"] or d["boost_uncertain"]: return "state_uncertain"
    if set(a["vol"]) & DMG_VOLATILES or set(d["vol"]) & DMG_VOLATILES: return "volatile"
    if slug(a["species"]) not in POKEDEX or slug(d["species"]) not in POKEDEX: return "unknown_species"
    if h["helped"]: return "api_gap_helping_hand"
    if h["screens"] and not h["crit"]: return "api_gap_screens"
    if a["hp"] != "100" and int(re.sub(r"[a-z]+$", "", a["hp"])) <= 34 and \
            (slug(a["ability"] or "") in PINCH or (not a["ability_known"] and any(slug(x) in PINCH for x in legal_abilities(a["species"])))):
        return "api_gap_pinch_ability"
    return None


def stat_roles(move):
    mv = PSDEX["moves"][slug(move)]
    phys = mv["category"] == "Physical"
    off = "def" if slug(move) == "bodypress" else ("atk" if phys else "spa")
    dfn = "def" if (phys or slug(move) in SPECIAL_DEF_PHYS) else "spd"
    return off, dfn, mv["type"]


def mon(species, item, ability, nature, sp, boosts, status):
    return {"species": species, "item": item, "ability": ability, "nature": nature, "sp": sp,
            "boosts": boosts, "status": status}


def fix_def_ability(ab, prev_tok):
    if ab and slug(ab) in FULL_HP_DEF and prev_tok != "100":
        return "No Ability"
    return ab


def candidates(h, mode):
    """Yield (atk_item, atk_ability, def_item, def_ability) combos for a mode."""
    a, d = h["atk"], h["def"]
    off, dfn, mtype = stat_roles(h["move"])
    mega_a = "-Mega" in a["species"]
    mega_d = "-Mega" in d["species"]

    def atk_items():
        if a["item_known"] or mega_a: return [None if mega_a else a["item"]]
        if mode == "tight":
            t = stats_top(a["species"], "items"); return [t[0] if t else None]
        base = stats_items(a["species"])
        if base is None:
            base = ["Choice Band" if off == "atk" else "Choice Specs", TYPE_ITEM.get(mtype), "Expert Belt"]
        s = {None, *base}
        if mode == "ext": s |= set(DMG_ITEMS_ATK) | {TYPE_ITEM.get(mtype)}
        return sorted(s, key=lambda x: x or "")

    def def_items():
        if d["item_known"] or mega_d: return [None if mega_d else d["item"]]
        if mode == "tight":
            t = stats_top(d["species"], "items"); it = t[0] if t else None
            # An unrevealed resist berry didn't fire, so drop it.
            return [None if (it and it.endswith("Berry")) else it]
        base = stats_items(d["species"]) or []
        s = {None} | {i for i in base if not i.endswith("Berry")}
        if mode == "ext": s |= {"Assault Vest", "Eviolite"}
        return sorted(s, key=lambda x: x or "")

    def abil(m, role):
        if m["ability_known"] and m["ability"]:
            return [m["ability"]]
        if "-Mega" in m["species"]:
            return [None]
        if mode == "tight":
            t = stats_top(m["species"], "abilities")
            return [t[0] if t else None]
        return legal_abilities(m["species"]) or [None]

    for ai in atk_items():
        for aa in abil(a, "atk"):
            for di in def_items():
                for da in abil(d, "def"):
                    yield ai, aa, di, fix_def_ability(da, h["prev"])


def nat_options(known, stat, role, mode):
    if known: return [slug(known)]
    if role == "atk":
        return ["serious", PLUS[stat]] + ([MINUS[stat]] if mode == "ext" else [])
    return [MINUS[stat], "serious", PLUS[stat]]


def build_queries(h, mode):
    """Return list of (tag, query). WIDE/EXT: two corner queries per combo; TIGHT: one."""
    a, d = h["atk"], h["def"]
    off, dfn, _ = stat_roles(h["move"])
    base = {"move": h["move"], "weather": h["weather"], "terrain": h["terrain"], "spread": h["spread"],
            "crit": h["crit"]}
    qs = []
    for ai, aa, di, da in candidates(h, mode):
        if mode == "tight":
            ta = stats_top(a["species"], "spreads"); td = stats_top(d["species"], "spreads")
            if not ta or not td: return None
            an = slug(a["nature"]) if a["nature"] else slug(ta["nature"])
            dn = slug(d["nature"]) if d["nature"] else slug(td["nature"])
            q = dict(base, atk=mon(a["species"], ai, aa, an, parse_sp(ta["stat_points"]), a["boosts"], a["status"]),
                     def_=None)
            q["def"] = mon(d["species"], di, da, dn, parse_sp(td["stat_points"]), d["boosts"], d["status"])
            q.pop("def_")
            qs.append(("t", q))
            continue
        ans = nat_options(a["nature"], off, "atk", mode)
        dns = nat_options(d["nature"], dfn, "def", mode)

        def nat_rank(n, stat):
            p, m = NATURES[n]
            return (1 if p == stat else 0) - (1 if m == stat else 0)
        a_lo = min(ans, key=lambda n: nat_rank(n, off)); a_hi = max(ans, key=lambda n: nat_rank(n, off))
        d_lo = min(dns, key=lambda n: nat_rank(n, dfn)); d_hi = max(dns, key=lambda n: nat_rank(n, dfn))
        sp_a_lo = [0] * 6; sp_a_hi = [0] * 6; sp_a_hi[SPI[off]] = 32
        sp_d_bulk = [0] * 6; sp_d_bulk[0] = 32; sp_d_bulk[SPI[dfn]] = 32
        if off == dfn == "def":
            pass
        lo = dict(base, atk=mon(a["species"], ai, aa, a_lo, sp_a_lo, a["boosts"], a["status"]),
                  **{"def": mon(d["species"], di, da, d_hi, sp_d_bulk, d["boosts"], d["status"])})
        hi = dict(base, atk=mon(a["species"], ai, aa, a_hi, sp_a_hi, a["boosts"], a["status"]),
                  **{"def": mon(d["species"], di, da, d_lo, [0] * 6, d["boosts"], d["status"])})
        qs.append(("lo", lo)); qs.append(("hi", hi))
    return qs


def fits_exact(rolls, M, prev_tok, new_tok):
    plo, phi = hp_interval(prev_tok); nlo, nhi = hp_interval(new_tok)
    hmin = max(1, math.ceil(plo * M - 1e-6)); hmax = min(M, math.floor(phi * M + 1e-6))
    for hp in range(hmin, hmax + 1):
        for r in set(rolls):
            left = max(0, hp - r)
            f = left / M
            if new_tok == "0":
                if left == 0: return True
            elif left > 0 and nlo - 1e-9 <= f <= nhi + 1e-9 and fmt_ok(left, M, new_tok):
                return True
    return False


def fmt_ok(hp, M, tok):
    """Exact Champions display check."""
    pct = (100 * hp) // M or 1
    s = str(pct)
    if pct == 20: s += "y" if hp * 5 > M else "r"
    elif pct == 50: s += "g" if hp * 2 > M else "y"
    if tok in ("20", "50"):  # older logs without suffix
        return str(pct) == tok
    return s == tok if tok[-1].isalpha() else str(pct) == tok


def fits_envelope(lo_res, hi_res, prev_tok, new_tok):
    """Observed damage fraction interval vs [min%, max%] across the envelope corners."""
    plo, phi = hp_interval(prev_tok); nlo, nhi = hp_interval(new_tok)
    min_frac = min(lo_res["rolls"]) / lo_res["hp"]
    max_frac = max(hi_res["rolls"]) / hi_res["hp"]
    if new_tok == "0":
        return max_frac >= plo - 1e-9, min_frac, max_frac, (plo, 9.99)
    olo, ohi = plo - nhi, phi - nlo
    return (max_frac >= olo - 1e-9 and min_frac <= ohi + 1e-9), min_frac, max_frac, (olo, ohi)


WORKAROUND = os.environ.get("WORKAROUND") == "1"
SEEDS = {"grassyseed", "electricseed", "psychicseed", "mistyseed"}


def neutralise(q):
    """Workaround for the calc-API bug: damage_only runs battle-start effects, so a
    held Intimidate drops the foe's Attack (and fires Defiant/Competitive) and terrain
    seeds get eaten. Neither ability nor seed has any other damage role."""
    for s in ("atk", "def"):
        m = q[s]
        if m.get("ability") and slug(m["ability"]) == "intimidate":
            m["ability"] = "No Ability"
        if m.get("ability") is None and slug(m["species"]) in INTIM_PRIMARY:
            m["ability"] = "No Ability"
        if m.get("item") and slug(m["item"]) in SEEDS:
            m["item"] = None
    return q


INTIM_PRIMARY = {k for k, v in PSDEX["species"].items() if v["abilities"][:1] == ["Intimidate"]}


def run_queries(qs):
    if WORKAROUND:
        qs = [neutralise(json.loads(json.dumps(q))) for q in qs]
    inp = "\n".join(json.dumps(q) for q in qs) + "\n"
    out = subprocess.run([str(RUNNER)], input=inp, capture_output=True, text=True, check=True).stdout
    return [json.loads(l) for l in out.splitlines()]


def main():
    hits = [json.loads(l) for l in open(sys.argv[1]) if '"kind": "hit"' in l]
    elig = collections.Counter()
    todo = []
    for h in hits:
        why = eligible(h)
        elig[why or "ok"] += 1
        if why is None: todo.append(h)
    print(f"hits {len(hits)} eligibility {dict(elig)}", file=sys.stderr)

    results = {}
    for mode in ("wide", "ext", "tight"):
        allq, index = [], []
        for n, h in enumerate(todo):
            qs = build_queries(h, mode)
            if qs is None: continue
            start = len(allq)
            allq.extend(q for _, q in qs)
            index.append((n, start, [t for t, _ in qs]))
        print(f"{mode}: {len(allq)} queries", file=sys.stderr)
        res = run_queries(allq)
        per = {}
        for n, start, tags in index:
            h = todo[n]
            rs = res[start:start + len(tags)]
            if any("err" in r for r in rs):
                per[n] = {"err": next(r["err"] for r in rs if "err" in r)}; continue
            if mode == "tight":
                r = rs[0]
                per[n] = {"fit": fits_exact(r["rolls"], r["hp"], h["prev"], h["new"]),
                          "min": min(r["rolls"]) / r["hp"], "max": max(r["rolls"]) / r["hp"],
                          "zero": max(r["rolls"]) == 0}
                continue
            fit, gmin, gmax, obs = False, 9, -1, None
            for k in range(0, len(tags), 2):
                f, mn, mx, obs = fits_envelope(rs[k], rs[k + 1], h["prev"], h["new"])
                fit |= f; gmin = min(gmin, mn); gmax = max(gmax, mx)
            per[n] = {"fit": fit, "min": gmin, "max": gmax, "obs": obs, "zero": gmax == 0}
        results[mode] = per
    out = HERE / ("damage_eval_wa.json" if WORKAROUND else "damage_eval.json")
    json.dump({"todo": todo, "results": {m: {str(k): v for k, v in r.items()} for m, r in results.items()},
               "eligibility": dict(elig)}, open(out, "w"))
    print("wrote damage_eval.json", file=sys.stderr)


if __name__ == "__main__":
    main()
