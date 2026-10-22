#!/usr/bin/env node
// PS move-target oracle for doubles legal-choice parity.
//
// Builds the golden input's battle at turn 1 and, for each side and active
// slot, offers PS every `move N [T] [terastallize]` with T in
// {none, 1, 2, -1, -2}. A choice is kept when PS's own Side.chooseMove
// accepts it (sim/side.ts:659-672: targetTypeChoices + validTargetLoc,
// sim/battle.ts:2399-2429). Prints {"p1": [[slot a cmds], [slot b cmds]],
// "p2": ...}, each list sorted. The engine side is
// crates/vgc-engine-golden/tests/ally_target_choices.rs (flag on).
//
// Usage: node target-choices.js <input.json> > <choices.json>

'use strict';
const fs = require('fs');
const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
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

const out = {};
for (const side of battle.sides) {
  const slots = side.active.map(() => []);
  // One valid slot-a choice so slot b's chooseMove lands on index 1.
  const accepted = (slot, n, loc, tera) => {
    side.clearChoice();
    if (slot === 1 && !side.chooseMove(1, 0) && !side.chooseMove(1, 1) && !side.chooseMove(1, 2)) {
      throw new Error(`no slot-a choice for ${side.id}`);
    }
    return side.chooseMove(n, loc, tera ? 'terastallize' : '') !== false;
  };
  side.active.forEach((mon, slot) => {
    mon.moveSlots.forEach((_, k) => {
      for (const loc of [0, 1, 2, -1, -2]) {
        for (const tera of [false, true]) {
          if (accepted(slot, k + 1, loc, tera)) {
            slots[slot].push(`move ${k + 1}${loc ? ' ' + loc : ''}${tera ? ' terastallize' : ''}`);
          }
        }
      }
    });
    slots[slot].sort();
  });
  side.clearChoice();
  out[side.id] = slots;
}
process.stdout.write(JSON.stringify(out, null, 2) + '\n');
