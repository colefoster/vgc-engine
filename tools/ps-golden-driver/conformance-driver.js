#!/usr/bin/env node
// PS Conformance Driver
//
// Runs a Pokemon Showdown battle with RANDOM-but-RECORDED choices and emits
// the JSON the vgc-engine conformance runner (crates/vgc-engine-conformance)
// consumes to build its keyed-outcome oracle (Rng::OracleKeyed).
//
// This is the "record" half of the keyed harness described in
//   docs/ps-comparison-harness-design.md  and
//   docs/conformance-key-contract.md  (the authoritative cross-language key).
//
// It is a SIBLING of driver.js (the strict golden driver), not a replacement.
// It reuses driver.js's RNG patch idea + random picker, and ADDS:
//   * a per-draw SEMANTIC ENVELOPE  {turn, actor, target, move, decision, value}
//     where `decision` is derived from the PS call SITE (stack frame), not just
//     the random/randomChance signature — see the site->decision table below;
//   * the RESOLVED CHOICE STRINGS per turn per side (verbatim, incl. forced
//     switches) so the engine can replay the exact same sequence;
//   * per-turn STATE (hp/maxhp/fainted/status of every active mon), snapshotted
//     from the live Battle object at each turn boundary.
//
// ---------------------------------------------------------------------------
// site -> decision mapping (runtime-verified against PS dist; .ts citations)
// ---------------------------------------------------------------------------
//   damage     Battle.randomizer            random(16)            raw 0..15
//                sim/battle.ts:2406  `tr(tr(base*(100-this.random(16)))/100)`
//   crit       BattleActions.getDamage      randomChance(1,critMult)  bool
//                sim/battle-actions.ts:1645 critMult gen9 = [0,24,8,2,1]
//   accuracy   BattleActions.hitStepAccuracy / hitStepMoveHitLoop
//                sim/battle-actions.ts:733 / :933  randomChance(acc,100)  BOOL ONLY
//   secondary  BattleActions.secondaries / selfDrops   random(100)   raw 0..99
//                sim/battle-actions.ts:1364 / :1346
//   range      BattleActions.hitStepMoveHitLoop (multihit) + duration callbacks
//                sim/battle-actions.ts:867/873/876, data/conditions.ts:*  raw
//   tiebreak   Battle.speedSort -> PRNG.shuffle   (BYPASSES Battle.random!)
//                sim/battle.ts:455 -> sim/prng.ts:150  -- see gotchas
//
// NO double-capture: Battle.randomChance -> PRNG.randomChance -> PRNG.random
// (the PRNG instance), never re-entering Battle.prototype.random. So patching
// both Battle.prototype.{random,randomChance} yields exactly one event per
// logical decision.
//
// Output JSON (stdout, and written to a file with --out):
//   {
//     "format": "gen9customgame",
//     "seed": [1,2,3,4],
//     "p1team": "<PS export text>",
//     "p2team": "<PS export text>",
//     "turns": [
//       { "turn": 1,
//         "choices": { "p1": ["move 1"], "p2": ["move 1"] },
//         "draws": [ {turn,actor,target,move,decision,value,raw_is_bool?}, ... ],
//         "state": { "p1a": {hp,maxhp,fainted,status}, ... } }
//     ]
//   }
//
// Usage:
//   node conformance-driver.js                 # built-in demo battle -> stdout
//   node conformance-driver.js --out out.json  # demo battle -> file
//   node conformance-driver.js job.json        # custom job {seed,format,p1,p2,max_turns}

'use strict';

const fs = require('fs');

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
const ps = require(PS_PATH);
const { BattleStream, Teams, getPlayerStreams } = ps;
const Battle = require(PS_PATH + '/battle').Battle;

// --- stack-site capture (same scheme as driver.js) -----------------------

