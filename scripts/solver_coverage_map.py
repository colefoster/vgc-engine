#!/usr/bin/env python3
"""Aggregate `solver_harness` output into the tables of docs/solver_coverage_map.md.

  python3 scripts/solver_coverage_map.py --brute 'brute_*.jsonl' --real 'real_*.jsonl' \
      [--positions real_positions.jsonl] > tables.md

Single pass over each file with running counters (bounded memory; the
harness inputs carry ~20 KB Battle JSON per line). `--positions` (the
harness input) adds belief-build failures, which never reach the solver,
and SP `actual` actions for principal-variation scoring.
"""
from __future__ import annotations

import argparse
import collections
import glob
import json
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TOL = 1e-6
OVER_MS = 50.0  # latency slack over the budget that still counts as "on time"
CONFIGS = ("exact_hp", "default", "factored", "lossy3", "served_anytime")


def implemented(kind: str) -> set[str]:
    """Slugs the core references by `<kind>_id::NAME` (docs/gaps/README.md)."""
    pat = re.compile(kind + r"_id::([A-Z_0-9]+)")
    out = set()
    for dp, _, fs in os.walk(os.path.join(ROOT, "crates", "vgc-engine-core", "src")):
        for f in fs:
            if f.endswith(".rs"):
                with open(os.path.join(dp, f)) as fh:
                    out.update(m.replace("_", "").lower() for m in pat.findall(fh.read()))
    return out


def stream(pattern: str):
    for f in sorted(glob.glob(pattern)):
        with open(f) as fh:
            for line in fh:
                if line.strip():
                    yield json.loads(line)


def pct(a: int, b: int) -> str:
    return f"{a}/{b} ({100 * a / b:.1f}%)" if b else "0/0"


def build_error_label(err: str) -> str:
    if re.search(r"opp: \d+ mons \(max 6\)", err):
        return "SP read: phantom opp mons (>6 species read)"
    m = re.search(r"unsupported: volatile ([A-Z_]+)", err)
    if m:
        return f"belief build: volatile {m.group(1).lower()} unsupported"
    if "illusion" in err:
        return "belief build: Illusion ambiguous"
    if "no mon left" in err:
        return "belief build: side with no mon left (decision after game end)"
    if "active mon is not in the team" in err:
        return "SP read: active mon not in team"
    return "belief build: " + err.split(":")[-1].strip()[:60]


