#!/usr/bin/env node
// Read-only full-state probe for Tera-at-faint (PS a5df8274
// sim/battle.ts:2558-2574; :2568 `delete pokemon.terastallized`).
//
// Plays each job of a ps-battle.js jobs file on a plain PS Battle (no RNG
// patching, no forced outcomes) and after every scripted turn dumps, for
// every team member: species, base species, hp, fainted, active,
// terastallized and canTerastallize, plus any side field naming Tera. The
// protocol and the standard captures carry HP and fainted slots only, so
// they can't show the flag.
//
// Usage: PS_DIST=<built pokemon-showdown dist/sim> node probe.js jobs.jsonl > probe.jsonl
'use strict';

const fs = require('fs');

const PS_DIST = process.env.PS_DIST;
if (!PS_DIST || !fs.existsSync(PS_DIST + '/index.js')) {
  console.error('set PS_DIST to a built pokemon-showdown dist/sim');
  process.exit(2);
}
const { Teams } = require(PS_DIST);
const { Battle } = require(PS_DIST + '/battle');

function sideState(side) {
  const tera = {};
  for (const k of Object.keys(side)) {
    if (/tera/i.test(k)) tera[k] = side[k];
  }
  return {
    side: side.id,
    tera,
    team: side.pokemon.map((p) => ({
      species: p.species.id,
      baseSpecies: p.baseSpecies.id,
      setSpecies: p.set.species,
      hp: p.hp,
      maxhp: p.maxhp,
      fainted: p.fainted,
      active: p.isActive,
      terastallized: p.terastallized === undefined ? null : p.terastallized,
      teraType: p.teraType,
      canTerastallize: p.canTerastallize === undefined ? null : p.canTerastallize,
    })),
  };
}

function run(job) {
  const battle = new Battle({ formatid: job.format, seed: job.seed });
  battle.setPlayer('p1', { name: 'P1', team: Teams.pack(Teams.import(job.p1team)) });
  battle.setPlayer('p2', { name: 'P2', team: Teams.pack(Teams.import(job.p2team)) });
  const errors = [];
  const turns = [];
  try {
    battle.makeChoices('default', 'default'); // team preview, team order
    for (const t of Object.keys(job.script).map(Number).sort((a, b) => a - b)) {
      if (battle.ended) break;
      const c = job.script[t];
      battle.makeChoices(c.p1, c.p2);
      turns.push({ turn: t, psTurn: battle.turn, sides: battle.sides.map(sideState) });
    }
  } catch (e) {
    errors.push(String(e));
  }
  return { id: job.id, seed: job.seed, ended: battle.ended, errors, turns, log: battle.log.join('\n') };
}

const lines = fs.readFileSync(process.argv[2], 'utf8').split('\n').filter((l) => l.trim());
for (const l of lines) process.stdout.write(JSON.stringify(run(JSON.parse(l))) + '\n');
