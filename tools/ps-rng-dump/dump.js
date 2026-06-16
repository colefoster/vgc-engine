#!/usr/bin/env node
// PS RNG dumper — stream-driven version.
//
// Drives Pokemon Showdown's BattleStream with a fixed PRNG seed against
// a replay's action sequence and captures every Battle.random /
// Battle.randomChance call as a JSON event stream. The output is
// consumed by `vgc-engine-replay::load_rng_dump` and fed into vgc-engine
// via `Rng::oracle_partial`.
//
// Driver model:
//   * Pre-extract a per-side queue from the replay log, labelled with
//     `type: "turn" | "replace"` so we never confuse a forced-switch
//     prompt with a normal-turn prompt.
//   * Use BattleStreams.getPlayerStreams(stream) → react to each side's
//     `|request|...` lines. Send `team`, `forceSwitch`, or `active`
//     responses pulled from the queue based on the request shape.
//
// This is the v2 of PR-73's single-shot driver, which assumed a fixed
// command order. PR-74 measured 0.4% clean dumps on real replays; the
// reactive flow here is meant to push that close to 100%.
//
// Usage:
//   node dump.js     # reads JSON job from stdin, writes JSON dump to stdout
//
// Job shape (input on stdin):
// {
//   "seed":     [1,2,3,4],          // optional; default [1,2,3,4]
//   "format":   "gen9doublescustomgame",  // optional
//   "replay":   { ...full replay JSON... } // {teams,actions} also OK
// }
//
// Output shape (stdout):
// {
//   "ok": true,
//   "seed": [...],
//   "turns": <number>,
//   "events": [ { kind, value, ... }, ... ],
//   "log": "...PS protocol log..."
// }

'use strict';

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

// --- Replay → per-side action queue ---------------------------------------

