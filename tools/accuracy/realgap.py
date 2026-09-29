#!/usr/bin/env python3
"""Experiment 1b: how close can ANY simulator get to the real ladder logs?

For each reconstructed battle, compare end-of-turn-1 HP (as % of max) and
faint status of every Pokemon active on turn 1 between:

  * the real spectated log (HP shown as x/100),
  * Pokemon Showdown itself playing the reconstructed teams with the log's
    turn-1 choices (its own RNG, or RNG forced to the log's visible crits,
    misses and secondary effects), and
  * the engine replaying that PS battle with PS's outcomes keyed in.

The Phase 2 gate's per-slot rule is used: agree if |HP% difference| <= 5 and
the faint status matches. A battle-turn agrees if every such Pokemon agrees.

Usage:
    python3 realgap.py <replay_root> <ps_battles_dir> <engine_turn1.jsonl> [label]
"""
import json
import math
import os
import re
import sys

TOL = 5.0


def to_id(s):
    return re.sub(r'[^a-z0-9]', '', s.lower())


def base_species(details):
    sp = to_id(details.split(',')[0])
    for suf in ('megax', 'megay', 'megaz', 'mega'):
        if sp.endswith(suf):
            return sp[: -len(suf)]
    return sp


def turn1_hp(log, exact):
    """(side, species) -> (pct, fainted) for the four Pokemon on the field at
    the end of turn 1's residuals (`|upkeep|`, before faint replacements),
    from a protocol log. `exact` logs carry 'x/maxhp', spectated ones x/100."""
    slot = {}   # 'p1a' -> (side, species)
    nick = {}   # side+name -> species
    hp = {}
    turn = 0
    for line in log.split('\n'):
        p = line.split('|')
        if len(p) < 2:
            continue
        cmd = p[1]
        if cmd == 'turn':
            turn = int(p[2])
            if turn >= 2:
                break
            continue
        if cmd == 'upkeep' and turn == 1:
            break
        if cmd in ('switch', 'drag', 'replace') and len(p) > 4:
            ident = p[2]
            side, name = ident[:2], ident.split(':', 1)[1].strip()
            sp = base_species(p[3])
            nick[side + name] = sp
            slot[ident[:3]] = (side, sp)
            hp[(side, sp)] = parse_hp(p[4])
            continue
        if cmd in ('-damage', '-heal', '-sethp') and len(p) > 3 and ':' in p[2]:
            ident = p[2]
            sp = nick.get(ident[:2] + ident.split(':', 1)[1].strip())
            if sp:
                hp[(ident[:2], sp)] = parse_hp(p[3])
        if cmd == 'faint' and len(p) > 2 and ':' in p[2]:
            ident = p[2]
            sp = nick.get(ident[:2] + ident.split(':', 1)[1].strip())
            if sp:
                hp[(ident[:2], sp)] = (0.0, True)
    return {k: hp[k] for k in slot.values() if k in hp}


def parse_hp(s):
    s = s.split()[0]
    if s == '0':
        return (0.0, True)
    if '/' in s:
        a, b = s.split('/')
        try:
            return (100.0 * int(a) / int(b), int(a) == 0)
        except ValueError:
            return (None, False)
    return (None, False)


def find_replays(root):
    out = {}
    for d, _, files in os.walk(root):
        for f in files:
            if f.endswith('.json'):
                out[f] = os.path.join(d, f)
    return out


def main():
    root, bdir, eng_path = sys.argv[1], sys.argv[2], sys.argv[3]
    label = sys.argv[4] if len(sys.argv) > 4 else bdir
    replays = find_replays(root)
    eng = {}
    for line in open(eng_path):
        r = json.loads(line)
        m = {}
        for i, (tok, mx) in enumerate(r['slots']):
            if tok == 'none':
                continue
            sp, hpv = tok.split(':')[0], int(tok.split(':')[1])
            side = 'p1' if i < 2 else 'p2'
            m[(side, base_species(sp))] = (100.0 * hpv / mx if mx else 0.0, hpv == 0)
        eng[r['id']] = m
    n = ps_ok = eng_ok = slots = ps_slot_ok = eng_slot_ok = 0
    for f in sorted(os.listdir(bdir)):
        if not f.startswith('out_'):
            continue
        b = json.load(open(os.path.join(bdir, f)))
        rep = replays.get(b['_meta'].get('replay') or '')
        if not rep or b['id'] not in eng:
            continue
        real = turn1_hp(json.load(open(rep))['log'], exact=False)
        ps = turn1_hp(b['_meta']['log'], exact=True)
        e = eng[b['id']]
        keys = [k for k in real if real[k][0] is not None]
        if not keys:
            continue
        n += 1
        all_ps = all_eng = True
        for k in keys:
            slots += 1
            rp, rf = real[k]
            pp = ps.get(k)
            ok_ps = pp is not None and pp[0] is not None and abs(pp[0] - rp) <= TOL and pp[1] == rf
            ee = e.get(k)
            # a mon replaced after fainting is no longer in the engine's
            # pre-replacement slots only if it never fainted; fall back to PS
            if ee is None and pp is not None and pp[1]:
                ee = None
            ok_eng = ee is not None and abs(ee[0] - rp) <= TOL and ee[1] == rf
            ps_slot_ok += ok_ps
            eng_slot_ok += ok_eng
            all_ps &= ok_ps
            all_eng &= ok_eng
        ps_ok += all_ps
        eng_ok += all_eng

    def ci(k, m):
        if m == 0:
            return (0, 0)
        z = 1.96
        p = k / m
        d = 1 + z * z / m
        c = p + z * z / (2 * m)
        h = z * math.sqrt(p * (1 - p) / m + z * z / (4 * m * m))
        return (100 * (c - h) / d, 100 * (c + h) / d)

    print(f'== {label}: {n} battles, turn 1 vs the real log (|HP%| <= {TOL:g} and faint match)')
    lo, hi = ci(ps_ok, n)
    print(f'PS itself:  whole turn agrees {ps_ok}/{n} = {100*ps_ok/max(1,n):.1f}% [{lo:.1f}-{hi:.1f}]; '
          f'per Pokemon {ps_slot_ok}/{slots} = {100*ps_slot_ok/max(1,slots):.1f}%')
    lo, hi = ci(eng_ok, n)
    print(f'engine:     whole turn agrees {eng_ok}/{n} = {100*eng_ok/max(1,n):.1f}% [{lo:.1f}-{hi:.1f}]; '
          f'per Pokemon {eng_slot_ok}/{slots} = {100*eng_slot_ok/max(1,slots):.1f}%')


if __name__ == '__main__':
    main()
