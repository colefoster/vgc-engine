#!/usr/bin/env python3
"""Compare engine observed damages vs Smogon damage-calc expected set.

Usage:
    python3 compare.py engine.json calc.json

Output:
  - Calc expected set (16 sorted damage values, or 1 for fixed-damage).
  - Engine observed unique values.
  - Set difference: any engine value NOT in calc's expected set is a
    REAL bug — engine produced a damage value the spec says is
    impossible.
  - Pass/fail verdict + a coarse coverage stat (how many of calc's 16
    values appeared in engine trials).
"""
import json
import sys


def load(p):
    with open(p) as f:
        return json.load(f)


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    eng = load(sys.argv[1])
    calc = load(sys.argv[2])

    # Use the union of non-crit + crit damages so a CSS-style harness
    # that observes both gets a fair test. The unioned set is what the
    # engine is allowed to produce per spec.
    calc_set = set(calc.get("damage_union", calc["damage"]))
    eng_observed = eng["observed_damage"]
    eng_unique = set(eng["observed_unique"])

    out_of_spec = sorted(v for v in eng_unique if v not in calc_set)
    in_spec = sorted(v for v in eng_unique if v in calc_set)
    missing = sorted(calc_set - eng_unique)

    print(f"== {calc.get('name', '?')} ({calc.get('move', '?')}) ==")
    print(f"  trials:         {eng['trials']}")
    print(f"  calc expected:  {sorted(calc_set)}")
    print(f"  engine unique:  {sorted(eng_unique)}")
    print(f"  in spec:        {in_spec}  ({len(in_spec)}/{len(calc_set)} calc values seen)")
    print(f"  out of spec:    {out_of_spec}  ← engine values not in calc's set")
    print(f"  missing:        {missing}  ← calc values engine never produced")
    print()
    if out_of_spec:
        print(f"  VERDICT: FAIL — engine produced {len(out_of_spec)} damage values impossible per spec.")
        sys.exit(1)
    else:
        print(f"  VERDICT: PASS — every engine damage is in calc's expected set.")


if __name__ == "__main__":
    main()
