#!/usr/bin/env python3
"""Experiment 4: compare turn-1 outcome distributions, engine vs PS.

Usage:
    python3 dist-compare.py <ps.jsonl> <engine.jsonl> [--clean-ids keyed.jsonl]

Each input line is {id, k, hist: [slot0..slot3, field] -> {token: count}}
(ps-dist.js / `accuracy dist`). Tokens are `species:hp:status:boosts`;
fainted slots are folded to `species:0:fnt`.

Per state and slot, a chi-square test of homogeneity on the outcome tokens
(categories with expected count < 5 pooled). Under the null "same outcome
distribution" the p-values are uniform, so we report how many tests reject
at alpha = 0.01 (expect ~1%), Benjamini-Hochberg discoveries at q = 0.05,
and a KS test of the p-values against U(0,1). Also pooled rates that are
RNG-only: faint rate and status rate per slot, and the HP-loss quantiles.

`--clean-ids` restricts to states whose turn 1 matched PS exactly in the
keyed replay (mechanics verified on at least one outcome path), so a
distribution difference there points at RNG *distribution* rather than
mechanics.
"""
import json
import sys
from collections import Counter

import numpy as np
from scipy import stats


def norm(tok):
    parts = tok.split(':')
    if len(parts) >= 2 and parts[1] == '0':
        return f'{parts[0]}:0:fnt'
    return tok


def load(path):
    out = {}
    for line in open(path):
        line = line.strip()
        if not line.startswith('{'):
            continue
        r = json.loads(line)
        out[r['id']] = [Counter({norm(k): v for k, v in h.items()}) for h in r['hist']]
    return out


def chi2(a, b):
    keys = sorted(set(a) | set(b))
    na, nb = sum(a.values()), sum(b.values())
    if na == 0 or nb == 0 or len(keys) < 2:
        return None
    rows = np.array([[a.get(k, 0) for k in keys], [b.get(k, 0) for k in keys]], dtype=float)
    tot = rows.sum(axis=0)
    exp_min = np.outer([na, nb], tot / (na + nb)).min(axis=0)
    small = exp_min < 5
    if small.any():
        pooled = rows[:, small].sum(axis=1, keepdims=True)
        rows = np.hstack([rows[:, ~small], pooled]) if (~small).any() else pooled
        rows = rows[:, rows.sum(axis=0) > 0]
    if rows.shape[1] < 2:
        return None
    chi, p, dof, _ = stats.chi2_contingency(rows, correction=False)
    return p


def main():
    ps = load(sys.argv[1])
    en = load(sys.argv[2])
    clean = None
    if '--clean-ids' in sys.argv:
        clean = set()
        for line in open(sys.argv[sys.argv.index('--clean-ids') + 1]):
            r = json.loads(line)
            if not r.get('engine_error') and r['matched_turns'] >= 1:
                clean.add(r['id'])
    ids = sorted(set(ps) & set(en) & (clean if clean is not None else set(ps)))
    pvals, rows = [], []
    faint_ps = faint_en = n_ps = n_en = 0
    for i in ids:
        worst = 1.0
        for slot in range(5):
            p = chi2(ps[i][slot], en[i][slot])
            if p is None:
                continue
            pvals.append(p)
            worst = min(worst, p)
            if slot < 4:
                faint_ps += sum(v for k, v in ps[i][slot].items() if k.endswith(':fnt'))
                faint_en += sum(v for k, v in en[i][slot].items() if k.endswith(':fnt'))
                n_ps += sum(ps[i][slot].values())
                n_en += sum(en[i][slot].values())
        rows.append((worst, i))
    pvals = np.array(pvals)
    m = len(pvals)
    print(f'states: {len(ids)}   slot-level chi-square tests with >1 outcome: {m}')
    if m == 0:
        return
    rej = int((pvals < 0.01).sum())
    print(f'rejections at alpha=0.01: {rej}/{m} = {100*rej/m:.1f}% (null expectation 1.0%)')
    order = np.sort(pvals)
    bh = [k for k in range(1, m + 1) if order[k - 1] <= 0.05 * k / m]
    print(f'Benjamini-Hochberg discoveries at q=0.05: {max(bh) if bh else 0}')
    ks = stats.kstest(pvals, 'uniform')
    print(f'KS test of p-values vs U(0,1): D={ks.statistic:.3f}, p={ks.pvalue:.3g}')
    print(f'pooled faint rate: PS {faint_ps}/{n_ps} = {100*faint_ps/max(1,n_ps):.2f}%   '
          f'engine {faint_en}/{n_en} = {100*faint_en/max(1,n_en):.2f}%')
    rows.sort()
    print('\nlowest per-state p-values:')
    for p, i in rows[:12]:
        print(f'  {i}  min slot p = {p:.2e}')


if __name__ == '__main__':
    main()
