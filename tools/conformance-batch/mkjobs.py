#!/usr/bin/env python3
"""Generate PS-conformance jobs with STABLE content-derived IDs.

Each battle's identity is a hash of (format, p1 team, p2 team, seed) — NOT a
sequential index. So the same (teams, seed) combo always produces the same
`job_<id>.json` / `out_<id>.json`, and a punch-list reference to a battle ID
stays valid across regenerations. Generating MORE battles only ever ADDS new
IDs; it never renames an existing one (the old `out_NN.json` scheme silently
remapped every index whenever the batch was regenerated).

Usage:
    python3 mkjobs.py [num_pairings] [seeds_per_pairing]

    num_pairings        how many distinct team match-ups        (default 50)
    seeds_per_pairing   battles per match-up, distinct seeds     (default 1)

Total battles = num_pairings * seeds_per_pairing. Defaults reproduce the
original 50-battle batch (team i vs team i+50, one seed each) — those battles
keep stable IDs from now on.

Env overrides:
    CONF_TEAMS_FILE  team list  (default ~/Dev/mimikyu/.../regmb_random_100.txt)
    CONF_JOBS_DIR    output dir (default /tmp/conf-batch/jobs)
"""
import re, json, os, sys, hashlib

TEAMS_FILE = os.environ.get(
    'CONF_TEAMS_FILE',
    os.path.expanduser('~/Dev/mimikyu/data/generated_teams/regmb_random_100.txt'),
)
JOBS_DIR = os.environ.get('CONF_JOBS_DIR', '/tmp/conf-batch/jobs')
FORMAT = 'gen9championsdoublescustomgame'
MAX_TURNS = 12


def battle_id(fmt, p1, p2, seed):
    """Stable 10-hex-char id for a battle's full content."""
    h = hashlib.sha1()
    for part in (fmt, p1, p2, ','.join(map(str, seed))):
        h.update(b'\x00')
        h.update(part.encode())
    return h.hexdigest()[:10]


def derive_seed(i, j, k):
    """Deterministic 4-byte PS seed from the pairing + seed-replica indices."""
    d = hashlib.sha1(f'{i}-{j}-{k}'.encode()).digest()
    return [d[0], d[1], d[2], d[3]]


def main():
    txt = open(TEAMS_FILE).read()
    blocks = re.split(r'=== Team \d+ ===\n', txt)
    teams = [b.strip() for b in blocks if b.strip()]
    T = len(teams)
    print(f'parsed {T} teams from {TEAMS_FILE}', file=sys.stderr)
    if T < 2:
        sys.exit('need at least 2 teams')

    n_pairings = int(sys.argv[1]) if len(sys.argv) > 1 else 50
    seeds_per = int(sys.argv[2]) if len(sys.argv) > 2 else 1

    os.makedirs(JOBS_DIR, exist_ok=True)
    written = 0
    ids = []
    for p in range(n_pairings):
        # First T pairings reproduce the legacy `i vs i+50` half-split; beyond
        # that, widen the offset so each new pairing is a fresh match-up.
        i = p % T
        half = T // 2
        offset = half + (p // T)
        j = (i + offset) % T
        if j == i:
            j = (j + 1) % T
        for k in range(seeds_per):
            seed = derive_seed(i, j, k)
            bid = battle_id(FORMAT, teams[i], teams[j], seed)
            job = {
                'id': bid,
                'seed': seed,
                'format': FORMAT,
                'max_turns': MAX_TURNS,
                'p1': {'team': teams[i]},
                'p2': {'team': teams[j]},
            }
            json.dump(job, open(os.path.join(JOBS_DIR, f'job_{bid}.json'), 'w'))
            ids.append(bid)
            written += 1
    print(f'wrote {written} jobs ({len(set(ids))} unique ids) to {JOBS_DIR}',
          file=sys.stderr)


if __name__ == '__main__':
    main()
