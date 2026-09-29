#!/usr/bin/env node
// Experiment 4 (PS side): outcome distribution of turn 1 from a fixed state.
//
// For each captured battle (ps-battle.js output), replays the same teams and
// the same turn-1 choices K times under independent PS seeds and counts each
// active slot's end-of-turn-1 outcome token `species:hp:status:boosts`, plus a
// field token. The engine side is `accuracy dist` (same token format);
// compare with dist-compare.py.
//
// Usage: node ps-dist.js <out_dir> <ids.txt> [K]   (JSON lines to stdout)
// Env:   PS_DIST — built pokemon-showdown dist/sim.

'use strict';

const fs = require('fs');
const path = require('path');
const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
const { Battle } = require(PS_PATH + '/battle');
const { Teams } = require(PS_PATH);

const WEATHER = { raindance: 'rain', primordialsea: 'rain', sunnyday: 'sun', desolateland: 'sun', sandstorm: 'sand', snow: 'snow', snowscape: 'snow', hail: 'snow' };
const TERRAIN = { electricterrain: 'electric', grassyterrain: 'grassy', psychicterrain: 'psychic', mistyterrain: 'misty' };
const BOOSTS = ['atk', 'def', 'spa', 'spd', 'spe', 'accuracy', 'evasion'];

function tokens(battle) {
  const out = [];
  for (const side of battle.sides) {
    for (let i = 0; i < 2; i++) {
      const p = side.active[i];
      if (!p) { out.push('none'); continue; }
      const st = p.fainted || !p.hp ? 'fnt' : (p.status || 'none');
      out.push(`${p.species.id}:${p.hp}:${st}:${BOOSTS.map((b) => p.boosts[b] || 0).join(',')}`);
    }
  }
  const f = battle.field;
  out.push(`w=${WEATHER[f.weather] || 'none'}|t=${TERRAIN[f.terrain] || 'none'}|tr=${!!f.pseudoWeather.trickroom}`);
  return out;
}

function seedFor(i) {
  return 'sodium,' + (0x5eed0000 + i).toString(16).padStart(32, '0');
}

function run(file, k) {
  const d = JSON.parse(fs.readFileSync(file, 'utf8'));
  const t1 = d.turns[0];
  const p1 = t1.choices.p1[0], p2 = t1.choices.p2[0];
  const team1 = Teams.pack(Teams.import(d.p1team));
  const team2 = Teams.pack(Teams.import(d.p2team));
  const n1 = Teams.import(d.p1team).length, n2 = Teams.import(d.p2team).length;
  const hist = [{}, {}, {}, {}, {}];
  let paused = 0, errors = 0;
  for (let i = 0; i < k; i++) {
    const battle = new Battle({
      formatid: d.format, seed: seedFor(i),
      p1: { name: 'P1', team: team1 }, p2: { name: 'P2', team: team2 },
    });
    try {
      battle.makeChoices('team ' + '1234'.slice(0, n1), 'team ' + '1234'.slice(0, n2));
      battle.makeChoices(p1, p2);
    } catch (e) {
      errors++;
      continue;
    }
    // A mid-turn decision (pivot, Eject Button...) pauses the turn; skip it.
    if (battle.midTurn && battle.queue.list.some((a) => a.choice === 'residual')) { paused++; continue; }
    tokens(battle).forEach((tok, j) => { hist[j][tok] = (hist[j][tok] || 0) + 1; });
  }
  return { id: d.id, k, side: 'ps', paused, errors, hist };
}

const [outDir, idsPath, kArg] = process.argv.slice(2);
const k = parseInt(kArg || '2000', 10);
const ids = fs.readFileSync(idsPath, 'utf8').split(/\s+/).filter(Boolean);
for (const id of ids) {
  process.stdout.write(JSON.stringify(run(path.join(outDir, `out_${id}.json`), k)) + '\n');
}