class Brute:
    def __init__(self):
        self.n = 0
        self.skips = collections.Counter()
        self.shapes = collections.Counter()
        self.depths = collections.Counter()
        z = lambda: {"n": 0, "agree": 0, "nt": 0, "agree_t": 0, "expl": 0, "panic": 0, "max": 0.0, "sum": 0.0, "worst": []}
        self.cfg = {c: z() for c in CONFIGS}
        self.mc = [0, 0, 0, 0.0]  # cells, samples, missing, tv max
        self.mc_bad = []

    def add(self, r):
        if "skip" in r:
            self.skips[r["skip"]] += 1
            return
        self.n += 1
        self.shapes[r["desc"].split(":")[0]] += 1
        self.depths[r["depth"]] += 1
        for c in CONFIGS:
            v, a = r["solvers"].get(c, {}), self.cfg[c]
            if "panic" in v:
                a["panic"] += 1
            if "dv" not in v:
                continue
            d = abs(v["dv"])
            a["n"] += 1
            a["agree"] += d <= TOL
            a["max"] = max(a["max"], d)
            a["sum"] += d
            a["expl"] += max(v["expl_row"], v["expl_col"]) > TOL
            if r["ref_all_terminal"]:
                a["nt"] += 1
                a["agree_t"] += d <= TOL
            a["worst"] = sorted(a["worst"] + [(d, r["i"], r["desc"].split(":")[0], r["ref_all_terminal"])])[-3:]
        self.mc[0] += r["mc_cells"]
        self.mc[1] += r["mc_samples"]
        self.mc[2] += r["mc_missing"]
        self.mc[3] = max(self.mc[3], r["mc_tv_max"])
        if len(self.mc_bad) < 10:
            self.mc_bad += [(r["i"], r["desc"][:40], b) for b in r["mc_bad"]][: 10 - len(self.mc_bad)]

    def render(self) -> list[str]:
        out = ["## Brute-force agreement (generated small endgames)", "",
               f"{self.n} positions compared ({', '.join(f'{k} {v}' for k, v in sorted(self.shapes.items()))}); "
               f"{sum(self.skips.values())} skipped ({'; '.join(f'{k}: {v}' for k, v in self.skips.items())}). "
               "Reference depth: " + ", ".join(f"d{d} {n}" for d, n in sorted(self.depths.items())) + ".", "",
               "| config | value agrees (abs dv <= 1e-6) | agrees when ref all-terminal | max abs dv | mean abs dv | root policy exploitable > 1e-6 | panics |",
               "|---|---|---|---|---|---|---|"]
        for c in CONFIGS:
            a = self.cfg[c]
            out.append(f"| {c} | {pct(a['agree'], a['n'])} | {pct(a['agree_t'], a['nt'])} | {a['max']:.4f} | "
                       f"{a['sum'] / max(1, a['n']):.4f} | {pct(a['expl'], a['n'])} | {a['panic']} |")
        out += ["", f"Real-RNG sampling: {self.mc[0]} root cells, {self.mc[1]} seeded `step()` samples; "
                    f"{self.mc[2]} landed outside the enumerated frontier; max TV distance {self.mc[3]:.3f}."]
        out += [f"- MC mismatch: position {i} ({d}...): {b}" for i, d, b in self.mc_bad]
        for c in ("default", "served_anytime"):
            out.append(f"- worst `{c}`: " + "; ".join(f"#{i} {s} abs dv {d:.4f} (ref all-terminal={t})"
                                                    for d, i, s, t in reversed(self.cfg[c]["worst"])))
        return out + [""]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--brute", default="")
    ap.add_argument("--real", default="")
    ap.add_argument("--positions")
    a = ap.parse_args()
    lines: list[str] = []
    if a.brute:
        br = Brute()
        for r in stream(a.brute):
            br.add(r)
        lines += br.render()

    # Positions: keep only the small fields (never the Battle JSON).
    meta: dict[str, dict] = {}
    by_src = collections.Counter()
    gaps: dict[str, dict] = collections.defaultdict(lambda: {"n": 0, "sp": 0, "ex": None, "status": "", "fail": 0})

    def note(key, status, rid, src, failed=False):
        g = gaps[key]
        g["n"] += 1
        g["sp"] += src == "sp"
        g["status"] = status
        g["fail"] += failed
        g["ex"] = g["ex"] or rid

    n_pos = n_build_err = 0
    if a.positions:
        for p in stream(a.positions):
            n_pos += 1
            by_src[p.get("src") or "?"] += 1
            meta[p["id"]] = {"src": p.get("src"), "side": p.get("side", "p1"), "actual": p.get("actual"),
                             "n": (p.get("n_own"), p.get("n_opp"))}
            if "error" in p:
                n_build_err += 1
                note(build_error_label(p["error"]), "unsupported", p["id"], p.get("src"), True)

    imp = {k: implemented(k) for k in ("ability", "item")}
    acc = {c: collections.Counter() for c in ("served", "exact_d2")}
    over = {c: 0.0 for c in acc}
    mc = collections.Counter()
    mc_tv = 0.0
    pv = collections.Counter()
    n_solved = allpass = 0
    for r in stream(a.real) if a.real else ():
        if "served" not in r:
            continue
        n_solved += 1
        m = meta.get(r["id"], {})
        rid, src, labels, s = r["id"], m.get("src") or r.get("src"), r.get("labels", []), r["served"]
        for c in acc:
            x = r.get(c, {})
            if "panic" in x:
                acc[c]["panic"] += 1
                continue
            acc[c]["n"] += 1
            acc[c]["on_time"] += x["over_ms"] <= OVER_MS
            over[c] = max(over[c], x["over_ms"])
            acc[c]["d1"] += x["depth"] >= 1
            acc[c]["exact"] += x["prov"] in ("exact", "terminal")
            acc[c]["illegal"] += not x.get("legal_ok", True)
        x = r.get("mc", {})
        if "missing" in x:
            mc["cells"] += 1
            mc["bad"] += x["missing"] > 0
            mc["unmatched"] += x["unmatched"]
            mc_tv = max(mc_tv, x["tv"])
        elif "skip" in x:
            mc["skip"] += 1
        allpass += bool(r.get("viewer_all_pass"))
        # PV vs what happened (SP: next request's protocol log).
        if m.get("actual") and s.get("top_p1"):
            viewer = m["side"]
            opp = "p2" if viewer == "p1" else "p1"
            for side in (viewer, opp):
                tops = {}
                for seg in (s["top_p1" if side == "p1" else "top_p2"] or "").split(" | "):
                    mm = re.match(r"(\d):(?:mega\+|tera\+)?(switch [a-z0-9]+|[a-z0-9]+)", seg)
                    if mm:
                        tops[int(mm.group(1))] = mm.group(2).replace("switch ", "")
                for slot, kind, slug in m["actual"].get(side, []):
                    if slot in tops:
                        hit = tops[slot] == slug if kind == "move" else tops[slot].startswith(slug[:5])
                        k = "opp" if side == opp else "own"
                        pv[k + "_n"] += 1
                        pv[k + "_hit"] += hit
        # Coverage labels.
        note("hidden info: unrevealed opp sets filled from usage prior", "approximate", rid, src)
        if s.get("depth", 0) == 0:
            note("too slow: no depth-1 iteration within 1500 ms", "unsupported", rid, src, True)
        elif s.get("prov") not in ("exact", "terminal"):
            note("depth-limited: hp_ratio leaf at the search horizon", "approximate", rid, src)
        if s.get("dropped_max", 0) > 0.05:
            note("chance compression dropped > 5% of a cell's mass", "approximate", rid, src)
        if x.get("missing", 0) > 0:
            note("engine/frontier mismatch (real-RNG sample outside frontier)", "bug", rid, src, True)
        if r.get("viewer_all_pass"):
            note("decision point mismatch (viewer has no move at root)", "bug", rid, src, True)
        if "panic" in s or "panic" in r.get("exact_d2", {}):
            note("solver panic", "bug", rid, src, True)
        size = next((l.split(":")[1] for l in labels if l.startswith("size:")), None)
        if src == "sp" and size and m.get("n") and None not in m["n"]:
            own, opp_n = (int(t) for t in size.split("v"))
            if m["side"] == "p2":
                own, opp_n = opp_n, own
            if (own, opp_n) != tuple(m["n"]):
                note("belief battle alive count != stream count (fainted/unseen mon mismatch)", "bug", rid, src, True)
        for lab in labels:
            kind, _, name = lab.partition(":")
            if kind == "item" and re.search(r"ite[xyz]?$", name) and name not in ("eviolite",):
                continue  # mega stones: data-driven (MEGA_STONES), not item_id arms
            if kind in ("ability", "item") and name and name not in imp[kind]:
                note(f"{kind} not referenced by engine: {name}", "unsupported?", rid, src)
            elif kind in ("weather", "terrain", "field", "side", "state", "status", "mech", "class", "size", "format"):
                ok = s.get("depth", 0) >= 1
                note(f"situation {lab}", "supported" if ok else "slow", rid, src, not ok)

    lines += ["## Real positions", "",
              "Sources: " + ", ".join(f"{k} {v}" for k, v in sorted(by_src.items())) + ".",
              f"Belief battle could not be built: {pct(n_build_err, n_pos)}.", "",
              "| config | budget | finished <= budget+50 ms | max overrun ms | completed depth >= 1 | exact/terminal | panics | illegal policy actions |",
              "|---|---|---|---|---|---|---|---|"]
    for c, bud in (("served", "1500 ms anytime, compressed"), ("exact_d2", "2000 ms, max_depth 2, node_budget 10k")):
        k = acc[c]
        lines.append(f"| {c} | {bud} | {pct(k['on_time'], k['n'])} | {over[c]:.1f} | {pct(k['d1'], k['n'])} | "
                     f"{pct(k['exact'], k['n'])} | {k['panic']} | {k['illegal']} |")
    lines += ["", f"Engine agreement (served PV cell, 48 real-RNG `step()`s vs its lossless frontier): "
                  f"{pct(mc['bad'], mc['cells'])} cells had a sample outside the frontier; max TV {mc_tv:.3f}; "
                  f"unmatched draws {mc['unmatched']}; frontier > 20k combos (skipped) {mc['skip']}.",
              f"Root has no real move for the viewer (engine at another decision point): {pct(allpass, n_solved)}.",
              f"PV vs what happened (SP, served top joint, per slot): opponent move predicted {pct(pv['opp_hit'], pv['opp_n'])}; "
              f"solver top = move the bot played (ACT policy) {pct(pv['own_hit'], pv['own_n'])}.", "",
              "## Coverage map", "",
              f"Counts are positions (of {n_pos or n_solved}; SP = Switch games). `situation` rows: mechanic on the field; "
              "`failed` = no depth-1 iteration within budget.", "",
              "| situation | status | positions | SP | failed | example id |", "|---|---|---|---|---|---|"]
    for key, g in sorted(gaps.items(), key=lambda kv: -kv[1]["n"]):
        if key.startswith("situation ") and g["n"] < 5:
            continue
        lines.append(f"| {key} | {g['status']} | {g['n']} | {g['sp']} | {g['fail']} | `{g['ex']}` |")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
