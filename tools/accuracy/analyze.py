#!/usr/bin/env python3
"""Taxonomy + offender tables for the accuracy-proof experiments.

Usage:
    python3 analyze.py keyed <results.jsonl> <battles_dir> [--examples N]
    python3 analyze.py psrng <results.jsonl> <battles_dir>

Reads the per-battle records written by
`cargo run -p vgc-engine-conformance --bin accuracy -- <mode> ... --jsonl` and
the PS battle files (for the protocol log), and prints:

* the headline: battles fully clean, with a Wilson 95% CI;
* turn-level agreement (turns matched before the first divergence);
* the first-divergence taxonomy (see docs/accuracy/2026-09-accuracy-proof.md);
* offenders: the effect (move / item / ability / condition) that last touched
  the diverged slot on the divergence turn, ranked by count and by
  divergence rate per use.
"""
import json
import math
import os
import re
import sys
from collections import Counter, defaultdict


def wilson(k, n, z=1.96):
    if n == 0:
        return (0.0, 0.0)
    p = k / n
    d = 1 + z * z / n
    c = p + z * z / (2 * n)
    m = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return ((c - m) / d, (c + m) / d)


def to_id(s):
    return re.sub(r'[^a-z0-9]', '', s.lower())


DECISION_MODEL = {'ejectbutton', 'ejectpack', 'redcard', 'emergencyexit', 'wimpout', 'revivalblessing'}


def turn_lines(log, turn):
    out, cur = [], 0
    for line in log.split('\n'):
        if line.startswith('|turn|'):
            cur = int(line[6:])
            continue
        if cur == turn and line.startswith('|') and not line.startswith('|t:|') and line != '|':
            out.append(line)
    return out


def sources_for(lines, slot):
    """Effects that changed `slot` this turn: [from] tags, else the move."""
    src = []
    last_move = None
    prefix = slot + ':'
    for line in lines:
        parts = line.split('|')
        cmd = parts[1] if len(parts) > 1 else ''
        if cmd == 'move':
            last_move = to_id(parts[3]) if len(parts) > 3 else None
            continue
        if cmd in ('-damage', '-heal', '-boost', '-unboost', '-status', '-sethp', '-setboost',
                   '-clearboost', '-clearnegativeboost', '-curestatus', '-enditem', '-item',
                   'switch', 'drag', 'detailschange', '-formechange', '-transform') and len(parts) > 2 \
                and parts[2].startswith(prefix):
            frm = [p for p in parts if p.startswith('[from]')]
            if frm:
                f = frm[0][6:].strip()
                f = re.sub(r'^(item|ability|move): ?', '', f)
                src.append(to_id(f))
            elif cmd in ('switch', 'drag'):
                src.append('switch')
            elif cmd == '-enditem':
                src.append(to_id(parts[3]) if len(parts) > 3 else 'item')
            elif last_move:
                src.append(last_move)
    return src


def all_sources(lines):
    src = []
    last_move = None
    for line in lines:
        parts = line.split('|')
        cmd = parts[1] if len(parts) > 1 else ''
        if cmd == 'move':
            last_move = to_id(parts[3]) if len(parts) > 3 else None
            src.append(last_move)
        for p in parts:
            if p.startswith('[from]'):
                src.append(to_id(re.sub(r'^(item|ability|move): ?', '', p[6:].strip())))
    return src


