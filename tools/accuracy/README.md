# tools/accuracy — reproducing the accuracy-proof experiments

These tools produce the numbers in
[`docs/accuracy/2026-09-accuracy-proof.md`](../../docs/accuracy/2026-09-accuracy-proof.md).

## Setup (once)

```sh
# Pokémon Showdown, built (the study used a5df8274e85b0889bf2a9b3422a08b39732374fc)
git clone https://github.com/smogon/pokemon-showdown /tmp/ps && (cd /tmp/ps && git checkout a5df8274 && npm install --omit=optional && node build)
export PS_DIST=/tmp/ps/dist/sim

# engine tools (the ps-rng feature is needed for psrng / trace)
cargo build --release -p vgc-engine-conformance --features ps-rng

# a random sample of real Reg M-C ladder logs (read-only copy from hil)
ssh hil 'cd /opt/mimikyu/data/replays/gen9championsvgc2026regmc && nice -n 19 find 2026-09-1* 2026-09-2[0-8] -name "*.json" \
  | shuf -n 3000 --random-source=<(yes 42) | nice -n 19 tar czf /tmp/mc_sample.tgz -T -'
mkdir -p mc && scp hil:/tmp/mc_sample.tgz mc/ && (cd mc && tar xzf mc_sample.tgz)
```

## Pipeline

```sh
W=/tmp/acc   # any work dir
node tools/accuracy/recon.js mc $W/jobs.jsonl                  # real logs -> full-information jobs
node tools/accuracy/ps-battle.js $W/jobs.jsonl $W/b --count 1300   # PS plays them (log-guided choices)
```

| experiment | command |
|---|---|
| 1. keyed (forced-RNG) conformance | `target/release/accuracy keyed $W/b --jsonl $W/keyed.jsonl` |
| 2. taxonomy + offenders | `python3 tools/accuracy/analyze.py keyed $W/keyed.jsonl $W/b` |
| 1b. real-log ceiling | `FORCE_LOG=1 node tools/accuracy/ps-battle.js $W/jobs.jsonl $W/bf --count 1300 --max-turns 3`<br>`target/release/accuracy turn1 $W/b > $W/t1.jsonl; target/release/accuracy turn1 $W/bf > $W/tf.jsonl`<br>`python3 tools/accuracy/realgap.py mc $W/b $W/t1.jsonl; python3 tools/accuracy/realgap.py mc $W/bf $W/tf.jsonl` |
| 3. seeded differential on PS's PRNG | `target/release/accuracy psrng $W/b --jsonl $W/psrng.jsonl`<br>`target/release/accuracy trace $W/b/out_<id>.json` (first divergent draw) |
| 4. distributions (scenarios) | `node tools/accuracy/ps-dist.js tools/accuracy/dist/scenarios tools/accuracy/dist/scenarios/ids.txt 10000 > $W/sc_ps.jsonl`<br>`DIST_K=10000 target/release/accuracy dist tools/accuracy/dist/scenarios/out_*.json > $W/sc_en.jsonl`<br>`python3 tools/accuracy/dist-compare.py $W/sc_ps.jsonl $W/sc_en.jsonl` |
| 4. distributions (real states) | pick ids (turn 1 matched in keyed mode, no pivots/eject items), then `ps-dist.js $W/b ids.txt 2000`, `accuracy dist`, `dist-compare.py` |
| repros | `tools/accuracy/repros/run.sh` |
| triage one battle | `tools/accuracy/show.sh $W/b <id> [turn]` |

Everything is deterministic: job ids and PS seeds derive from the replay file
name, so the same sample reproduces the same battles.

## Files

- `recon.js`: parses spectated logs. It builds complete teams (revealed
  moves, items and abilities, filled from usage in the sample; a Stat Point
  spread by role), plus per-turn choice intents and the visible turn-1 random
  outcomes.
- `ps-battle.js`: runs PS battles. For each battle it records:
  - the resolved choices by phase (main, mid-turn, replacement), with switch
    targets remapped to the original team order;
  - keyed RNG envelopes (via `../ps-golden-driver/conformance-driver.js`);
  - a raw trace of every PRNG call with its call site;
  - end-of-turn state.

  `FORCE_LOG=1` forces turn-1 crit, accuracy and secondary outcomes to the
  real log. `SEED_SALT` reseeds.
- `analyze.py`, `realgap.py`, `dist-compare.py`: the statistics.
- `show.sh`: triage view.
- `repros/`: scripted minimal repros for confirmed engine bugs.
- `dist/scenarios/`: RNG micro-scenarios.
