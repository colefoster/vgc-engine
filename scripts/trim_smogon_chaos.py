#!/usr/bin/env python3
"""Trim a Smogon chaos JSON into the compact prior file `vgc_engine_replay::prior` embeds.

    python3 scripts/trim_smogon_chaos.py RAW_CHAOS.json DEX_DIR OUT.json --month 2026-09 [--fetched 2026-10-12]

RAW_CHAOS is https://www.smogon.com/stats/<month>/chaos/<format>-<cutoff>.json.
DEX_DIR holds Showdown's client dex (play.pokemonshowdown.com/data/{moves.json,items.js,abilities.js})
for display names. Stdlib only.

Same trim as Bring Four's `smogon.py` (which produced the 2026-08 Reg M-B file): species with
usage >= 0.1%, top 10 items / abilities / moves / spreads, percentages of the species' weighted
teams (abilities total), rounded to 2 dp; usage_pct to 3 dp. Teammates, checks, tera types and
happiness are dropped. Champions spreads are stat points (0-32 each, <= 66 total) and are kept
verbatim under `stat_points`.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

SENTINELS = {"", "nothing", "none", "null", "unknown", "undefined"}
TOP = 10
MIN_USAGE_PCT = 0.1


def load_names(dex: Path) -> dict[str, dict[str, str]]:
    out = {"moves": {k: v["name"] for k, v in json.loads((dex / "moves.json").read_text()).items() if "name" in v}}
    pat = re.compile(r'([a-z0-9]+):\{(?:[^{}"]|"[^"]*"|\{[^{}]*\})*?name:"([^"]+)"')
    for kind in ("items", "abilities"):
        out[kind] = dict(pat.findall((dex / f"{kind}.js").read_text()))
    return out


def dist(weights: dict, total: float, names: dict) -> list[list]:
    rows = [(names.get(k, k), w) for k, w in weights.items() if w > 0 and str(k).strip().lower() not in SENTINELS]
    rows.sort(key=lambda r: -r[1])
    return [[k, round(100 * w / total, 2)] for k, w in rows[:TOP]] if total > 0 else []


def spreads(weights: dict, total: float) -> list[dict]:
    out = []
    for key, w in sorted(weights.items(), key=lambda kv: -kv[1])[:TOP]:
        if ":" not in key or total <= 0:
            continue
        nature, sp = key.split(":", 1)
        out.append({"nature": nature, "stat_points": sp, "pct": round(100 * w / total, 2)})
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("raw")
    ap.add_argument("dex")
    ap.add_argument("out")
    ap.add_argument("--month", required=True)
    ap.add_argument("--fetched", default=None, help="YYYY-MM-DD the raw file was downloaded")
    a = ap.parse_args()
    chaos = json.loads(Path(a.raw).read_text())
    names = load_names(Path(a.dex))
    info = chaos["info"]
    species = {}
    for k, v in chaos["data"].items():
        if 100 * v["usage"] < MIN_USAGE_PCT:
            continue
        total = sum(v.get("Abilities", {}).values())
        species[k] = {
            "usage_pct": round(100 * v["usage"], 3),
            "items": dist(v.get("Items", {}), total, names["items"]),
            "abilities": dist(v.get("Abilities", {}), total, names["abilities"]),
            "moves": dist(v.get("Moves", {}), total, names["moves"]),
            "spreads": spreads(v.get("Spreads", {}), total),
        }
    species = dict(sorted(species.items(), key=lambda kv: -kv[1]["usage_pct"]))
    cutoff = info.get("cutoff")
    out = {
        "source": "Smogon usage statistics (smogon.com/stats), Pokémon Showdown ladder",
        "url": f"https://www.smogon.com/stats/{a.month}/chaos/{info['metagame']}-{cutoff}.json",
        "format": info["metagame"],
        "month": a.month,
        "cutoff": cutoff,
        **({"fetched": a.fetched} if a.fetched else {}),
        "battles_total": info.get("number of battles"),
        "units": "pct of the species' weighted teams (moves sum to ~400); spreads are Champions stat points",
        "trim": f"usage >= {MIN_USAGE_PCT}%, top {TOP} per category (scripts/trim_smogon_chaos.py)",
        "species": species,
    }
    Path(a.out).write_text(json.dumps(out, separators=(",", ":")))
    print(f"{info['metagame']} {a.month} cutoff {cutoff}: {len(species)}/{len(chaos['data'])} species kept")


if __name__ == "__main__":
    main()