def main():
    mode, res_path, bdir = sys.argv[1], sys.argv[2], sys.argv[3]
    n_ex = int(sys.argv[sys.argv.index('--examples') + 1]) if '--examples' in sys.argv else 3
    recs = [json.loads(l) for l in open(res_path)]
    n = len(recs)
    errs = [r for r in recs if r.get('engine_error')]
    ok = [r for r in recs if not r.get('engine_error')]
    clean = [r for r in ok if not r['divergence']]
    lo, hi = wilson(len(clean), len(ok))
    tm = sum(r['matched_turns'] for r in ok)
    tc = sum(r['turns_compared'] for r in ok)
    print(f'== {mode}: {len(ok)} battles replayed ({len(errs)} engine errors excluded)')
    print(f'fully clean (every compared turn matches, to the end): {len(clean)} = {100*len(clean)/max(1,len(ok)):.1f}%  '
          f'[95% CI {100*lo:.1f}-{100*hi:.1f}%]')
    tl, th = wilson(tm, tm + (len(ok) - len(clean)))
    print(f'turn-level: {tm} turns matched before first divergence; per-turn agreement given an agreeing start '
          f'= {tm}/{tm + len(ok) - len(clean)} = {100*tm/max(1, tm + len(ok) - len(clean)):.1f}% [95% CI {100*tl:.1f}-{100*th:.1f}%]')
    print(f'compared turns (incl. post-divergence cascade not counted): {tc}')
    ended = sum(1 for r in clean if r.get('ended_naturally'))
    print(f'clean battles that ran to a natural end: {ended}')

    # --- taxonomy ------------------------------------------------------------
    cats = Counter()
    by_cat = defaultdict(list)
    base_use = Counter()
    div_src = Counter()
    field_c = Counter()
    for r in ok:
        d = r['divergence']
        path = os.path.join(bdir, f"out_{r['id']}.json")
        try:
            battle = json.load(open(path))
        except Exception:
            battle = None
        log = battle['_meta']['log'] if battle else ''
        upto = d['turn'] if d else 10 ** 9
        for t in range(1, min(upto, 60) + 1):
            tl_ = turn_lines(log, t)
            if not tl_:
                continue
            for s in set(all_sources(tl_)):
                base_use[s] += 1
        if not d:
            continue
        field_c[d['field']] += 1
        lines = turn_lines(log, d['turn'])
        effects = set(all_sources(lines)) | set(to_id(x) for x in (r.get('div_context') or {}).get('effects', []))
        if mode == 'keyed' and r.get('rng_sensitive'):
            cat = 'a_rng_plumbing'
        elif mode == 'psrng':
            fd = r.get('first_draw_div')
            if fd and fd['turn'] <= d['turn']:
                cat = 'a_draw_order'
            else:
                cat = 'b_mechanics'
        else:
            cat = 'b_mechanics'
        if cat == 'b_mechanics':
            if effects & DECISION_MODEL:
                cat = 'c_decision_model'
            elif 'illusion' in effects or any('zoroark' in l.lower() for l in lines):
                cat = 'c_hidden_info'
        cats[cat] += 1
        slot = d['slot']
        srcs = sources_for(lines, slot) if slot.startswith('p') and len(slot) == 3 else []
        if not srcs:
            srcs = ['<field:' + d['field'] + '>'] if slot in ('field', 'p1', 'p2') else ['<none>']
        rec = dict(id=r['id'], turn=d['turn'], slot=slot, field=d['field'], engine=d['engine'], ps=d['ps'],
                   sources=srcs, species=r.get('div_species'))
        by_cat[cat].append(rec)
        if cat == 'b_mechanics':
            for s in set(srcs):
                div_src[s] += 1
    tot = sum(cats.values())
    print(f'\nfirst-divergence taxonomy ({tot} diverged battles):')
    labels = {
        'a_rng_plumbing': '(a) RNG plumbing — divergence changes with the stream for draws PS never supplied',
        'a_draw_order': '(a) draw order — first differing PRNG call precedes the state divergence',
        'b_mechanics': '(b) mechanics — deterministic given PS outcomes / aligned draws',
        'c_decision_model': '(c) decision model — Eject Button/Pack, Red Card, Emergency Exit (engine auto-picks)',
        'c_hidden_info': '(c) hidden info — Illusion',
    }
    for c, k in sorted(cats.items()):
        print(f'  {k:5d}  {100*k/max(1,tot):5.1f}%  {labels.get(c, c)}')
    print('\ndivergence field (all diverged):', dict(field_c.most_common()))

    print('\ntop offenders among (b): effect that last touched the diverged slot')
    print(f"{'battles':>8} {'uses':>6} {'rate':>6}  effect")
    rows = sorted(div_src.items(), key=lambda kv: -kv[1])[:30]
    for s, k in rows:
        u = base_use.get(s, 0)
        print(f'{k:8d} {u:6d} {100*k/max(1,u):5.1f}%  {s}')
    print('\nhighest divergence rate per use (>= 8 uses, >= 3 battles):')
    rate_rows = [(s, k, base_use.get(s, 0)) for s, k in div_src.items() if base_use.get(s, 0) >= 8 and k >= 3]
    rate_rows.sort(key=lambda x: -x[1] / x[2])
    for s, k, u in rate_rows[:20]:
        print(f'{k:8d} {u:6d} {100*k/u:5.1f}%  {s}')

    for c in sorted(by_cat):
        print(f'\nexamples {c}:')
        for e in by_cat[c][:n_ex]:
            print('  ', json.dumps(e))


if __name__ == '__main__':
    main()
