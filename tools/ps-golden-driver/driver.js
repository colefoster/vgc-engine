#!/usr/bin/env node
// PS Golden Driver
//
// Drives Pokemon Showdown's BattleStream under a fixed PRNG seed against
// a fully-specified scripted battle (teams in Showdown export text +
// per-turn action strings) and writes a JSON ground-truth log:
//
//   * `events`: protocol events with turn number, actor slot, raw HP
//     (omniscient stream reports `current/max` directly, not %).
//   * `rng`: RNG events captured by patching Battle.prototype.random /
//     randomChance — same scheme as tools/ps-rng-dump/dump.js.
//
// This is the PS-side ground truth the vgc-engine-golden harness diffs
// against. RNG events feed the engine's Rng::oracle_partial so the two
// sims draw the same crit/damage-roll/accuracy outcomes deterministically.
//
// Input schema (JSON file path as argv[2], or stdin):
//   {
//     "name":   "<test-name>",
//     "format": "gen9customgame",            // optional; default gen9customgame
//     "seed":   [1, 2, 3, 4],                // PS prng seed; default [1,2,3,4]
//     "p1":     { "team": "<showdown export text>" },
//     "p2":     { "team": "<showdown export text>" },
//     "turns":  [
//       { "p1": "move 1", "p2": "move 1" },     // strings or arrays
//       { "p1": "move 1", "p2": "switch 3" },
//       ...
//     ]
//   }
//
// Output schema (stdout):
//   {
//     "ok": true,
//     "name": "...",
//     "seed": [...],
//     "events": [
//       { "turn": 1, "kind": "move", "actor": "p1a", "name": "Earthquake",
//         "target": "p2a" },
//       { "turn": 1, "kind": "damage", "actor": "p2a", "hp": 87, "max": 281,
//         "from": null },
//       { "turn": 1, "kind": "crit", "actor": "p2a" },
//       { "turn": 1, "kind": "boost", "actor": "p1a", "stat": "atk", "amount": 1 },
//       { "turn": 1, "kind": "status", "actor": "p2a", "status": "brn" },
//       { "turn": 1, "kind": "faint", "actor": "p2a" },
//       { "turn": 2, "kind": "switch", "actor": "p2a", "species": "Amoonguss" },
//       ...
//     ],
//     "rng": [
//       { "kind": "PercentRoll", "value": false, "threshold": 30 },
//       { "kind": "Crit", "value": false },
//       { "kind": "DamageRoll", "value": 12 },
//       ...
//     ],
//     "log": "...PS protocol log..."
//   }
//
// PS source references:
//   * BattleStream / getPlayerStreams: sim/battle-stream.ts
//   * Battle.random / Battle.randomChance: sim/battle.ts
//   * `>start` / `>player` commands: sim/battle-stream.ts:128+
//
// Usage:
//   node driver.js path/to/input.json > out.ps.json
//   node driver.js < input.json > out.ps.json

'use strict';

const fs = require('fs');

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
const ps = require(PS_PATH);
const { BattleStream, Teams, getPlayerStreams } = ps;
const Battle = require(PS_PATH + '/battle').Battle;

async function readStdin() {
  let data = '';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) data += chunk;
  return data;
}

// --- RNG capture (matches tools/ps-rng-dump/dump.js semantics) -----------

function patchRng(events) {
  const origRandom = Battle.prototype.random;
  const origRandomChance = Battle.prototype.randomChance;
  Battle.prototype.random = function (m, n) {
    const v = origRandom.call(this, m, n);
    if (m === undefined && n === undefined) {
      events.push({ kind: 'Tiebreak', value: '0x' + v.toString(16) });
    } else if (m === 16 && n === undefined) {
      events.push({ kind: 'DamageRoll', value: v });
    } else if (n === undefined) {
      events.push({ kind: 'Range', value: v, bound: m });
    } else {
      events.push({ kind: 'Range', value: v - m, bound: n - m });
    }
    return v;
  };
  Battle.prototype.randomChance = function (numerator, denominator) {
    const v = origRandomChance.call(this, numerator, denominator);
    if (numerator === 1 && denominator === 24) {
      events.push({ kind: 'Crit', value: v });
    } else if (denominator === 100) {
      events.push({ kind: 'PercentRoll', value: v, threshold: numerator });
    } else {
      events.push({ kind: 'Chance', value: v, num: numerator, denom: denominator });
    }
    return v;
  };
  return () => {
    Battle.prototype.random = origRandom;
    Battle.prototype.randomChance = origRandomChance;
  };
}

