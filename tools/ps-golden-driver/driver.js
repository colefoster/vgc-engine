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
//
// In random-play mode, `actions` is null and `picker` is a function
// `(request) => choiceString` that picks a uniformly-random legal
// action per-request. Picker logic mirrors PS's own
// `sim/tools/random-player-ai.ts` `receiveRequest` (lines 37-189).

async function driveSide(playerStream, actions, teamLen, sideTag, debug, errorsOut, picker) {
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
      if (picker) {
        const cmd = picker(req);
        if (cmd == null) break;
        playerStream.write(cmd);
        continue;
      }
      if (idx >= actions.length) break;
      const cmd = actions[idx++];
      playerStream.write(cmd);
      continue;
    }
  }
}

// --- Random-play picker --------------------------------------------------
//
// mulberry32 — same PRNG team-gen.js uses; small, deterministic, no deps.

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

// Picks a uniformly-random legal action per request. Mirrors
// sim/tools/random-player-ai.ts:37-189 (`receiveRequest`) — same
// branching on req.wait / req.forceSwitch / req.active, same legal-set
// construction, same one-side comma-joined output.
function makeRandomPicker(rand) {
  const randInt = (n) => Math.floor(rand() * n);
  const sample = (arr) => arr[randInt(arr.length)];

  return function picker(req) {
    if (req.wait) return null;

    if (req.forceSwitch) {
      // sim/tools/random-player-ai.ts:41-67
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
          // Same parity rule as the PS AI (random-player-ai.ts:55):
          // keep j when `!fnt === !reviving` — normal switch wants
          // alive (reviving=false → !fnt must be true), revival-blessing
          // wants fainted.
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
      // sim/tools/random-player-ai.ts:70-189
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
        // Filter adjacentAlly moves if no ally alive.
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

        // Same gate as PS AI: switch if no moves OR `random() > move` (1.0
        // by default → never switch voluntarily, but we use 0.8 so we
        // exercise voluntary switching in the random goldens).
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

  const randomPlay = !!job.random_play;
  const maxTurns = job.max_turns || 30;

  const p1Actions = randomPlay ? null : (job.turns || []).map((t) => normalizeTurnAction(t.p1));
  const p2Actions = randomPlay ? null : (job.turns || []).map((t) => normalizeTurnAction(t.p2));

  // Side-distinct PRNG seeds: derived from seed[0] so the run is
  // reproducible from a single integer (matches team-gen.js convention).
  // The constants are arbitrary but distinct large primes so p1's stream
  // doesn't trivially alias p2's.
  const p1Picker = randomPlay
    ? makeRandomPicker(mulberry32(((seed[0] || 1) * 2654435761) >>> 0))
    : null;
  const p2Picker = randomPlay
    ? makeRandomPicker(mulberry32(((seed[0] || 1) * 1597334677) >>> 0))
    : null;

  const rng = [];
  const restore = patchRng(rng);
  const sideErrors = [];

  const stream = new BattleStream();
  const sides = getPlayerStreams(stream);
  const logChunks = [];
  let currentTurn = 0;
  const drainOmni = (async () => {
    for await (const chunk of sides.omniscient) {
      logChunks.push(chunk);
      // Track turn number from the protocol so we can stop after max_turns.
      for (const l of chunk.split('\n')) {
        if (l.startsWith('|turn|')) {
          const n = parseInt(l.slice('|turn|'.length), 10);
          if (Number.isFinite(n)) currentTurn = n;
          if (randomPlay && currentTurn > maxTurns) {
            try { sides.omniscient.write('>forcetie'); } catch (_) {}
          }
        }
        if (l.startsWith('|win|') || l.startsWith('|tie')) {
          // Battle ended naturally.
        }
      }
    }
  })();

  const debug = process.env.PS_GOLDEN_DEBUG === '1';

  try {
    const driveP1 = driveSide(sides.p1, p1Actions, team1.length, 'p1', debug, sideErrors, p1Picker);
    const driveP2 = driveSide(sides.p2, p2Actions, team2.length, 'p2', debug, sideErrors, p2Picker);

    sides.omniscient.write('>start ' + JSON.stringify({ formatid: format, seed }));
    sides.omniscient.write('>player p1 ' + JSON.stringify({
      name: 'P1', team: Teams.pack(team1),
    }));
    sides.omniscient.write('>player p2 ' + JSON.stringify({
      name: 'P2', team: Teams.pack(team2),
    }));

    // Wait for both sides to exhaust their action lists or stop receiving
    // requests; hard timeout in case PS hangs waiting on input.
    // Random-play battles run to 30 turns by default — bump the cap so the
    // long ones don't get truncated by the watchdog.
    const timeoutMs = randomPlay ? 60_000 : 10_000;
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
    turns: randomPlay ? currentTurn : (job.turns || []).length,
    random_play: randomPlay,
    events,
    rng,
    errors: sideErrors,
    log,
  };
}

// --- Batch mode ----------------------------------------------------------
//
// Reads newline-delimited JSON jobs from stdin and emits one compact-JSON
// result line per job on stdout. Boots PS once at startup so the dex load
// (~60s) is amortized across the whole batch instead of being paid per
// invocation. Errors in a single job are returned as `{ ok: false, errors }`
// so one bad job doesn't kill the loop.
async function runBatch() {
  process.stdin.setEncoding('utf8');
  let buf = '';
  // We process jobs sequentially: PS's Battle global state (the prototype
  // patch in patchRng + per-battle BattleStream) is not safe to run in
  // parallel within one Node process.
  const jobQueue = [];
  let stdinDone = false;
  let resolveNext = null;

  const waitForJob = () => new Promise((res) => {
    if (jobQueue.length || stdinDone) return res();
    resolveNext = res;
  });

  process.stdin.on('data', (chunk) => {
    buf += chunk;
    let idx;
    while ((idx = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 1);
      if (line.trim()) jobQueue.push(line);
    }
    if (resolveNext) { const r = resolveNext; resolveNext = null; r(); }
  });
  process.stdin.on('end', () => {
    if (buf.trim()) jobQueue.push(buf);
    buf = '';
    stdinDone = true;
    if (resolveNext) { const r = resolveNext; resolveNext = null; r(); }
  });

  while (true) {
    if (!jobQueue.length) {
      if (stdinDone) break;
      await waitForJob();
      continue;
    }
    const line = jobQueue.shift();
    let result;
    try {
      const job = JSON.parse(line);
      result = await runJob(job);
    } catch (err) {
      result = {
        ok: false,
        errors: [String((err && err.message) || err)],
      };
    }
    process.stdout.write(JSON.stringify(result) + '\n');
  }
}

(async () => {
  try {
    if (process.argv.includes('--batch')) {
      await runBatch();
      return;
    }
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
