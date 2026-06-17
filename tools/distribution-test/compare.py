#!/usr/bin/env python3
"""Side-by-side compare of two distribution-test outputs (engine + PS).

Usage:
    python3 compare.py engine-dist.json ps-dist.json

Prints:
  * Headline counts (trials, max HP, faint rate).
  * HP histogram in 1/Nth bins side-by-side.
  * Kolmogorov-Smirnov (two-sample) statistic between the HP
    distributions — no scipy dep; we compute D directly on the empirical
    CDFs.
  * Status-distribution diff.

The KS statistic D ∈ [0,1] is the max absolute gap between the two
empirical CDFs. With N1, N2 trials, the 95% critical value is
~1.36 * sqrt((N1+N2)/(N1*N2)). If D is below that, we can't reject the
null that they're drawn from the same distribution.
"""
import json
import math
import sys
from collections import Counter


def load(path):
    with open(path) as f:
        return json.load(f)


def cdf(hist_counts, total):
    """Return sorted (hp, cumulative_fraction) pairs."""
    items = sorted(hist_counts.items(), key=lambda x: int(x[0]))
    out = []
    acc = 0
    for hp, c in items:
        acc += c
        out.append((int(hp), acc / total))
    return out


def ks_statistic(a_hist, b_hist):
    """Two-sample KS D over integer HP keys."""
    a_total = sum(a_hist.values())
    b_total = sum(b_hist.values())
    if a_total == 0 or b_total == 0:
        return 0.0, 0.0
    keys = sorted(set(int(k) for k in a_hist.keys()) | set(int(k) for k in b_hist.keys()))
    a_cdf = b_cdf = 0
    d = 0.0
    for k in keys:
        a_cdf += a_hist.get(str(k), 0) + a_hist.get(k, 0)
        b_cdf += b_hist.get(str(k), 0) + b_hist.get(k, 0)
        gap = abs(a_cdf / a_total - b_cdf / b_total)
        if gap > d:
            d = gap
    # 95% critical value: c(α=0.05) ≈ 1.36
    crit = 1.36 * math.sqrt((a_total + b_total) / (a_total * b_total))
    return d, crit


def mean_var(hist):
    total = sum(hist.values())
    if total == 0:
        return 0.0, 0.0
    s = 0
    s2 = 0
    for k, c in hist.items():
        v = int(k)
        s += v * c
        s2 += v * v * c
    mean = s / total
    var = s2 / total - mean * mean
    return mean, var


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    a = load(sys.argv[1])
    b = load(sys.argv[2])

    # Normalise keys to strings so both shapes (engine: int keys via serde,
    # ps: string keys from JS) survive the diff cleanly.
    def keyify(h):
        return {str(k): int(v) for k, v in h.items()}

    a_hp = keyify(a["hp_histogram"])
    b_hp = keyify(b["hp_histogram"])

    print(f"== scenario: {a.get('scenario')} ==")
    print(f"engine: {a['trials']} trials, max_hp={a.get('target_max_hp')}, "
          f"fainted={a.get('fainted_count')}")
    print(f"ps:     {b['trials']} trials, max_hp={b.get('target_max_hp')}, "
          f"fainted={b.get('fainted_count')}")

    a_mean, a_var = mean_var(a_hp)
    b_mean, b_var = mean_var(b_hp)
    print()
    print(f"engine HP: mean={a_mean:.2f}, stdev={math.sqrt(a_var):.2f}")
    print(f"ps     HP: mean={b_mean:.2f}, stdev={math.sqrt(b_var):.2f}")
    print(f"mean delta = {a_mean - b_mean:+.3f}")

    d, crit = ks_statistic(a_hp, b_hp)
    verdict = "SAME distribution (D < crit)" if d <= crit else "DIFFERENT distributions (D ≥ crit)"
    print()
    print(f"KS two-sample: D={d:.4f}, 95% crit={crit:.4f} → {verdict}")

    # Status counts side by side.
    print()
    print("status counts:")
    all_statuses = sorted(set(a.get("status_counts", {}).keys()) | set(b.get("status_counts", {}).keys()))
    print(f"  {'status':<6} {'engine':>7} {'ps':>7} {'engine%':>9} {'ps%':>7}")
    for s in all_statuses:
        ae = a.get("status_counts", {}).get(s, 0)
        be = b.get("status_counts", {}).get(s, 0)
        ap = 100.0 * ae / a["trials"] if a["trials"] else 0
        bp = 100.0 * be / b["trials"] if b["trials"] else 0
        print(f"  {s:<6} {ae:>7} {be:>7} {ap:>8.2f}% {bp:>6.2f}%")

    # Compact HP histogram side-by-side (bucketed by every Nth HP point if range wide).
    print()
    all_hp = sorted(set(int(k) for k in a_hp.keys()) | set(int(k) for k in b_hp.keys()))
    if not all_hp:
        return
    span = max(all_hp) - min(all_hp)
    bucket = max(1, span // 30)
    buckets_a = Counter()
    buckets_b = Counter()
    for k, v in a_hp.items():
        buckets_a[int(k) // bucket * bucket] += v
    for k, v in b_hp.items():
        buckets_b[int(k) // bucket * bucket] += v
    keys = sorted(set(buckets_a.keys()) | set(buckets_b.keys()))
    print(f"HP histogram (bucket={bucket}):")
    max_count = max(max(buckets_a.values(), default=0), max(buckets_b.values(), default=0))
    bar_w = 30
    for k in keys:
        ea = buckets_a.get(k, 0)
        eb = buckets_b.get(k, 0)
        bar_a = "#" * int(bar_w * ea / max_count) if max_count else ""
        bar_b = "#" * int(bar_w * eb / max_count) if max_count else ""
        print(f"  {k:>4}-{k+bucket-1:<4} eng {ea:>4} {bar_a:<{bar_w}} | ps {eb:>4} {bar_b:<{bar_w}}")


if __name__ == "__main__":
    main()
