#!/usr/bin/env node
// PS move-target oracle for doubles legal-choice parity.
//
// Plays a golden-style input's `turns` (PS choice strings, both sides),
// then, for every active that has a move to choose (not fainted, not a
// commanding Tatsugiri), offers PS every `move N [T] [terastallize|mega]`
// with T in {none, 1, 2, -1, -2}. A choice is kept when PS's own
// Side.chooseMove accepts it (sim/side.ts:586-672: the request's move target,
// targetTypeChoices + validTargetLoc, sim/battle.ts:2399-2429; disabled
// moves; Tera / Mega permits). Prints {"p1": [cmds | null, cmds | null],
// "p2": ...}, each list sorted; null = no move choice for that slot.
// The engine side is crates/vgc-engine-golden/tests/ally_target_choices.rs.
//
// Usage: PS_DIST=<pokemon-showdown>/dist/sim node target-choices.js <input.json>
// PS_DIST defaults to the `pokemon-showdown` npm package's dist/sim; the
// committed fixtures were made with PS a5df8274.

'use strict';
const fs = require('fs');
const PS_PATH = process.env.PS_DIST || 'pokemon-showdown/dist/sim';
const { Teams } = require(PS_PATH);
const { Battle } = require(PS_PATH + '/battle');

const job = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const battle = new Battle({ formatid: job.format, seed: job.seed || [1, 2, 3, 4] });
battle.setPlayer('p1', { name: 'P1', team: Teams.pack(Teams.import(job.p1.team)) });
battle.setPlayer('p2', { name: 'P2', team: Teams.pack(Teams.import(job.p2.team)) });
for (const side of battle.sides) {
  if (side.activeRequest && side.activeRequest.teamPreview) {
    battle.choose(side.id, 'team ' + side.pokemon.map((_, i) => i + 1).join(''));
  }
}
const errors = [];
for (const turn of job.turns || []) {
  for (const id of ['p1', 'p2']) {
    if (!battle.choose(id, turn[id])) errors.push(`turn ${battle.turn} ${id}: ${turn[id]}`);
  }
}
if (errors.length) throw new Error('PS rejected: ' + errors.join('; '));
if (battle.ended) throw new Error('battle ended');

const choosable = (mon) => mon && !mon.fainted && !mon.volatiles['commanding'];
// Fill the slots before `slot` (fainted / commanding ones auto-pass in
// getChoiceIndex) with any accepted move.
function prefill(side, slot) {
  side.clearChoice();
  while (side.getChoiceIndex() < slot) {
    const before = side.choice.actions.length;
    let ok = false;
    for (const loc of [1, 2, 0, -1, -2]) {
      if (side.chooseMove(1, loc) !== false) { ok = true; break; }
      side.choice.error = '';
    }
    if (!ok || side.choice.actions.length === before) throw new Error(`no prefill for ${side.id} slot ${slot}`);
  }
}

const out = {};
for (const side of battle.sides) {
  out[side.id] = side.active.map((mon, slot) => {
    if (!choosable(mon)) return null;
    const cmds = [];
    mon.moveSlots.forEach((_, k) => {
      for (const loc of [0, 1, 2, -1, -2]) {
        for (const ev of ['', 'terastallize', 'mega']) {
          prefill(side, slot);
          if (side.chooseMove(k + 1, loc, ev) !== false) {
            cmds.push(`move ${k + 1}${loc ? ' ' + loc : ''}${ev ? ' ' + ev : ''}`);
          }
        }
      }
    });
    side.clearChoice();
    return cmds.sort();
  });
}
process.stdout.write(JSON.stringify(out, null, 2) + '\n');
