#!/usr/bin/env python3
"""Summarize a sim-diff.js run: identical full streams, and for the rest the
first divergence, split into

  formatting  every |request| and the end record match PS (the battle state
              never diverged): only log text / line order differs;
  draw        the state diverged and the first differing line is a random
              outcome (damage roll, crit, miss, secondary, speed-tie order,
              random target): PS's PRNG draw order the engine doesn't follow;
  other       the state diverged on a deterministic line (a mechanic).

usage: python3 tools/protocol/sim-diff-report.py <out-dir> [--list bucket]
"""
import collections
import json
import re
import sys

d = sys.argv[1]
want = sys.argv[sys.argv.index('--list') + 1] if '--list' in sys.argv else None
rows = [json.loads(l) for l in open(f'{d}/results.jsonl')]

def tag(line):
    if not line:
        return '(none)'
    if not line.startswith('|'):
        return line.split('\n')[0][:20]
    return line.split('|')[1] if len(line.split('|')) > 1 else line

def strip_nums(s):
    return re.sub(r'\d+/\d+|\b\d+\b', '#', s or '')

def kind(r):
    if r.get('state_equal'):
        return 'formatting'
    ps, vg = r.get('ps') or '', r.get('vgc') or ''
    tp, tv = tag(ps), tag(vg)
    rng_tags = {'-crit', '-miss', '-damage', '-heal', '-status', '-activate', 'cant', '-start', '-boost', '-unboost', '-fail', '-supereffective', '-resisted', '-immune', '-hitcount', 'faint', '-curestatus', '-enditem', '-item', 'move', '-singleturn', 'switch', 'drag', '-ability', 'turn', '|', 'upkeep', '-weather', '-end'}
    # Same event, different numbers (damage roll / HP): a draw.
    if tp == tv and strip_nums(ps) == strip_nums(vg):
        return 'draw'
    if tp in ('-crit', '-miss') or tv in ('-crit', '-miss'):
        return 'draw'
    # A different mon acting / being hit, a different event at a random point.
    if tp in rng_tags and tv in rng_tags:
        return 'draw'
    return 'other'

n = len(rows)
same = sum(r['identical'] for r in rows)
ps_bad = sum(not r['ps_matches_inprocess'] for r in rows)
by_fmt = collections.Counter(r['format'] for r in rows)
same_fmt = collections.Counter(r['format'] for r in rows if r['identical'])
kinds = collections.Counter(kind(r) for r in rows if not r['identical'])
print(f'battles {n}   identical full streams {same} ({100 * same / n:.1f}%)   PS subprocess != PS in-process: {ps_bad}')
for f in sorted(by_fmt):
    print(f'  {f:32s} {same_fmt[f]:4d} / {by_fmt[f]:4d} ({100 * same_fmt[f] / by_fmt[f]:.1f}%)')
print('first divergence:')
for k in ('formatting', 'draw', 'other'):
    print(f'  {k:10s} {kinds[k]:4d} ({100 * kinds[k] / n:.1f}% of battles)')
buckets = collections.Counter()
examples = {}
for r in rows:
    if r['identical']:
        continue
    key = (kind(r), tag(r.get('ps')), tag(r.get('vgc')))
    buckets[key] += 1
    examples.setdefault(key, r['id'])
print('buckets (kind, PS line tag, vgc line tag):')
for key, c in buckets.most_common():
    print(f'  {c:4d}  {key[0]:10s} {key[1]:>16s} | {key[2]:16s}  e.g. {examples[key]}')
if want:
    for r in rows:
        if not r['identical'] and kind(r) == want:
            print(r['id'], r['format'], '|', (r.get('ps') or '')[:110], '||', (r.get('vgc') or '')[:110])