function extractFromReplay(replay) {
  const log = replay.log || '';
  const lines = log.split('\n');

  // Team preview details per side.
  const teamPreviewOrder = [[], []];
  let gametype = 'singles';
  for (const line of lines) {
    if (!line.startsWith('|')) continue;
    const [, kind, ...rest] = line.split('|');
    if (kind === 'gametype') gametype = rest[0];
    else if (kind === 'poke') {
      const player = parseInt(rest[0].slice(1)) - 1;
      teamPreviewOrder[player].push(rest[1]);
    }
  }
  const slots = gametype === 'doubles' ? 2 : 1;

  // Minimal legal sets — PS picks default ability if blank; default IVs/EVs
  // are accepted by Teams.unpack.
  const teams = [[], []];
  for (let p = 0; p < 2; p++) {
    for (const details of teamPreviewOrder[p]) {
      const species = details.split(',')[0].trim();
      teams[p].push({
        species, name: species, ability: '', item: '',
        moves: ['tackle'], // overridden per-mon below
        nature: 'Hardy',
        evs: { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 },
        ivs: { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 },
        level: 50,
      });
    }
  }

  // Per-mon move-slot map: moves[player][teamIdx] = [moveSlug…]
  const moves = [[], []];
  const teraDone = [[], []];
  for (let p = 0; p < 2; p++) {
    for (let i = 0; i < teams[p].length; i++) {
      moves[p].push([]);
      teraDone[p].push(false);
    }
  }
  const slotChars = ['a', 'b'];

  // active[player][slot] = team-index of currently active mon. Filled
  // by pre-turn-1 `|switch|` events (the leads). Mutated across the
  // walk; for team-order computation we snapshot the original leads
  // into `leads[]` before the first |turn|.
  const active = [[null, null], [null, null]];
  const leads = [[null, null], [null, null]];
  const needsReplace = [[false, false], [false, false]];

  function moveSlug(name) { return name.toLowerCase().replace(/[^a-z0-9]/g, ''); }
  function ensureMove(p, t, slug) {
    const arr = moves[p][t];
    if (!arr.includes(slug) && arr.length < 4) arr.push(slug);
  }
  function moveSlot(p, t, slug) { return moves[p][t].indexOf(slug) + 1; }

  // Each side's queue: ordered list of { type: "turn"|"replace",
  // slots: [perSlotCommand|null] }. Per-slot command is a string like
  // "move 2 -2" or "switch 3" or null (use 'pass').
  const queue = [[], []];
  let pendingTurn = [
    { type: 'turn', slots: new Array(slots).fill(null) },
    { type: 'turn', slots: new Array(slots).fill(null) },
  ];
  // Replacements collected this turn-bucket, per side: [{slot, teamIdx}, …]
  let pendingReplaces = [[], []];

  function flushTurn() {
    for (let p = 0; p < 2; p++) {
      // Main turn — only push if at least one slot has an action; an
      // entirely empty "pass, pass" turn is still meaningful (e.g.
      // doubles with both mons fainted waiting for replace).
      queue[p].push(pendingTurn[p]);
      pendingTurn[p] = { type: 'turn', slots: new Array(slots).fill(null) };
      // Replacements after this turn — emit per slot. Each replacement is
      // its own request from PS, but PS in doubles batches both slot
      // replacements into a single `forceSwitch: [true,true]` request,
      // so we coalesce same-turn replacements into ONE entry with
      // multiple slots filled in.
      if (pendingReplaces[p].length > 0) {
        const item = { type: 'replace', slots: new Array(slots).fill(null) };
        for (const r of pendingReplaces[p]) {
          item.slots[r.slot] = `switch ${r.teamIdx + 1}`;
        }
        queue[p].push(item);
        pendingReplaces[p] = [];
      }
    }
  }

  let sawFirstTurn = false;
  let lastTeraSlot = null;

  for (const line of lines) {
    if (!line.startsWith('|')) continue;
    const parts = line.split('|');
    const kind = parts[1];
    if (kind === 'turn') {
      if (!sawFirstTurn) {
        // Snapshot the leads at the moment of the first |turn| — this
        // is the team order we'll send back to PS.
        for (let p = 0; p < 2; p++) {
          for (let s = 0; s < slots; s++) leads[p][s] = active[p][s];
        }
      }
      if (sawFirstTurn) flushTurn();
      sawFirstTurn = true;
      lastTeraSlot = null;
      continue;
    }
    const fromIdx = parts.indexOf('[from]', 3);
    const isFromCause = fromIdx > 0;

    if (kind === 'switch' && !isFromCause) {
      // |switch|p1a: NICK|species, L50, X|HP
      const slotStr = parts[2].split(': ')[0];
      const player = parseInt(slotStr[1]) - 1;
      const slot = slotChars.indexOf(slotStr[2]);
      if (slot < 0 || slot >= slots) continue;
      const details = parts[3];
      const incomingSpecies = details.split(',')[0].trim();
      const teamIdx = teamPreviewOrder[player]
        .findIndex((d) => d.split(',')[0].trim() === incomingSpecies);
      if (teamIdx < 0) continue;
      active[player][slot] = teamIdx;
      if (sawFirstTurn) {
        if (needsReplace[player][slot]) {
          pendingReplaces[player].push({ slot, teamIdx });
          needsReplace[player][slot] = false;
        } else {
          pendingTurn[player].slots[slot] = `switch ${teamIdx + 1}`;
        }
      }
      continue;
    }
    if (!sawFirstTurn) continue;

    if (kind === 'faint') {
      const slotStr = parts[2].split(': ')[0];
      const player = parseInt(slotStr[1]) - 1;
      const slot = slotChars.indexOf(slotStr[2]);
      if (slot >= 0 && slot < slots) {
        needsReplace[player][slot] = true;
        active[player][slot] = null;
      }
      continue;
    }

    if (kind === '-terastallize') {
      const slotStr = parts[2].split(': ')[0];
      const teraPlayer = parseInt(slotStr[1]) - 1;
      const teraSlot = slotChars.indexOf(slotStr[2]);
      if (teraSlot >= 0) lastTeraSlot = { teraPlayer, teraSlot };
      continue;
    }

    if (kind === 'move' && !isFromCause) {
      const slotStr = parts[2].split(': ')[0];
      const player = parseInt(slotStr[1]) - 1;
      const slot = slotChars.indexOf(slotStr[2]);
      if (slot < 0 || slot >= slots) continue;
      const moveName = parts[3];
      const targetStr = parts[4] || '';
      const isSpread = parts.some((p) => p && p.startsWith('[spread]'));
      const teamIdx = active[player][slot];
      if (teamIdx == null) continue;
      const slug = moveSlug(moveName);
      ensureMove(player, teamIdx, slug);
      const mvSlot = moveSlot(player, teamIdx, slug);
      let targetCmd = '';
      if (slots > 1 && !isSpread && targetStr && targetStr.startsWith('p')) {
        const tp = parseInt(targetStr[1]) - 1;
        const ts = slotChars.indexOf(targetStr[2]);
        // Same-slot reference = self target → no index. Same side ally
        // = -slot. Foe side = +slot.
        const isSelf = tp === player && ts === slot;
        if (ts >= 0 && !isSelf) {
          const idx = tp === player ? -(ts + 1) : (ts + 1);
          targetCmd = ` ${idx}`;
        }
      }
      let teraSuffix = '';
      if (lastTeraSlot
          && lastTeraSlot.teraPlayer === player
          && lastTeraSlot.teraSlot === slot
          && !teraDone[player][teamIdx]) {
        teraSuffix = ' terastallize';
        teraDone[player][teamIdx] = true;
        lastTeraSlot = null;
      }
      pendingTurn[player].slots[slot] = `move ${mvSlot}${targetCmd}${teraSuffix}`;
    }
  }
  if (sawFirstTurn) flushTurn();

  // Move slugs we observed → set the team's move list.
  for (let p = 0; p < 2; p++) {
    for (let i = 0; i < teams[p].length; i++) {
      if (moves[p][i].length > 0) teams[p][i].moves = moves[p][i].slice();
    }
  }

  // Build per-side team-preview order so PS picks the right leads.
  // active[player][slot] currently holds each lead's team-index from
  // the pre-turn-1 switches. Format: lead0, lead1, then remaining
  // mons in original dex order (PS accepts any permutation of 1..N).
  const teamOrders = [[], []];
  // posOf[player][teamIdx] = 1-based position in the reordered team.
  // PS `switch N` interprets N as the reordered position, NOT the
  // original dex index, so we translate before emitting commands.
  const posOf = [[], []];
  for (let p = 0; p < 2; p++) {
    const used = new Set();
    for (let s = 0; s < slots; s++) {
      const t = leads[p][s];
      if (t != null && !used.has(t)) {
        teamOrders[p].push(t + 1);
        used.add(t);
      }
    }
    for (let i = 0; i < teams[p].length; i++) {
      if (!used.has(i)) teamOrders[p].push(i + 1);
    }
    posOf[p] = new Array(teams[p].length).fill(0);
    for (let i = 0; i < teamOrders[p].length; i++) {
      posOf[p][teamOrders[p][i] - 1] = i + 1;
    }
  }

  // Re-translate every `switch N` command in the queues from
  // teamIdx-based to reordered-position-based.
  for (let p = 0; p < 2; p++) {
    for (const item of queue[p]) {
      for (let s = 0; s < item.slots.length; s++) {
        const cmd = item.slots[s];
        if (typeof cmd === 'string' && cmd.startsWith('switch ')) {
          const oldIdx = parseInt(cmd.slice('switch '.length)) - 1;
          item.slots[s] = `switch ${posOf[p][oldIdx]}`;
        }
      }
    }
  }

  return {
    teams, gametype, slots,
    queueP1: queue[0], queueP2: queue[1],
    teamOrders,
  };
}