// --- Omniscient-log parser ------------------------------------------------
//
// Parses the protocol output of the omniscient stream into a flat
// {turn, kind, actor, ...} event array. Omniscient lines carry raw HP
// values (`current/max`) rather than fractions, which is exactly what
// we need for differential checks.

function parseActor(s) {
  // "p1a: Garchomp" → "p1a"
  if (!s) return null;
  const colon = s.indexOf(':');
  return colon >= 0 ? s.slice(0, colon) : s;
}

function parseHpString(s) {
  // PS HP strings:
  //   "281/281"                    (alive)
  //   "281/281 brn"                (alive + status)
  //   "0 fnt"                      (fainted)
  if (!s) return { hp: null, max: null };
  const first = s.split(' ')[0];
  if (first === '0') return { hp: 0, max: null };
  const m = first.match(/^(\d+)\/(\d+)$/);
  if (!m) return { hp: null, max: null };
  return { hp: parseInt(m[1]), max: parseInt(m[2]) };
}

function parseFromTag(parts) {
  // PS frequently appends `|[from] <cause>` to events. Returns the
  // cause string or null. Used to tell direct damage from indirect
  // (residual / item / ability / status / weather).
  const idx = parts.indexOf('[from]', 3);
  if (idx < 0) return null;
  return parts[idx + 1] || null;
}

function parseLog(log) {
  const out = [];
  let turn = 0;
  for (const raw of log.split('\n')) {
    if (!raw.startsWith('|')) continue;
    const parts = raw.split('|');
    const kind = parts[1];
    switch (kind) {
      case 'turn':
        turn = parseInt(parts[2]) || turn;
        break;
      case 'move': {
        // |move|p1a: Garchomp|Earthquake|p2a: Amoonguss
        const actor = parseActor(parts[2]);
        const name = parts[3];
        const target = parts[4] ? parseActor(parts[4]) : null;
        out.push({ turn, kind: 'move', actor, name, target });
        break;
      }
      case 'switch':
      case 'drag': {
        // |switch|p1a: Garchomp|Garchomp, L50, M|281/281
        const actor = parseActor(parts[2]);
        const details = parts[3] || '';
        const species = details.split(',')[0].trim();
        const { hp, max } = parseHpString(parts[4] || '');
        out.push({ turn, kind: 'switch', actor, species, hp, max });
        break;
      }
      case '-damage': {
        // |-damage|p2a: Amoonguss|123/281|[from] item: Life Orb
        const actor = parseActor(parts[2]);
        const { hp, max } = parseHpString(parts[3] || '');
        const from = parseFromTag(parts);
        out.push({ turn, kind: 'damage', actor, hp, max, from });
        break;
      }
      case '-heal': {
        const actor = parseActor(parts[2]);
        const { hp, max } = parseHpString(parts[3] || '');
        const from = parseFromTag(parts);
        out.push({ turn, kind: 'heal', actor, hp, max, from });
        break;
      }
      case 'faint': {
        const actor = parseActor(parts[2]);
        out.push({ turn, kind: 'faint', actor });
        break;
      }
      case '-status': {
        // |-status|p2a: Amoonguss|brn
        const actor = parseActor(parts[2]);
        const status = parts[3];
        const from = parseFromTag(parts);
        out.push({ turn, kind: 'status', actor, status, from });
        break;
      }
      case '-curestatus': {
        const actor = parseActor(parts[2]);
        const status = parts[3];
        out.push({ turn, kind: 'curestatus', actor, status });
        break;
      }
      case '-boost': {
        // |-boost|p1a: Garchomp|atk|1
        const actor = parseActor(parts[2]);
        const stat = parts[3];
        const amount = parseInt(parts[4]) || 0;
        out.push({ turn, kind: 'boost', actor, stat, amount });
        break;
      }
      case '-unboost': {
        const actor = parseActor(parts[2]);
        const stat = parts[3];
        const amount = parseInt(parts[4]) || 0;
        out.push({ turn, kind: 'unboost', actor, stat, amount });
        break;
      }
      case '-crit': {
        const actor = parseActor(parts[2]);
        out.push({ turn, kind: 'crit', actor });
        break;
      }
      case '-miss': {
        const source = parseActor(parts[2]);
        const target = parts[3] ? parseActor(parts[3]) : null;
        out.push({ turn, kind: 'miss', source, target });
        break;
      }
      case '-supereffective':
      case '-resisted':
      case '-immune': {
        const actor = parseActor(parts[2]);
        out.push({ turn, kind: kind.slice(1), actor });
        break;
      }
      case 'win': {
        out.push({ turn, kind: 'win', winner: parts[2] });
        break;
      }
      case 'tie': {
        out.push({ turn, kind: 'tie' });
        break;
      }
      default:
        break;
    }
  }
  return out;
}