function captureSite() {
  const raw = new Error().stack || '';
  const lines = raw.split('\n');
  for (const line of lines) {
    if (!line.includes(' at ')) continue;
    if (line.includes('captureSite')) continue;
    if (line.includes('Battle.random')) continue;
    if (line.includes('Battle.randomChance')) continue;
    if (line.includes('prng.ts')) continue;
    if (line.includes('prng.js')) continue;
    const m = line.match(/at\s+([^\s(]+)(?:\s+\(([^)]+)\))?/);
    if (!m) continue;
    const fn = m[1];
    let loc = m[2] || '';
    loc = loc.replace(/^.*?\/(?:dist\/)?sim\//, 'sim/')
             .replace(/^.*?\/(?:dist\/)?data\//, 'data/')
             .replace(/:[0-9]+$/, '');
    return loc ? `${fn} (${loc})` : fn;
  }
  return '<unknown>';
}

// --- decision classification (THE CRUX) ----------------------------------
//
// Map a single Battle.random / Battle.randomChance draw to one of the six
// conformance-key decision categories, using the SEMANTIC call site first
// and the call signature as a tiebreaker. Returns null for draws we do NOT
// want to record as a keyed decision (none currently; kept for future
// filtering, e.g. team-gen draws before the battle starts).

function classifyDraw(isChance, sig, site) {
  const s = site || '';

  // ---- damage roll: Battle.randomizer, random(16) ----
  if (!isChance && sig.m === 16 && sig.n === undefined && /randomizer/.test(s)) {
    return 'damage';
  }
  // ---- crit: BattleActions.getDamage, randomChance(1, critMult) ----
  if (isChance && /getDamage/.test(s)) return 'crit';
  // ---- accuracy: hitStepAccuracy / hitStepMoveHitLoop, randomChance(acc,100) ----
  if (isChance && sig.denom === 100 && /hitStepAccuracy|hitStepMoveHitLoop/.test(s)) {
    return 'accuracy';
  }
  // ---- secondary: secondaries / selfDrops, raw random(100) ----
  if (!isChance && /secondaries|selfDrops/.test(s)) return 'secondary';
  // ---- tiebreak: no-arg random() or speedSort frame ----
  if (!isChance && sig.m === undefined) return 'tiebreak';
  if (/speedSort/.test(s)) return 'tiebreak';

  // ---- signature fallbacks (site frame ambiguous / not in the table) ----
  if (isChance && sig.num === 1 && [24, 16, 8, 4, 3, 2, 1].includes(sig.denom)) return 'crit';
  if (isChance && sig.denom === 100) return 'secondary'; // ability/item proc (Static, Flame Body...)
  if (!isChance && sig.m === 16) return 'damage';
  if (!isChance && sig.m === 100) return 'secondary';
  return 'range';
}

// Slot ref ("p1a"/"p2b") of a live Pokemon, or null.
function slotRef(p) {
  if (!p || !p.side || typeof p.getSlot !== 'function') return null;
  try { return p.getSlot(); } catch (_) { return null; }
}

// Patch Battle.random / Battle.randomChance to push a full semantic envelope
// per draw. `draws` accumulates {turn, actor, target, move, decision, value, ...}.
function patchRng(draws) {
  const origRandom = Battle.prototype.random;
  const origRandomChance = Battle.prototype.randomChance;

  const envelope = function (battle, decision, value, rawIsBool) {
    const move = battle.activeMove ? (battle.activeMove.id || null) : null;
    const actor = slotRef(battle.activePokemon);
    // target: the resolved target slot, but null for self-target / field.
    let target = null;
    const at = battle.activeTarget;
    if (at && at !== battle.activePokemon) target = slotRef(at);
    const ev = {
      turn: battle.turn,
      actor,
      target,
      move,
      decision,
      value,
    };
    if (decision === 'accuracy' || decision === 'secondary') ev.raw_is_bool = !!rawIsBool;
    return ev;
  };

  Battle.prototype.random = function (m, n) {
    const v = origRandom.call(this, m, n);
    const site = captureSite();
    const decision = classifyDraw(false, { m, n }, site);
    // value: damage 0..15 raw; secondary raw 0..99; range raw int; tiebreak raw.
    draws.push(envelope(this, decision, v, false));
    return v;
  };

  Battle.prototype.randomChance = function (numerator, denominator) {
    const v = origRandomChance.call(this, numerator, denominator);
    const site = captureSite();
    const decision = classifyDraw(true, { num: numerator, denom: denominator }, site);
    // randomChance only exposes the BOOL (crit + accuracy + ability procs).
    draws.push(envelope(this, decision, v, true));
    return v;
  };

  return () => {
    Battle.prototype.random = origRandom;
    Battle.prototype.randomChance = origRandomChance;
  };
}

// --- per-turn state snapshot ---------------------------------------------

function snapshotState(battle) {
  const out = {};
  if (!battle || !battle.sides) return out;
  for (const side of battle.sides) {
    if (!side || !side.active) continue;
    for (const p of side.active) {
      if (!p) continue;
      const ref = slotRef(p);
      if (!ref) continue;
      out[ref] = {
        hp: p.hp,
        maxhp: p.maxhp,
        fainted: !!p.fainted,
        status: p.status || null,
      };
    }
  }
  return out;
}

// --- random picker (mirrors driver.js / random-player-ai.ts) -------------

function mulberry32(seed) {
  let a = (seed >>> 0) || 1;
  return function () {
    a = (a + 0x6D2B79F5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function makeRandomPicker(rand) {
  const randInt = (n) => Math.floor(rand() * n);
  const sample = (arr) => arr[randInt(arr.length)];

  return function picker(req) {
    if (req.wait) return null;

    if (req.forceSwitch) {
      const pokemon = req.side.pokemon;
      const chosen = [];
      const choices = req.forceSwitch.map((mustSwitch, i) => {
        if (!mustSwitch) return 'pass';
        const canSwitch = [];
        for (let j = 1; j <= 6; j++) {
          const p = pokemon[j - 1];
          if (!p) continue;
          if (j <= req.forceSwitch.length && !pokemon[i].reviving) continue;
          if (chosen.includes(j)) continue;
          const isFnt = p.condition.endsWith(' fnt');
          if ((!isFnt) !== (!pokemon[i].reviving)) continue;
          canSwitch.push(j);
        }
        if (!canSwitch.length) return 'pass';
        const target = sample(canSwitch);
        chosen.push(target);
        return `switch ${target}`;
      });
      return choices.join(', ');
    }

    if (req.active) {
      const pokemon = req.side.pokemon;
      const chosen = [];
      const choices = req.active.map((active, i) => {
        if (pokemon[i].condition.endsWith(' fnt') || pokemon[i].commanding) return 'pass';

        const possibleMoves = active.moves || [];
        const canMove = [];
        for (let j = 1; j <= possibleMoves.length; j++) {
          if (possibleMoves[j - 1].disabled) continue;
          canMove.push({
            slot: j,
            move: possibleMoves[j - 1].move,
            target: possibleMoves[j - 1].target,
          });
        }
        const hasAlly = pokemon.length > 1 && pokemon[i ^ 1] &&
          !pokemon[i ^ 1].condition.endsWith(' fnt');
        const filtered = canMove.filter((m) => m.target !== 'adjacentAlly' || hasAlly);
        const movesList = filtered.length ? filtered : canMove;

        const moves = movesList.map((m) => {
          let move = `move ${m.slot}`;
          if (req.active.length > 1) {
            if (['normal', 'any', 'adjacentFoe'].includes(m.target)) {
              move += ` ${1 + randInt(2)}`;
            } else if (m.target === 'adjacentAlly') {
              move += ` -${(i ^ 1) + 1}`;
            } else if (m.target === 'adjacentAllyOrSelf') {
              if (hasAlly) move += ` -${1 + randInt(2)}`;
              else move += ` -${i + 1}`;
            }
          }
          return move;
        });

        const canSwitch = [];
        for (let j = 1; j <= 6; j++) {
          const p = pokemon[j - 1];
          if (!p) continue;
          if (p.active) continue;
          if (chosen.includes(j)) continue;
          if (p.condition.endsWith(' fnt')) continue;
          canSwitch.push(j);
        }
        const switches = active.trapped ? [] : canSwitch;

        const moveRate = 0.8;
        if (switches.length && (!moves.length || rand() > moveRate)) {
          const target = sample(switches);
          chosen.push(target);
          return `switch ${target}`;
        }
        if (moves.length) {
          return sample(moves);
        }
        return 'pass';
      });
      return choices.join(', ');
    }

    return null;
  };
}

// --- side driver that RECORDS each resolved choice -----------------------
//
// Tags each choice with the turn it applies to via a per-side counter:
// an `active` (move) request opens a new turn for that side (increment);
// a `forceSwitch` request is a replacement belonging to the turn that just
// resolved (no increment). This is robust against the async timing of the
// concurrent omniscient stream.

async function driveSide(playerStream, teamLen, sideTag, picker, errorsOut, recordChoice) {
  for await (const chunk of playerStream) {
    for (const l of chunk.split('\n')) {
      if (l.startsWith('|error|')) errorsOut.push(`[${sideTag}] ${l}`);
    }
    const reqLine = chunk.split('\n').find((l) => l.startsWith('|request|'));
    if (!reqLine) continue;
    let req;
    try { req = JSON.parse(reqLine.slice('|request|'.length)); } catch (_) { continue; }
    if (!req) continue;

    if (req.teamPreview) {
      const order = Array.from({ length: teamLen }, (_, i) => i + 1).join('');
      playerStream.write(`team ${order}`);
      continue;
    }
    if (req.wait) continue;
    if (req.forceSwitch || req.active) {
      const cmd = picker(req);
      if (cmd == null) break;
      recordChoice(sideTag, req, cmd);
      playerStream.write(cmd);
      continue;
    }
  }
}

// --- job runner ----------------------------------------------------------

async function runJob(job) {
  const seed = job.seed || [1, 2, 3, 4];
  const format = job.format || 'gen9customgame';
  if (!job.p1 || !job.p2) throw new Error('job needs p1.team and p2.team');
  const p1team = job.p1.team;
  const p2team = job.p2.team;
  const team1 = Teams.import(p1team);
  const team2 = Teams.import(p2team);
  if (!Array.isArray(team1) || team1.length === 0) throw new Error('p1.team failed to parse');
  if (!Array.isArray(team2) || team2.length === 0) throw new Error('p2.team failed to parse');

  const maxTurns = job.max_turns || 20;

  const p1Picker = makeRandomPicker(mulberry32(((seed[0] || 1) * 2654435761) >>> 0));
  const p2Picker = makeRandomPicker(mulberry32(((seed[0] || 1) * 1597334677) >>> 0));

  const draws = [];
  const restore = patchRng(draws);

  const sideErrors = [];

  // Resolved-choice recording (per side, tagged to turn).
  const sideTurn = { p1: 0, p2: 0 };
  const choiceLog = []; // {side, turn, cmd, kind}
  const recordChoice = (side, req, cmd) => {
    if (req.active && !req.forceSwitch) sideTurn[side] += 1;
    const turn = Math.max(1, sideTurn[side]);
    choiceLog.push({ side, turn, cmd, kind: req.forceSwitch ? 'switch' : 'move' });
  };

  const stream = new BattleStream();
  const sides = getPlayerStreams(stream);
  const logChunks = [];
  let currentTurn = 0;
  const stateByTurn = {}; // turn -> snapshot (end-of-turn)

  const drainOmni = (async () => {
    for await (const chunk of sides.omniscient) {
      logChunks.push(chunk);
      for (const l of chunk.split('\n')) {
        if (l.startsWith('|turn|')) {
          const n = parseInt(l.slice('|turn|'.length), 10);
          if (Number.isFinite(n)) {
            // |turn|N is printed once turn N-1 has fully resolved and PS is
            // paused awaiting input, so the live battle reflects end-of-(N-1).
            if (n > 1) stateByTurn[n - 1] = snapshotState(stream.battle);
            currentTurn = n;
            if (currentTurn > maxTurns) {
              try { sides.omniscient.write('>forcetie'); } catch (_) {}
            }
          }
        }
      }
    }
  })();

  try {
    const driveP1 = driveSide(sides.p1, team1.length, 'p1', p1Picker, sideErrors, recordChoice);
    const driveP2 = driveSide(sides.p2, team2.length, 'p2', p2Picker, sideErrors, recordChoice);

    sides.omniscient.write('>start ' + JSON.stringify({ formatid: format, seed }));
    sides.omniscient.write('>player p1 ' + JSON.stringify({ name: 'P1', team: Teams.pack(team1) }));
    sides.omniscient.write('>player p2 ' + JSON.stringify({ name: 'P2', team: Teams.pack(team2) }));

    const timeoutMs = 60_000;
    const timeoutSym = Symbol('timeout');
    const safeP1 = driveP1.catch(() => {});
    const safeP2 = driveP2.catch(() => {});
    await Promise.race([
      Promise.all([safeP1, safeP2]),
      new Promise((res) => setTimeout(() => res(timeoutSym), timeoutMs)),
    ]);
    sides.omniscient.writeEnd();
    await Promise.race([drainOmni, new Promise((res) => setTimeout(res, 2_000))]);
  } finally {
    restore();
  }

  // Final snapshot: end of the last turn that actually played.
  stateByTurn[currentTurn] = snapshotState(stream.battle);

  // --- assemble the per-turn output -------------------------------------
  const drawsByTurn = {};
  for (const d of draws) {
    const t = d.turn || 0;
    (drawsByTurn[t] = drawsByTurn[t] || []).push(d);
  }
  const choicesByTurn = {};
  for (const c of choiceLog) {
    const t = choicesByTurn[c.turn] = choicesByTurn[c.turn] || { p1: [], p2: [] };
    t[c.side].push(c.cmd);
  }

  const turnNums = new Set();
  for (const k of Object.keys(drawsByTurn)) turnNums.add(parseInt(k, 10));
  for (const k of Object.keys(choicesByTurn)) turnNums.add(parseInt(k, 10));
  for (const k of Object.keys(stateByTurn)) turnNums.add(parseInt(k, 10));
  turnNums.delete(0); // pre-turn / team-preview draws (none expected)

  const turns = [...turnNums].sort((a, b) => a - b).map((t) => ({
    turn: t,
    choices: choicesByTurn[t] || { p1: [], p2: [] },
    draws: drawsByTurn[t] || [],
    state: stateByTurn[t] || {},
  })).filter((t) =>
    // Drop phantom trailing turns: the forcetie/battle-end turn records the
    // picker's choices but produces no draws and no live state.
    t.draws.length > 0 || Object.keys(t.state).length > 0
  );

  return {
    format,
    seed,
    p1team,
    p2team,
    turns,
    // diagnostics (not part of the strict contract shape, but handy):
    _meta: {
      ok: sideErrors.length === 0,
      lastTurn: currentTurn,
      totalDraws: draws.length,
      errors: sideErrors,
    },
  };
}

// --- built-in demo battle ------------------------------------------------
//
// Phase 0 tracer: a fast physical attacker (Crunch -> 20% Def-drop secondary)
// vs a very physically bulky wall using Splash (no RNG on its side). Single
// damaging move with a secondary; the wall survives so no faints/switches.

const DEMO_JOB = {
  seed: [1, 2, 3, 4],
  format: 'gen9customgame',
  max_turns: 4,
  p1: {
    team: [
      'Weavile',
      'Ability: Pressure',
      'Level: 100',
      'EVs: 252 Atk / 4 Def / 252 Spe',
      'Adamant Nature',
      '- Crunch',
    ].join('\n'),
  },
  p2: {
    team: [
      'Garganacl @ Leftovers',
      'Ability: Purifying Salt',
      'Level: 100',
      'EVs: 252 HP / 252 Def / 4 SpD',
      'Bold Nature',
      '- Splash',
    ].join('\n'),
  },
};

// --- main ----------------------------------------------------------------

function parseArgs(argv) {
  const out = { out: null, jobPath: null };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--out') { out.out = argv[++i]; }
    else if (a.startsWith('--out=')) { out.out = a.slice('--out='.length); }
    else if (!a.startsWith('-')) { out.jobPath = a; }
  }
  return out;
}

(async () => {
  try {
    const { out, jobPath } = parseArgs(process.argv);
    const job = jobPath ? JSON.parse(fs.readFileSync(jobPath, 'utf8')) : DEMO_JOB;
    const result = await runJob(job);
    const json = JSON.stringify(result, null, 2);
    if (out) {
      fs.writeFileSync(out, json);
      process.stderr.write(`wrote ${out} (${result.turns.length} turns, ${result._meta.totalDraws} draws)\n`);
    } else {
      process.stdout.write(json);
    }
    if (!result._meta.ok) process.stderr.write(`WARN side errors: ${JSON.stringify(result._meta.errors)}\n`);
  } catch (err) {
    process.stderr.write(String((err && err.stack) || err) + '\n');
    process.exit(1);
  }
})();
