#!/usr/bin/env node
// PS distribution runner.
//
// Given a fixed scenario (teams + first-turn actions), runs N PS battles
// with varying PRNG seeds and dumps the *distribution* of post-turn-1
// target HP / status / faints. The aim is probabilistic-correctness
// validation: do PS and vgc-engine produce the same outcome
// distribution from the same setup? — independent of whether they
// consume RNG draws in the same order.
//
// Usage:
//   node ps-distribution.js path/to/scenario.json > ps-dist.json
//
// Input (scenario.json):
//   {
//     "name": "...",
//     "format": "gen9customgame",
//     "p1_team": "<showdown export>",
//     "p2_team": "<showdown export>",
//     "p1_action": "move 1",
//     "p2_action": "move 1",
//     "target_side": "p1" | "p2",
//     "target_slot": "a" | "b",
//     "trials": 1000
//   }
//
// Output (stdout):
//   {
//     "side": "ps",
//     "scenario": "...",
//     "trials": N,
//     "target_max_hp": <int>,
//     "hp_histogram": { "<hp>": count, ... },
//     "status_counts": { "none": N1, "brn": N2, ... },
//     "fainted_count": <int>,
//     "errors": [...]
//   }
'use strict';

const fs = require('fs');

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
const { BattleStream, Teams, getPlayerStreams } = require(PS_PATH);

function parseTeam(text) {
  const team = Teams.import(text);
  if (!team) throw new Error('PS rejected team');
  return team;
}

async function driveSide(stream, action, teamLen) {
  // For one turn, we expect a single team-preview request followed by a
  // single move request. Both consumed by writing the action back.
  let tpDone = false;
  let moveDone = false;
  for await (const chunk of stream) {
    for (const line of chunk.split('\n')) {
      if (line.startsWith('|request|')) {
        const req = JSON.parse(line.slice('|request|'.length));
        if (req.teamPreview && !tpDone) {
          // Pick all team members in order: "team 123..."
          const order = Array.from({ length: teamLen }, (_, i) => i + 1).join('');
          stream.write(`team ${order}`);
          tpDone = true;
        } else if (req.active && !moveDone) {
          stream.write(action);
          moveDone = true;
          return;
        }
      }
    }
  }
}

function targetState(battle, side, slot) {
  const sideIdx = side === 'p1' ? 0 : 1;
  const slotIdx = slot === 'a' ? 0 : 1;
  const mon = battle.sides[sideIdx].active[slotIdx];
  if (!mon) return null;
  return {
    hp: mon.hp,
    maxhp: mon.maxhp,
    status: mon.status || 'none',
    fainted: !!mon.fainted,
  };
}

async function runOnce(scenario, seed) {
  const team1 = parseTeam(scenario.p1_team);
  const team2 = parseTeam(scenario.p2_team);
  const format = scenario.format || 'gen9customgame';
  const stream = new BattleStream();
  const sides = getPlayerStreams(stream);
  // Drain omniscient quietly.
  const drain = (async () => { for await (const _ of sides.omniscient) {} })();
  const p1 = driveSide(sides.p1, scenario.p1_action, team1.length);
  const p2 = driveSide(sides.p2, scenario.p2_action, team2.length);
  sides.omniscient.write('>start ' + JSON.stringify({ formatid: format, seed }));
  sides.omniscient.write('>player p1 ' + JSON.stringify({ name: 'P1', team: Teams.pack(team1) }));
  sides.omniscient.write('>player p2 ' + JSON.stringify({ name: 'P2', team: Teams.pack(team2) }));
  const timeoutSym = Symbol('t');
  await Promise.race([
    Promise.all([p1.catch(() => null), p2.catch(() => null)]),
    new Promise((r) => setTimeout(() => r(timeoutSym), 4000)),
  ]);
  // Wait one tick for the turn to resolve before reading state.
  await new Promise((r) => setImmediate(r));
  const state = targetState(stream.battle, scenario.target_side, scenario.target_slot);
  sides.omniscient.writeEnd();
  await Promise.race([drain, new Promise((r) => setTimeout(r, 100))]);
  return state;
}

function pickSeed(i) {
  // Deterministic [u16;4] from trial index. Mulberry32 for spread.
  let x = (i * 2654435761) >>> 0;
  const out = [];
  for (let k = 0; k < 4; k++) {
    x = (x + 0x6D2B79F5) >>> 0;
    let t = x;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    out.push(((t ^ (t >>> 14)) >>> 0) & 0xFFFF);
  }
  return out;
}

async function main() {
  const path = process.argv[2];
  if (!path) {
    process.stderr.write('usage: ps-distribution.js scenario.json\n');
    process.exit(2);
  }
  const scenario = JSON.parse(fs.readFileSync(path, 'utf8'));
  const trials = scenario.trials || 200;
  const hp = {};
  const statuses = {};
  let fainted = 0;
  let maxhp = null;
  const errors = [];
  for (let i = 0; i < trials; i++) {
    try {
      const seed = pickSeed(i);
      const st = await runOnce(scenario, seed);
      if (!st) { errors.push(`trial ${i}: null state`); continue; }
      maxhp = st.maxhp;
      hp[st.hp] = (hp[st.hp] || 0) + 1;
      statuses[st.status] = (statuses[st.status] || 0) + 1;
      if (st.fainted) fainted++;
    } catch (e) {
      errors.push(`trial ${i}: ${e.message || e}`);
    }
    if ((i + 1) % 100 === 0) process.stderr.write(`ps: ${i + 1}/${trials}\n`);
  }
  const out = {
    side: 'ps',
    scenario: scenario.name,
    trials,
    target_max_hp: maxhp,
    hp_histogram: hp,
    status_counts: statuses,
    fainted_count: fainted,
    errors,
  };
  process.stdout.write(JSON.stringify(out, null, 2));
}

main().catch((e) => {
  process.stderr.write(`fatal: ${e.stack || e}\n`);
  process.exit(1);
});
