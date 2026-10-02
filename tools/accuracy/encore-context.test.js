// Keyed context of the target roll Champions Encore causes
// (tools/ps-golden-driver/conformance-driver.js patchRng). Run:
//   node --test tools/accuracy/encore-context.test.js
// Needs PS_DIST (a built pokemon-showdown dist/sim); skipped without it.
//
// data/mods/champions/moves.ts:307-339 encore onStart: when the target's
// queued move differs from the Encored one, `queue.changeAction(target,
// {choice: 'move', moveid})` re-queues it with no target, and
// sim/battle-queue.ts:268-275 resolveAction picks one with
// `battle.getRandomTarget(action.pokemon, action.move)` — while
// battle.activePokemon / activeMove are still the Encore user and Encore.
// That roll belongs to the Encored mon's forced move.

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');

const PS_DIST = process.env.PS_DIST;
const skip = !(PS_DIST && fs.existsSync(path.join(PS_DIST, 'index.js'))) && 'PS_DIST not set';

const set = (species, ability, moves) =>
  `${species}\nAbility: ${ability}\nLevel: 50\n${moves.map((m) => `- ${m}`).join('\n')}`;
const team = (...sets) => sets.join('\n\n');
const BENCH = [set('Chansey', 'Natural Cure', ['Splash']), set('Blissey', 'Natural Cure', ['Splash'])];

// Turn 1: p2a Snorlax Tackles (its lastMove). Turn 2: Prankster Whimsicott
// Encores it while it queued Splash, so PS swaps in a targetless Tackle.
const job = (seed) => ({
  id: `e0c0${seed}000`,
  format: 'gen9championsvgc2026regmc',
  seed: `sodium,${String(seed).padStart(32, '0')}`,
  p1team: team(set('Whimsicott', 'Prankster', ['Encore', 'Splash']), set('Snorlax', 'Thick Fat', ['Tackle', 'Splash']), ...BENCH),
  p2team: team(set('Snorlax', 'Thick Fat', ['Tackle', 'Splash']), set('Miltank', 'Sap Sipper', ['Splash']), ...BENCH),
  intents: {},
  script: {
    1: { p1: 'move 2, move 1 1', p2: 'move 1 1, move 1' },
    2: { p1: 'move 1 1, move 2', p2: 'move 2, move 1' },
  },
  meta: { replay: `encore-context-${seed}` },
});

// ps-battle.js raw-trace sites of the retarget roll on PS a5df8274's build.
const RETARGET_SITES = [
  'Side.randomFoe@side.js:191',
  'Battle.getRandomTarget@battle.js:2064',
  'BattleQueue.resolveAction@battle-queue.js:168',
  'BattleQueue.insertChoice@battle-queue.js:269',
  'BattleQueue.changeAction@battle-queue.js:200',
  'Battle.onStart@data/mods/champions/moves.js:351',
];

const isRetarget = (d) => d.decision === 'range' && d.actor === 'p2a' && d.target === null && d.move === 'tackle';