// --- RNG capture ----------------------------------------------------------

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

// --- Side driver ----------------------------------------------------------

/**
 * Drive one side's player stream. Reacts to each `|request|...` chunk,
 * pops the matching entry from `queue`, and writes the response.
 * Resolves once the stream ends or the queue is exhausted.
 */
async function driveSide(playerStream, queue, team, slots, sideTag, teamOrder, opts = {}) {
  let qIdx = 0;
  const responses = [];
  const debug = process.env.PS_RNG_DUMP_DEBUG === '1';
  const maxRequests = opts.maxRequests || 200;
  let reqCount = 0;
  const errors = [];
  function nextOfType(type) {
    while (qIdx < queue.length && queue[qIdx].type !== type) qIdx++;
    if (qIdx >= queue.length) return null;
    return queue[qIdx++];
  }

  for await (const chunk of playerStream) {
    if (debug) process.stderr.write(`[${sideTag} CHUNK] ${chunk.slice(0,200).replace(/\n/g,' | ')}\n`);
    const errLines = chunk.split('\n').filter((l) => l.startsWith('|error|'));
    errors.push(...errLines);
    if (errors.length >= 3) {
      if (debug) process.stderr.write(`[${sideTag} GIVE UP ON ERRORS]\n`);
      break;
    }
    const requestLine = chunk.split('\n').find((l) => l.startsWith('|request|'));
    if (!requestLine) continue;
    reqCount++;
    if (reqCount > maxRequests) {
      if (debug) process.stderr.write(`[${sideTag} MAX REQUESTS]\n`);
      break;
    }
    let req;
    try {
      req = JSON.parse(requestLine.slice('|request|'.length));
    } catch (_) {
      continue;
    }
    if (!req) continue;

    if (debug) process.stderr.write(`[${sideTag} REQ] ${JSON.stringify(Object.keys(req))}\n`);
    if (req.teamPreview) {
      const order = (teamOrder && teamOrder.length === team.length
        ? teamOrder
        : team.map((_, i) => i + 1)).join('');
      playerStream.write(`team ${order}`);
      responses.push(`team ${order}`);
      if (debug) process.stderr.write(`[${sideTag} -> team ${order}]\n`);
      continue;
    }

    if (req.wait) continue;
    if (req.noCancel) {
      // Same as a normal request; falls through.
    }

    if (req.forceSwitch) {
      // PS's fixed-seed run may produce different faints than the
      // original replay (because we feed it default sets). Best-effort:
      // pull the next recorded replacement entry, and for any slot
      // PS asks about that the recording didn't fill, fall back to
      // the first alive, non-active mon listed in the request's side.
      const item = nextOfType('replace');
      const alivePokemon = (req.side && req.side.pokemon) || [];
      const usedSwitchPos = new Set();
      // First pass: collect recorded switch positions so the fallback
      // doesn't double up onto the same slot.
      if (item) {
        for (const s of item.slots) {
          if (typeof s === 'string' && s.startsWith('switch ')) {
            usedSwitchPos.add(parseInt(s.slice('switch '.length)));
          }
        }
      }
      function fallbackSwitch() {
        // Walk the side's pokemon list (1-based in PS) and return the
        // first one that's alive, not active, and not already used.
        for (let i = 0; i < alivePokemon.length; i++) {
          const mon = alivePokemon[i];
          if (mon.active) continue;
          if (mon.condition && /fnt/.test(mon.condition)) continue;
          const pos = i + 1;
          if (usedSwitchPos.has(pos)) continue;
          usedSwitchPos.add(pos);
          return `switch ${pos}`;
        }
        return 'pass'; // PS shouldn't accept this, but at least bail.
      }
      const parts = req.forceSwitch.map((needs, slot) => {
        if (!needs) return 'pass';
        if (item && item.slots[slot]) return item.slots[slot];
        return fallbackSwitch();
      });
      const cmd = parts.join(', ');
      playerStream.write(cmd);
      responses.push(cmd);
      if (debug) process.stderr.write(`[${sideTag} -> ${cmd}]\n`);
      continue;
    }

    if (req.active) {
      const item = nextOfType('turn');
      if (!item) { if (debug) process.stderr.write(`[${sideTag} OUT OF TURNS]\n`); break; }
      const parts = req.active.map((slotInfo, slot) => {
        return item.slots[slot] || 'pass';
      });
      const cmd = parts.join(', ');
      playerStream.write(cmd);
      responses.push(cmd);
      if (debug) process.stderr.write(`[${sideTag} -> ${cmd}]\n`);
      continue;
    }
  }
  if (debug) process.stderr.write(`[${sideTag} EXIT, qIdx=${qIdx}/${queue.length}]\n`);
  // Mark variable as used to satisfy linters.
  void sideTag;
  return { responses, errors };
}

