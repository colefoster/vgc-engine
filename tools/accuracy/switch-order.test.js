// Switch-target remapping from PS's current side order to the engine's
// original team order (ps-battle.js). Run:
//   node --test tools/accuracy/switch-order.test.js
// The integration case needs PS_DIST (a built pokemon-showdown dist/sim) and
// is skipped without it.

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');

const { captureRoster, toEngineCmd } = require('./switch-order.js');

const PS_DIST = process.env.PS_DIST;
const havePS = !!PS_DIST && fs.existsSync(path.join(PS_DIST, 'index.js'));

// --- pure: stand-in Pokemon objects -----------------------------------------

// Shaped like PS's Pokemon (fullname, details) and its switch request entry
// (ident, details); the request is built from the live order, as PS does.
const mon = (fullname, details) => ({ fullname, details });
const side = (...pokemon) => ({ pokemon });
const request = (current) => ({
  side: { pokemon: current.map((p) => ({ ident: p.fullname, details: p.details })) },
});
const map = (cmd, current, original) => toEngineCmd(cmd, request(current), current, original);

test('identical nicknames and details map by object, through permutations', () => {
  const a = mon('p1: Fin', 'Palafin, L50'), b = mon('p1: Fin', 'Palafin, L50');
  const c = mon('p1: Lax', 'Snorlax, L50'), d = mon('p1: Lax', 'Snorlax, L50');
  const s = side(a, b, c, d);
  const original = captureRoster(s);
  s.pokemon.reverse(); // e.g. team preview order 4321
  assert.deepEqual(original, [a, b, c, d]);
  assert.equal(map('switch 3', s.pokemon, original), 'switch 2');
  assert.equal(map('move 1, switch 4', s.pokemon, original), 'move 1, switch 1');
  assert.equal(map('switch 2, switch 1', s.pokemon, original), 'switch 3, switch 4');
});

test('a changed species or forme does not change identity', () => {
  const ditto = mon('p1: Ditto', 'Ditto, L50'), fin = mon('p1: Palafin', 'Palafin, L50');
  const hero = mon('p1: Palafin', 'Palafin-Hero, L50'), lax = mon('p1: Snorlax', 'Snorlax, L50');
  const s = side(ditto, fin, hero, lax);
  const original = captureRoster(s);
  ditto.species = 'snorlax'; // Transform: same object, request details unchanged
  fin.details = 'Palafin-Hero, L50'; // Zero to Hero: now identical to member 3
  s.pokemon = [lax, hero, fin, ditto];
  assert.equal(map('switch 4', s.pokemon, original), 'switch 1');
  assert.equal(map('switch 2', s.pokemon, original), 'switch 3');
  assert.equal(map('switch 3', s.pokemon, original), 'switch 2');
});

test('non-switch parts pass through unchanged', () => {
  const s = side(mon('p1: A', 'A'), mon('p1: B', 'B'));
  const original = captureRoster(s);
  assert.equal(map('move 1 -2 mega, pass', s.pokemon, original), 'move 1 -2 mega, pass');
  assert.equal(map('team 21', s.pokemon, original), 'team 21');
});

test('invalid targets throw instead of mapping silently', () => {
  const a = mon('p1: A', 'A'), b = mon('p1: B', 'B');
  const s = side(a, b);
  const original = captureRoster(s);
  assert.throws(() => map('switch 3', s.pokemon, original), /no member 3/);
  assert.throws(() => map('switch 0', s.pokemon, original), /no member 0/);
  // a member PS never had at `>player`
  const stranger = mon('p1: A', 'A');
  assert.throws(() => map('switch 1', [stranger, b], original), /original team/);
  // side order moved on after the request was built
  const req = request([a, b]);
  assert.throws(() => toEngineCmd('switch 1', req, [b, a], original), /request shows p1: A/);
  assert.throws(() => captureRoster(side()), /no pokemon/);
});

// --- integration: real PS battle -------------------------------------------

const set = (species, ability, moves) =>
  `${species}\nAbility: ${ability}\nLevel: 50\n${moves.map((m) => `- ${m}`).join('\n')}`;
const team = (sets) => sets.join('\n\n');

// P1 holds Palafin and Palafin-Hero (same base species, no Species Clause in
// the sim). Turn 1 sends Hero (slot b) out for Snorlax, which puts Hero at
// PS's current index 3; turn 2's identical `switch 3` brings Hero back, which
// is original team member 2. P2 has distinct species and does the same dance
// in slot a, so plain permutation mapping is covered too. Everyone Protects:
// no damage, no faints, no forced switches.
const JOB = {
  id: '5a1fe000',
  format: 'gen9championsvgc2026regmc',
  seed: 'sodium,00000000000000000000000000000001',
  p1team: team([
    set('Palafin', 'Zero to Hero', ['Protect', 'Jet Punch']),
    set('Palafin-Hero', 'Zero to Hero', ['Protect', 'Jet Punch']),
    set('Snorlax', 'Thick Fat', ['Protect', 'Body Slam']),
    set('Chansey', 'Natural Cure', ['Protect', 'Seismic Toss']),
  ]),
  p2team: team([
    set('Incineroar', 'Blaze', ['Protect', 'Flare Blitz']),
    set('Amoonguss', 'Regenerator', ['Protect', 'Spore']),
    set('Whimsicott', 'Prankster', ['Protect', 'Moonblast']),
    set('Garchomp', 'Rough Skin', ['Protect', 'Earthquake']),
  ]),
  intents: {},
  script: {
    1: { p1: 'move 1, switch 3', p2: 'switch 3, move 1' },
    2: { p1: 'move 1, switch 3', p2: 'switch 3, move 1' },
    3: { p1: 'move 1, switch 4', p2: 'move 1, switch 4' },
  },
  meta: { replay: 'synthetic-palafin-switch-order' },
};

test('PS battle: switch targets map to original roster members', { skip: !havePS && 'PS_DIST not set' }, async () => {
  const { runBattle } = require('./ps-battle.js');
  const res = await runBattle(JOB, 3);
  assert.deepEqual(res._meta.errors, []);
  const turn = (n) => res.turns.find((t) => t.turn === n);

  // PS really ran the scripted commands ...
  assert.equal(turn(1).state.p1b.species, 'snorlax');
  assert.equal(turn(2).state.p1b.species, 'palafinhero');
  assert.equal(turn(2).state.p2a.species, 'incineroar');
  assert.equal(turn(3).state.p1b.species, 'chansey');
  assert.equal(turn(3).state.p2b.species, 'garchomp');

  // ... and the engine commands name the same mons in original team order.
  assert.deepEqual(turn(1).choices, { p1: ['move 1, switch 3'], p2: ['switch 3, move 1'] });
  assert.deepEqual(turn(2).choices, { p1: ['move 1, switch 2'], p2: ['switch 1, move 1'] });
  assert.deepEqual(turn(3).choices, { p1: ['move 1, switch 4'], p2: ['move 1, switch 4'] });
});
