// ps-battle.js's turn bound: with a finite maxTurns the output holds exactly
// the turns PS played. Run:
//   node --test tools/accuracy/turn-limit.test.js
// Needs PS_DIST (a built pokemon-showdown dist/sim); skipped without it.

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
const job = (id, p1team, p2team, script) => ({
  id, format: 'gen9championsvgc2026regmc', seed: 'sodium,00000000000000000000000000000001',
  p1team, p2team, intents: {}, script, meta: { replay: id },
});

// Each turn both Chansey Seismic Toss the other (fixed 50 damage at L50)
// while the Snorlax Protect: every turn's end HP is known, nobody faints.
// Scripted past every bound used, so an unplayed turn would still record.
const QUIET_TURN = { p1: 'move 1 1, move 1', p2: 'move 1 1, move 1' };
const QUIET = job(
  '0a1e7000',
  team(set('Chansey', 'Natural Cure', ['Seismic Toss']), set('Snorlax', 'Thick Fat', ['Protect']),
    set('Garchomp', 'Rough Skin', ['Protect']), set('Incineroar', 'Blaze', ['Protect'])),
  team(set('Chansey', 'Natural Cure', ['Seismic Toss']), set('Snorlax', 'Thick Fat', ['Protect']),
    set('Whimsicott', 'Prankster', ['Protect']), set('Amoonguss', 'Regenerator', ['Protect'])),
  { 1: QUIET_TURN, 2: QUIET_TURN, 3: QUIET_TURN, 4: QUIET_TURN },
);

// P1 Protects; P2's only two mons Explode: P1 wins on turn 1.
const EARLY = job(
  '0e5d1000',
  team(set('Snorlax', 'Thick Fat', ['Protect']), set('Chansey', 'Natural Cure', ['Protect']),
    set('Garchomp', 'Rough Skin', ['Protect']), set('Incineroar', 'Blaze', ['Protect'])),
  team(set('Electrode', 'Static', ['Explosion']), set('Electrode', 'Static', ['Explosion'])),
  { 1: { p1: 'move 1, move 1', p2: 'move 1, move 1' }, 2: { p1: 'move 1, move 1', p2: 'move 1, move 1' } },
);

// Protocol lines after the first `|turn|n` line.
const linesAfterTurn = (log, n) => {
  const lines = log.split('\n');
  return lines.slice(lines.indexOf(`|turn|${n}`) + 1);
};

for (const limit of [1, 2]) {
  test(`maxTurns ${limit}: only played turns, last state is the end of turn ${limit}`, { skip }, async () => {
    const { runBattle } = require('./ps-battle.js');
    const res = await runBattle(QUIET, limit);
    assert.deepEqual(res._meta.errors, []);

    // PS played `limit` turns, then the forcetie ended it at the next marker.
    const log = res._meta.log;
    assert.equal(log.split('\n').filter((l) => l.startsWith('|move|')).length, 4 * limit);
    const tail = linesAfterTurn(log, limit + 1).filter((l) => l.startsWith('|'));
    assert.ok(tail.length > 0, `no |turn|${limit + 1} marker`);
    assert.ok(!tail.some((l) => /^\|(move|switch|-damage)\|/.test(l)), 'turn after the bound was played');
    assert.ok(tail.includes('|tie'));
    assert.equal(res._meta.ended, true);
    // lastTurn is the cutoff marker PS printed, not a played turn.
    assert.equal(res._meta.lastTurn, limit + 1);

    // Output: exactly the played turns, their commands, their end state.
    assert.deepEqual(res.turns.map((t) => t.turn), Array.from({ length: limit }, (_, i) => i + 1));
    for (const t of res.turns) {
      assert.deepEqual(t.choices, { p1: [QUIET_TURN.p1], p2: [QUIET_TURN.p2] });
      assert.equal(t.has_state, true);
      for (const ref of ['p1a', 'p2a']) {
        assert.equal(t.state[ref].species, 'chansey');
        assert.equal(t.state[ref].hp, t.state[ref].maxhp - 50 * t.turn, `${ref} hp after turn ${t.turn}`);
      }
    }
  });
}

for (const limit of [1, 3]) {
  test(`maxTurns ${limit}: a battle won on turn 1 keeps its final turn`, { skip }, async () => {
    const { runBattle } = require('./ps-battle.js');
    const res = await runBattle(EARLY, limit);
    assert.deepEqual(res._meta.errors, []);
    const log = res._meta.log.split('\n');
    assert.ok(log.includes('|win|P1'));
    assert.ok(!log.includes('|turn|2') && !log.includes('|tie'));
    assert.equal(res._meta.ended, true);
    assert.equal(res._meta.lastTurn, 1);

    assert.deepEqual(res.turns.map((t) => t.turn), [1]);
    const [t1] = res.turns;
    assert.deepEqual(t1.choices, { p1: ['move 1, move 1'], p2: ['move 1, move 1'] });
    assert.equal(t1.state.p2a.fainted, true);
    assert.equal(t1.state.p2b.fainted, true);
    assert.equal(t1.state.p1a.hp, t1.state.p1a.maxhp);
  });
}