// --- Job runner -----------------------------------------------------------

async function runJob(job) {
  let extracted = null;
  if (job.replay) {
    extracted = extractFromReplay(job.replay);
    job.teams = extracted.teams;
    job.format = job.format || (extracted.gametype === 'doubles'
      ? 'gen9doublescustomgame'
      : 'gen9customgame');
  }
  if (!Array.isArray(job.teams) || job.teams.length !== 2) {
    throw new Error('job needs `replay` or `teams` (2 arrays)');
  }
  const seed = job.seed || [1, 2, 3, 4];
  const format = job.format || 'gen9customgame';
  const events = [];
  let sideErrors = [];
  const restore = patchRng(events);

  const stream = new BattleStream();
  const sides = getPlayerStreams(stream);

  const logChunks = [];
  const drainOmni = (async () => {
    for await (const chunk of sides.omniscient) {
      logChunks.push(chunk);
    }
  })();

  try {
    const slots = extracted ? extracted.slots : (job.gametype === 'doubles' ? 2 : 1);

    // Wire side drivers BEFORE we send `>start` so the first request
    // (team preview) lands in their async loop.
    const driveP1 = extracted
      ? driveSide(sides.p1, extracted.queueP1, extracted.teams[0], slots, 'p1', extracted.teamOrders[0])
      : Promise.resolve([]);
    const driveP2 = extracted
      ? driveSide(sides.p2, extracted.queueP2, extracted.teams[1], slots, 'p2', extracted.teamOrders[1])
      : Promise.resolve([]);

    sides.omniscient.write('>start ' + JSON.stringify({ formatid: format, seed }));
    sides.omniscient.write('>player p1 ' + JSON.stringify({
      name: 'P1', team: Teams.pack(job.teams[0]),
    }));
    sides.omniscient.write('>player p2 ' + JSON.stringify({
      name: 'P2', team: Teams.pack(job.teams[1]),
    }));

    if (!extracted) {
      // Explicit-actions path: keep PR-73 behavior for synthetic jobs.
      const order1 = job.teams[0].map((_, i) => i + 1).join('');
      const order2 = job.teams[1].map((_, i) => i + 1).join('');
      sides.p1.write(`team ${order1}`);
      sides.p2.write(`team ${order2}`);
      for (const turn of (job.actions || [])) {
        if (turn.p1) sides.p1.write(turn.p1);
        if (turn.p2) sides.p2.write(turn.p2);
        if (turn.replacements) {
          const byP1 = turn.replacements.filter((r) => r.side === 'p1');
          const byP2 = turn.replacements.filter((r) => r.side === 'p2');
          if (byP1.length) {
            sides.p1.write(byP1.map((r) => `switch ${r.teamIdx + 1}`).join(', '));
          }
          if (byP2.length) {
            sides.p2.write(byP2.map((r) => `switch ${r.teamIdx + 1}`).join(', '));
          }
        }
      }
      sides.omniscient.writeEnd();
    } else {
      // Stream-driven path: wait for both side drivers to exhaust their
      // queues (or bail on errors), then close out. Race with a hard
      // timeout — PS can hang waiting for a never-coming response if
      // both sides bail mid-prompt.
      const sideTimeoutMs = 5_000;
      const timeoutSym = Symbol('timeout');
      // Defensive: never let an awaited driver promise reject — wrap in
      // a resolve so we can always pull `.errors` off both sides after
      // the race, even when one side timed out waiting for input.
      const safeP1 = driveP1.catch((e) => ({ responses: [], errors: [String(e)] }));
      const safeP2 = driveP2.catch((e) => ({ responses: [], errors: [String(e)] }));
      const sideTimeout = new Promise((res) => setTimeout(() => res(timeoutSym), sideTimeoutMs));
      const raceResult = await Promise.race([
        Promise.all([safeP1, safeP2]),
        sideTimeout,
      ]);
      sides.omniscient.writeEnd();
      if (raceResult !== timeoutSym) {
        const [p1res, p2res] = raceResult;
        sideErrors = [...p1res.errors, ...p2res.errors];
      } else {
        // Timeout: one or both sides are still iterating but PS isn't
        // sending them anything. Try to drain whatever errors each
        // side ALREADY collected by giving them a brief window after
        // the omniscient writeEnd closes their streams.
        const drainMs = 500;
        const drained = await Promise.race([
          Promise.all([safeP1, safeP2]),
          new Promise((res) => setTimeout(() => res(null), drainMs)),
        ]);
        if (drained) {
          const [p1res, p2res] = drained;
          sideErrors = [...p1res.errors, ...p2res.errors];
        }
        if (sideErrors.length === 0) {
          sideErrors = ['side driver timed out'];
        }
      }
    }

    // Drain omniscient with its own short timeout in case PS doesn't
    // propagate the writeEnd cleanly.
    const drainTimeoutMs = 2_000;
    await Promise.race([
      drainOmni,
      new Promise((res) => setTimeout(res, drainTimeoutMs)),
    ]);
    // `ok` is true only when neither player-side driver reported an
    // |error| line back from PS. The omniscient stream doesn't echo
    // those errors, so checking `log` alone (as `generate-sidecars.sh`
    // used to do) silently classifies aborted-mid-turn dumps as clean.
    // Surfacing `errors` here lets the driver reject them and avoids
    // poisoning the oracle queue with zero-event sidecars from runs
    // that bailed at turn 1.
    return {
      ok: sideErrors.length === 0,
      seed,
      turns: extracted ? Math.max(extracted.queueP1.length, extracted.queueP2.length) : (job.actions || []).length,
      events,
      errors: sideErrors,
      log: logChunks.join(''),
    };
  } finally {
    restore();
  }
}

(async () => {
  try {
    const raw = await readStdin();
    if (!raw.trim()) throw new Error('empty stdin (expected JSON job)');
    const job = JSON.parse(raw);
    const result = await runJob(job);
    process.stdout.write(JSON.stringify(result));
  } catch (err) {
    process.stderr.write(String(err && err.stack || err) + '\n');
    process.stdout.write(JSON.stringify({ ok: false, error: String(err && err.message || err) }));
    process.exit(1);
  }
})();
