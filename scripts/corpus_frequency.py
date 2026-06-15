#!/usr/bin/env python3
"""Walk the mimikyu replay corpus and emit move/ability/item/species
frequency tables to ``data/corpus_frequency.md``.

This is the Phase 2 work-order document. The top-N rows of each table that
cumulatively cover 95% of usage are the implementation queue.

Usage:
    python scripts/corpus_frequency.py
    python scripts/corpus_frequency.py --limit 1000           # quick smoke
    python scripts/corpus_frequency.py --corpus PATH --workers 8
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path
from typing import Optional, Tuple

DEFAULT_CORPUS = Path.home() / "Dev" / "mimikyu" / "data" / "replays"
OUT_FILE = Path(__file__).resolve().parent.parent / "data" / "corpus_frequency.md"

SLUG_RE = re.compile(r"[^a-z0-9]")


def slugify(name: str) -> str:
    return SLUG_RE.sub("", name.lower())


# PS protocol — see https://github.com/smogon/pokemon-showdown/blob/master/sim/SIM-PROTOCOL.md
RE_MOVE = re.compile(r"^\|move\|[^|]+\|([^|]+)\|")
RE_SWITCH = re.compile(r"^\|(?:switch|drag|detailschange)\|[^|]+\|([^,|]+)")
RE_ABILITY = re.compile(r"^\|-ability\|[^|]+\|([^|]+)")
RE_ITEM = re.compile(r"^\|-(?:item|enditem)\|[^|]+\|([^|]+)")
RE_FROM_ABILITY = re.compile(r"\[from\] ability: ([^|\[]+?)(?=\s*\||\s*\[|$)")
RE_FROM_ITEM = re.compile(r"\[from\] item: ([^|\[]+?)(?=\s*\||\s*\[|$)")


def process_log(log: str):
    moves: Counter = Counter()
    abilities: Counter = Counter()
    items: Counter = Counter()
    species: Counter = Counter()
    started = False
    for line in log.split("\n"):
        if not line.startswith("|"):
            continue
        if line.startswith("|start"):
            started = True
            continue
        if not started:
            continue
        m = RE_MOVE.match(line)
        if m:
            moves[slugify(m.group(1))] += 1
            for fm in RE_FROM_ABILITY.findall(line):
                abilities[slugify(fm)] += 1
            for fm in RE_FROM_ITEM.findall(line):
                items[slugify(fm)] += 1
            continue
        m = RE_SWITCH.match(line)
        if m:
            species[slugify(m.group(1))] += 1
            continue
        m = RE_ABILITY.match(line)
        if m:
            abilities[slugify(m.group(1))] += 1
            continue
        m = RE_ITEM.match(line)
        if m:
            items[slugify(m.group(1))] += 1
            continue
        # Inline annotations on damage/heal/boost/etc.
        for fm in RE_FROM_ABILITY.findall(line):
            abilities[slugify(fm)] += 1
        for fm in RE_FROM_ITEM.findall(line):
            items[slugify(fm)] += 1
    return moves, abilities, items, species


def process_file(path: str):
    try:
        with open(path, "rb") as fh:
            doc = json.load(fh)
    except Exception:
        return Counter(), Counter(), Counter(), Counter()
    log = doc.get("log") or ""
    if not log:
        return Counter(), Counter(), Counter(), Counter()
    return process_log(log)


def process_chunk(paths):
    m, a, i, s = Counter(), Counter(), Counter(), Counter()
    for p in paths:
        m2, a2, i2, s2 = process_file(p)
        m.update(m2)
        a.update(a2)
        i.update(i2)
        s.update(s2)
    return m, a, i, s


def find_files(root: Path, limit: Optional[int]):
    out = []
    for dp, _, files in os.walk(root):
        for f in files:
            if f.endswith(".json"):
                out.append(os.path.join(dp, f))
                if limit and len(out) >= limit:
                    return out
    return out


def chunked(seq, n):
    for i in range(0, len(seq), n):
        yield seq[i : i + n]


def render_table(counter: Counter) -> Tuple[str, int]:
    total = sum(counter.values())
    rows = counter.most_common()
    cum = 0
    lines = ["| slug | count | cumulative_pct |", "|---|---|---|"]
    p95_at = 0
    for i, (slug, c) in enumerate(rows, 1):
        cum += c
        pct = 100.0 * cum / total if total else 0.0
        lines.append(f"| {slug} | {c} | {pct:.2f}% |")
        if p95_at == 0 and pct >= 95.0:
            p95_at = i
    return "\n".join(lines), p95_at


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    ap.add_argument("--limit", type=int, default=None, help="Only scan first N files")
    ap.add_argument("--workers", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--out", type=Path, default=OUT_FILE)
    args = ap.parse_args()

    print(f"scanning {args.corpus} ...", file=sys.stderr, flush=True)
    files = find_files(args.corpus, args.limit)
    print(f"found {len(files):,} replay files", file=sys.stderr, flush=True)
    if not files:
        sys.exit("no replays found")

    moves, abilities, items, species = Counter(), Counter(), Counter(), Counter()
    chunk_size = max(50, len(files) // (args.workers * 16))
    chunks = list(chunked(files, chunk_size))
    print(f"dispatching {len(chunks)} chunks of ~{chunk_size} files across {args.workers} workers", file=sys.stderr, flush=True)
    done = 0
    with ProcessPoolExecutor(max_workers=args.workers) as ex:
        futs = [ex.submit(process_chunk, c) for c in chunks]
        for fut in as_completed(futs):
            m, a, i, s = fut.result()
            moves.update(m)
            abilities.update(a)
            items.update(i)
            species.update(s)
            done += 1
            if done % 25 == 0 or done == len(chunks):
                print(f"  {done}/{len(chunks)} chunks", file=sys.stderr, flush=True)

    moves_md, mv95 = render_table(moves)
    abilities_md, ab95 = render_table(abilities)
    items_md, it95 = render_table(items)
    species_md, sp95 = render_table(species)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        f"""# Corpus frequency

Generated by `scripts/corpus_frequency.py` from `{args.corpus}` ({len(files):,} replays).

Counts are per-occurrence within a turn that was actually played (post-`|start`,
ignoring team preview). Slugs are `[a-z0-9]`-normalized to match `@pkmn/dex`
IDs.

This document is the **Phase 2 work-order**: implementing the top-N rows of
each category in order is how we climb the corpus coverage curve fastest.

## 95% coverage cutoffs

- moves     : top {mv95} cover ≥95% of usage
- abilities : top {ab95} cover ≥95% of usage
- items     : top {it95} cover ≥95% of usage
- species   : top {sp95} cover ≥95% of usage

## Moves

{moves_md}

## Abilities

{abilities_md}

## Items

{items_md}

## Species

{species_md}
"""
    )
    print(f"wrote {args.out}", file=sys.stderr)
    print(
        f"top-95% cutoffs — moves:{mv95} abilities:{ab95} items:{it95} species:{sp95}"
    )


if __name__ == "__main__":
    main()