// --- Side driver ---------------------------------------------------------
//
// Scripted-action driver. Reacts to `|request|...` lines from each
// player stream and writes the next pre-specified action for that side.
// Handles team preview and forceSwitch automatically (sends a default
// team order; uses the next turn entry for forceSwitch).

async function driveSide(playerStream, actions, teamLen, sideTag, debug, errorsOut) {
  let idx = 0;
  for await (const chunk of playerStream) {
    if (debug) process.stderr.write(`[${sideTag}] ${chunk.slice(0, 200).replace(/\n/g, ' | ')}\n`);
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
      if (idx >= actions.length) break;
      const cmd = actions[idx++];
      playerStream.write(cmd);
      continue;
    }
  }
}

// --- Job runner ----------------------------------------------------------

function normalizeTurnAction(a) {
  if (a == null) return 'pass';
  if (typeof a === 'string') return a;
  if (Array.isArray(a)) return a.join(', ');
  throw new Error(`bad turn action: ${JSON.stringify(a)}`);
}

async function runJob(job) {
  const seed = job.seed || [1, 2, 3, 4];
  const format = job.format || 'gen9customgame';
  if (!job.p1 || !job.p2) throw new Error('job needs p1.team and p2.team');
  const team1 = Teams.import(job.p1.team);
  const team2 = Teams.import(job.p2.team);
  if (!Array.isArray(team1) || team1.length === 0) throw new Error('p1.team failed to parse');
  if (!Array.isArray(team2) || team2.length === 0) throw new Error('p2.team failed to parse');

  const p1Actions = (job.turns || []).map((t) => normalizeTurnAction(t.p1));
  const p2Actions = (job.turns || []).map((t) => normalizeTurnAction(t.p2));

  const rng = [];
  const restore = patchRng(rng);
  const sideErrors = [];

  const stream = new BattleStream();
  const sides = getPlayerStreams(stream);
  const logChunks = [];
  const drainOmni = (async () => {
    for await (const chunk of sides.omniscient) logChunks.push(chunk);
  })();

  const debug = process.env.PS_GOLDEN_DEBUG === '1';

  try {
    const driveP1 = driveSide(sides.p1, p1Actions, team1.length, 'p1', debug, sideErrors);
    const driveP2 = driveSide(sides.p2, p2Actions, team2.length, 'p2', debug, sideErrors);

    sides.omniscient.write('>start ' + JSON.stringify({ formatid: format, seed }));
    sides.omniscient.write('>player p1 ' + JSON.stringify({
      name: 'P1', team: Teams.pack(team1),
    }));
    sides.omniscient.write('>player p2 ' + JSON.stringify({
      name: 'P2', team: Teams.pack(team2),
    }));

    // Wait for both sides to exhaust their action lists or stop receiving
    // requests; hard timeout in case PS hangs waiting on input.
    const timeoutMs = 10_000;
    const timeoutSym = Symbol('timeout');
    const safeP1 = driveP1.catch((e) => { if (debug) process.stderr.write(`p1 err: ${e}\n`); });
    const safeP2 = driveP2.catch((e) => { if (debug) process.stderr.write(`p2 err: ${e}\n`); });
    await Promise.race([
      Promise.all([safeP1, safeP2]),
      new Promise((res) => setTimeout(() => res(timeoutSym), timeoutMs)),
    ]);
    sides.omniscient.writeEnd();
    await Promise.race([
      drainOmni,
      new Promise((res) => setTimeout(res, 2_000)),
    ]);
  } finally {
    restore();
  }

  const log = logChunks.join('');
  const events = parseLog(log);
  return {
    ok: sideErrors.length === 0,
    name: job.name || null,
    seed,
    format,
    turns: (job.turns || []).length,
    events,
    rng,
    errors: sideErrors,
    log,
  };
}

(async () => {
  try {
    let raw;
    if (process.argv[2]) {
      raw = fs.readFileSync(process.argv[2], 'utf8');
    } else {
      raw = await readStdin();
    }
    if (!raw.trim()) throw new Error('empty input (expected JSON job)');
    const job = JSON.parse(raw);
    const result = await runJob(job);
    process.stdout.write(JSON.stringify(result, null, 2));
  } catch (err) {
    process.stderr.write(String((err && err.stack) || err) + '\n');
    process.stdout.write(JSON.stringify({ ok: false, error: String((err && err.message) || err) }));
    process.exit(1);
  }
})();