test('Encore retarget roll is keyed to the Encored mon and its forced move', { skip }, async () => {
  const { runBattle } = require('./ps-battle.js');
  const picked = new Set();
  for (const seed of [1, 2, 3, 4]) {
    const res = await runBattle(job(seed), 2);
    assert.deepEqual(res._meta.errors, [], `seed ${seed}`);
    assert.deepEqual(res.turns.map((t) => t.turn), [1, 2], `seed ${seed}`);
    const [t1, t2] = res.turns;
    assert.deepEqual(t2.choices, { p1: ['move 1 1, move 2'], p2: ['move 2, move 1'] }, `seed ${seed}`);

    // PS: Encore landed and p2a used Tackle on a random foe.
    const log = res._meta.log.split('\n');
    const start = log.indexOf('|turn|2');
    const tackle = log.slice(start).find((l) => l.startsWith('|move|p2a: Snorlax|Tackle|'));
    assert.ok(tackle, `seed ${seed}: no Encored Tackle`);
    const hit = tackle.split('|')[4].slice(0, 3);

    // The raw trace's getRandomTarget draw is the one roll keyed to p2a's
    // Tackle, with no target, and its value picks the foe that was hit.
    const rawTarget = t2.raw.filter((r) => r.site.some((s) => s.startsWith('Battle.getRandomTarget@')));
    assert.equal(rawTarget.length, 1, `seed ${seed}: getRandomTarget draws`);
    // Its semantic call sites are PS's own stack (pinned dist), with no
    // recorder frame displacing Encore's onStart.
    assert.deepEqual(rawTarget[0].site, RETARGET_SITES, `seed ${seed}`);
    const keyed = t2.draws.filter(isRetarget);
    assert.equal(keyed.length, 1, `seed ${seed}: ${JSON.stringify(t2.draws)}`);
    assert.equal(keyed[0].value, rawTarget[0].result, `seed ${seed}`);
    assert.equal(['p1a', 'p1b'][keyed[0].value], hit, `seed ${seed}: value vs ${tackle}`);
    picked.add(hit);

    // Encore's own draws keep Encore's context: its accuracy roll, and the
    // queue-insertion tie roll (insertChoice) as the only Encore range.
    const encore = t2.draws.filter((d) => d.actor === 'p1a' && d.move === 'encore');
    assert.ok(encore.some((d) => d.decision === 'accuracy' && d.target === 'p2a'), `seed ${seed}: Encore accuracy`);
    assert.equal(encore.filter((d) => d.decision === 'range').length, 1, `seed ${seed}: ${JSON.stringify(encore)}`);

    // Control: an ordinary chosen-target move keeps the active-move context.
    assert.ok(t1.draws.some((d) => d.decision === 'damage' && d.actor === 'p1b' && d.target === 'p2a' && d.move === 'tackle'), `seed ${seed}`);
    assert.ok(!t1.draws.some(isRetarget), `seed ${seed}: turn-1 relabelled`);

    // End state: the Encored mon's Tackle hit the foe PS logged.
    assert.ok(t2.state[hit].hp < t2.state[hit].maxhp, `seed ${seed}: ${hit} undamaged`);
  }
  assert.deepEqual([...picked].sort(), ['p1a', 'p1b'], 'both foes picked across seeds');
});

test('patchRng restores getRandomTarget and its scoped context, even on a throw', { skip }, () => {
  const conf = require('../ps-golden-driver/conformance-driver.js');
  const { Battle } = require(PS_DIST + '/battle');
  const before = {
    getRandomTarget: Battle.prototype.getRandomTarget,
    random: Battle.prototype.random,
    randomChance: Battle.prototype.randomChance,
  };

  const draws = [];
  const restore = conf.patchRng(draws);
  try {
    assert.notEqual(Battle.prototype.getRandomTarget, before.getRandomTarget);
    // An Encore-shaped context whose original getRandomTarget throws (no
    // dex on the fake battle): the error propagates and the scope unwinds.
    const holder = { side: {}, getSlot: () => 'p9z' };
    const fake = {
      effect: { id: 'encore', effectType: 'Condition' },
      effectState: { id: 'encore', move: 'tackle', target: holder },
    };
    assert.throws(() => Battle.prototype.getRandomTarget.call(fake, holder, 'tackle'));
    const b = new Battle({ formatid: 'gen9customgame', seed: [1, 2, 3, 4] });
    b.random(5);
    const last = draws[draws.length - 1];
    assert.equal(last.actor, null, JSON.stringify(last));
    assert.equal(last.move, null, JSON.stringify(last));
  } finally {
    restore();
  }
  assert.equal(Battle.prototype.getRandomTarget, before.getRandomTarget);
  assert.equal(Battle.prototype.random, before.random);
  assert.equal(Battle.prototype.randomChance, before.randomChance);
});
