#!/usr/bin/env node
// PS side of the accuracy-proof experiments (docs/accuracy/2026-09-accuracy-proof.md).
//
// Runs full Pokémon Showdown battles and records everything the engine-side
// tool (`cargo run -p vgc-engine-conformance --bin accuracy`) needs:
//
//   * teams + seed (a real PS seed string: `sodium,<hex>` or `a,b,c,d`);
//   * the resolved choices per turn, split by phase: the main decision,
//     mid-turn forced switches (U-turn, Eject Button, ...) and end-of-turn
//     replacements after faints;
//   * keyed RNG outcomes (the conformance-key-contract envelopes, via
//     ../ps-golden-driver/conformance-driver.js `patchRng`) for keyed replay;
//   * a RAW draw trace: every call into PS's PRNG (`PRNG.random`, which
//     `randomChance`, `sample` and `shuffle` all go through) with turn, args,
//     raw 32-bit output, result and the PS call site — for the draw-by-draw
//     diff against the engine's `ps-rng` trace;
//   * end-of-turn state (post replacement) of every active slot, the field
//     and both sides.
//
// Choices come from a job's `intents` (the real ladder log's choices, see
// recon.js) when they are legal in the PS battle, else a seeded random legal
// choice. Jobs without intents play fully random.
//
// Usage:
//   node ps-battle.js <jobs.jsonl> <out_dir> [--max-turns N] [--start K] [--count M]
//
// Env: PS_DIST — path to a built pokemon-showdown `dist/sim`.

'use strict';

const fs = require('fs');
const path = require('path');

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';
process.env.PS_DIST = PS_PATH;
const conf = require(path.join(__dirname, '..', 'ps-golden-driver', 'conformance-driver.js'));
// PS `switch N` (current side order) -> engine `switch N` (original order).
const { captureRoster, toEngineCmd } = require('./switch-order.js');
const ps = require(PS_PATH);
const { BattleStream, Teams, getPlayerStreams, Dex } = ps;
const { PRNG, SodiumRNG, Gen5RNG } = require(PS_PATH + '/prng');
const { Battle } = require(PS_PATH + '/battle');

Error.stackTraceLimit = 25;

// --- raw PRNG trace -------------------------------------------------------

let RAW = null; // array being filled for the current battle, or null
let CUR = null; // current Battle (for .turn)
let lastRaw = 0;

for (const C of [SodiumRNG, Gen5RNG]) {
  const orig = C.prototype.next;
  C.prototype.next = function () {
    lastRaw = orig.call(this);
    return lastRaw;
  };
}

// Frames that are PRNG plumbing rather than the semantic call site.
const WRAPPER = /PRNG\.|prng\.js|Battle\.random |Battle\.randomChance|Battle\.sample|Battle\.random$/;

function frames() {
  const raw = (new Error().stack || '').split('\n').slice(2);
  const out = [];
  for (const line of raw) {
    const m = line.match(/at\s+(?:new\s+)?([^\s(]+)(?:\s+\(([^)]+)\))?/);
    if (!m) continue;
    let loc = (m[2] || '').replace(/^.*?\/dist\//, '').replace(/:\d+$/, '');
    out.push({ fn: m[1], loc });
  }
  return out;
}

const origRandom = PRNG.prototype.random;
PRNG.prototype.random = function (from, to) {
  const v = origRandom.call(this, from, to);
  if (RAW && CUR && this === CUR.prng) {
    const fr = frames();
    // op: the outermost PRNG-level method (shuffle/sample/randomChance/random)
    let op = 'random';
    for (const f of fr) {
      const m = /^PRNG\.(\w+)$/.exec(f.fn);
      if (m && m[1] !== 'random') op = m[1];
    }
    const sem = fr.filter((f) => !WRAPPER.test(f.fn + ' ') && !/prng\.js/.test(f.loc));
    const site = sem.slice(0, 6).map((f) => `${f.fn}@${f.loc.replace(/^sim\//, '').replace(/^data\//, 'data/')}`);
    RAW.push({
      seq: RAW.length,
      turn: CUR.turn,
      op,
      a: from === undefined ? 0 : from,
      b: to === undefined ? 0 : to,
      raw: lastRaw >>> 0,
      result: from === undefined ? lastRaw >>> 0 : v,
      site,
    });
  }
  return v;
};

// --- choice selection ------------------------------------------------------

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

const TARGETED = new Set(['normal', 'any', 'adjacentFoe', 'adjacentAlly', 'adjacentAllyOrSelf']);

const toID = (s) => ('' + s).toLowerCase().replace(/[^a-z0-9]+/g, '');

function speciesKey(details) {
  // "Arcanine-Hisui, L50, M" / "Froslass-Mega, ..." -> species id, megas
  // folded to their base forme.
  const sp = Dex.species.get((details || '').split(', ')[0]);
  return toID(sp.isMega ? sp.baseSpecies : sp.name);
}

function makePicker(sideId, intents, rand, stats) {
  const randInt = (n) => Math.floor(rand() * n);
  const sample = (arr) => arr[randInt(arr.length)];

  function randomMove(active, i, nActive, allyAlive) {
    const moves = [];
    (active.moves || []).forEach((m, j) => {
      if (m.disabled) return;
      if (m.target === 'adjacentAlly' && !allyAlive) return;
      moves.push({ slot: j + 1, target: m.target });
    });
    if (!moves.length) return null;
    const m = sample(moves);
    let cmd = `move ${m.slot}`;
    if (nActive > 1) {
      if (['normal', 'any', 'adjacentFoe'].includes(m.target)) cmd += ` ${1 + randInt(2)}`;
      else if (m.target === 'adjacentAlly') cmd += ` -${(i ^ 1) + 1}`;
      else if (m.target === 'adjacentAllyOrSelf') cmd += allyAlive ? ` -${1 + randInt(2)}` : ` -${i + 1}`;
    }
    return cmd;
  }

  function switchTo(pokemon, speciesId, chosen, allowFainted) {
    for (let j = 0; j < pokemon.length; j++) {
      const p = pokemon[j];
      if (p.active || chosen.includes(j + 1)) continue;
      if (!allowFainted && p.condition.endsWith(' fnt')) continue;
      if (speciesKey(p.details) === speciesId) return j + 1;
    }
    return null;
  }

  function benchChoices(pokemon, chosen) {
    const out = [];
    for (let j = 0; j < pokemon.length; j++) {
      const p = pokemon[j];
      if (p.active || chosen.includes(j + 1) || p.condition.endsWith(' fnt')) continue;
      out.push(j + 1);
    }
    return out;
  }

  // phase: 'main' | 'mid' | 'rep'; turn: the battle turn the request belongs to
  return function pick(req, turn, phase) {
    const t = (intents && intents[turn] && intents[turn][sideId]) || null;
    const pokemon = req.side.pokemon;
    if (req.forceSwitch) {
      const chosen = [];
      const queue = t ? (phase === 'mid' ? t.mid : t.rep).slice() : [];
      return req.forceSwitch.map((must, i) => {
        if (!must) return 'pass';
        if (pokemon[i].reviving) {
          // Revival Blessing: PS wants a FAINTED bench mon here.
          for (let j = 0; j < pokemon.length; j++) {
            if (!pokemon[j].active && !chosen.includes(j + 1) && pokemon[j].condition.endsWith(' fnt')) {
              chosen.push(j + 1);
              stats.fallback++;
              return `switch ${j + 1}`;
            }
          }
          return 'pass';
        }
        const k = queue.findIndex((q) => q.slot === i);
        if (k >= 0) {
          const target = switchTo(pokemon, toID(queue[k].species), chosen, false);
          queue.splice(k, 1);
          if (target) { chosen.push(target); stats.log++; return `switch ${target}`; }
        }
        const bench = benchChoices(pokemon, chosen);
        if (!bench.length) return 'pass';
        const target = sample(bench);
        chosen.push(target);
        stats.fallback++;
        return `switch ${target}`;
      }).join(', ');
    }
    const chosen = [];
    let megaUsed = false;
    const n = req.active.length;
    return req.active.map((active, i) => {
      if (pokemon[i].condition.endsWith(' fnt') || pokemon[i].commanding) return 'pass';
      const allyAlive = n > 1 && pokemon[i ^ 1] && !pokemon[i ^ 1].condition.endsWith(' fnt');
      const intent = t && t.main ? t.main[i] : null;
      if (intent && intent.kind === 'switch' && !active.trapped) {
        const target = switchTo(pokemon, toID(intent.species), chosen, false);
        if (target) { chosen.push(target); stats.log++; return `switch ${target}`; }
      }
      if (intent && intent.kind === 'move') {
        const j = (active.moves || []).findIndex((m) => m.id === intent.move);
        if (j >= 0 && !active.moves[j].disabled) {
          const m = active.moves[j];
          let cmd = `move ${j + 1}`;
          if (n > 1 && TARGETED.has(m.target)) {
            const tg = intent.target;
            if (tg && tg.side !== sideId) cmd += ` ${tg.slot + 1}`;
            else if (tg && tg.side === sideId && tg.slot !== i && m.target !== 'adjacentFoe') cmd += ` -${tg.slot + 1}`;
            else if (m.target === 'adjacentAlly') cmd += ` -${(i ^ 1) + 1}`;
            else if (m.target === 'adjacentAllyOrSelf') cmd += ` -${i + 1}`;
            else cmd += ` ${1 + randInt(2)}`;
          }
          if (intent.mega && active.canMegaEvo && !megaUsed) { cmd += ' mega'; megaUsed = true; }
          stats.log++;
          return cmd;
        }
      }
      stats.fallback++;
      const bench = active.trapped ? [] : benchChoices(pokemon, chosen);
      const mv = randomMove(active, i, n, allyAlive);
      if (bench.length && (!mv || rand() > 0.9)) {
        const target = sample(bench);
        chosen.push(target);
        return `switch ${target}`;
      }
      return mv || 'pass';
    }).join(', ');
  };
}

function installForcing(job, on) {
  if (!on || !job.force || !job.force[1]) return () => {};
  const uses = job.force[1];
  const origRandom = Battle.prototype.random;
  const origChance = Battle.prototype.randomChance;
  const find = (battle) => {
    if (battle.turn !== 1 || !battle.activeMove || !battle.activePokemon) return null;
    const actor = battle.activePokemon.getSlot();
    return uses.find((u) => u.actor === actor && u.move === battle.activeMove.id) || null;
  };
  const siteOf = () => (new Error().stack || '').split('\n').slice(2, 6).join(' ');
  Battle.prototype.randomChance = function (num, den) {
    const v = origChance.call(this, num, den);
    const u = find(this);
    if (!u) return v;
    const tgt = this.activeTarget && this.activeTarget.getSlot ? this.activeTarget.getSlot() : null;
    const site = siteOf();
    if (/getDamage/.test(site)) return u.crits.includes(tgt);
    if (/hitStepAccuracy|hitStepMoveHitLoop/.test(site) && den === 100) return !u.misses.includes(tgt);
    return v;
  };
  Battle.prototype.random = function (m, n) {
    const v = origRandom.call(this, m, n);
    const u = find(this);
    if (!u || m !== 100 || n !== undefined) return v;
    if (!/secondaries/.test(siteOf())) return v;
    const tgt = this.activeTarget && this.activeTarget.getSlot ? this.activeTarget.getSlot() : null;
    return u.effects.includes(tgt) ? 0 : 99;
  };
  return () => {
    Battle.prototype.random = origRandom;
    Battle.prototype.randomChance = origChance;
  };
}

// --- one battle -------------------------------------------------------------

function parseSeed(seed) {
  if (Array.isArray(seed)) return seed;
  // SEED_SALT: rerun the same jobs under a different (still deterministic)
  // sodium seed, e.g. to measure PS-vs-PS agreement from RNG alone.
  const salt = process.env.SEED_SALT;
  if (salt && seed.startsWith('sodium,')) {
    const h = require('crypto').createHash('sha256').update(seed + ':' + salt).digest('hex');
    return 'sodium,' + h.slice(0, 32);
  }
  return seed; // PS accepts 'sodium,<hex>' / 'gen5,<hex>' / 'a,b,c,d' strings
}

async function runBattle(job, maxTurns) {
  const format = job.format;
  const team1 = Teams.import(job.p1team);
  const team2 = Teams.import(job.p2team);
  const seedNum = parseInt((job.id || '1').slice(0, 8), 16) || 1;
  const stats = { log: 0, fallback: 0 };
  const pickers = {
    p1: makePicker('p1', job.intents, mulberry32(seedNum * 2654435761), stats),
    p2: makePicker('p2', job.intents, mulberry32(seedNum * 1597334677 + 7), stats),
  };

  // Each side's PS Pokemon objects in original team order (switch-order.js),
  // captured right after `>player`.
  const rosters = {};
  const engineCmd = (sideId, cmd, req) => {
    const side = stream.battle.sides[sideId === 'p1' ? 0 : 1];
    return toEngineCmd(cmd, req, side.pokemon, rosters[sideId]);
  };
  // FORCE_LOG=1: on turn 1, make PS's crit / accuracy / secondary outcomes
  // match what the real log showed (job.force from recon.js). Installed
  // below the keyed recorder so the recorded outcome is the forced one; the
  // underlying PRNG call still happens (same stream consumption).
  const restoreForce = installForcing(job, process.env.FORCE_LOG === '1');
  const keyed = [];
  const restoreKeyed = conf.patchRng(keyed);
  // `Battle.sample` goes straight to the PRNG, bypassing the keyed patch on
  // Battle.random, so status picks (Dire Claw), Champions sleep length and
  // random targets would never be keyed. Route it through Battle.random —
  // the same single random(n) call — so the draw is recorded (as `range`,
  // value = index).
  const origSample = Battle.prototype.sample;
  Battle.prototype.sample = function (items) {
    if (!items.length) return origSample.call(this, items);
    return items[this.random(items.length)];
  };
  const raw = [];

  const stream = new BattleStream();
  const sides = getPlayerStreams(stream);
  const errors = [];
  const choiceLog = []; // {side, turn, phase, cmd}
  const stateByTurn = {}, fieldByTurn = {}, sidesByTurn = {};
  let currentTurn = 0;
  let ended = false;
  let battleRef = null;
  const snap = (t) => {
    const bt = stream.battle || battleRef;
    if (!bt) return;
    const st = conf.snapshotState(bt);
    for (const side of bt.sides) {
      if (!side || !side.active) continue;
      for (const p of side.active) {
        if (!p) continue;
        const ref = p.getSlot();
        if (st[ref]) st[ref].species = p.species.id;
      }
    }
    stateByTurn[t] = st;
    fieldByTurn[t] = conf.snapshotField(bt);
    sidesByTurn[t] = conf.snapshotSides(bt);
  };

  const logChunks = [];
  const drainOmni = (async () => {
    for await (const chunk of sides.omniscient) {
      logChunks.push(chunk);
      for (const l of chunk.split('\n')) {
        if (l.startsWith('|turn|')) {
          const n = parseInt(l.slice(6), 10);
          if (n > 1) snap(n - 1);
          currentTurn = n;
          if (n > maxTurns) { try { sides.omniscient.write('>forcetie'); } catch (_) {} }
        }
        if (l.startsWith('|win|') || l === '|tie') {
          ended = true;
          snap(currentTurn); // before writeEnd() tears the battle down
        }
      }
    }
  })();

  async function driveSide(sideId) {
    const s = sides[sideId];
    let lastReq = null;
    let lastPhase = 'main';
    for await (const chunk of s) {
      const lines = chunk.split('\n');
      if (lines.some((l) => l.startsWith('|error|'))) {
        const err = lines.find((l) => l.startsWith('|error|'));
        errors.push(`[${sideId}] ${err}`);
        // [Unavailable choice]: PS revealed hidden info (trapped, disabled)
        // and follows up with a fresh |request| — answer that one instead.
        if (err.includes('[Unavailable choice]')) {
          const last = choiceLog.findLastIndex((c) => c.side === sideId);
          if (last >= 0) choiceLog.splice(last, 1);
          if (!lines.some((l) => l.startsWith('|request|'))) continue;
        } else {
        // Invalid choice: PS keeps the request open. Retry fully random.
        if (lastReq && errors.length < 50) {
          const turn = stream.battle.turn;
          const cmd = pickers[sideId](lastReq, -1, lastPhase);
          const last = choiceLog.findLastIndex((c) => c.side === sideId);
          if (last >= 0) { choiceLog[last].cmd = cmd; choiceLog[last].eng = engineCmd(sideId, cmd, lastReq); }
          s.write(cmd);
          void turn;
        }
        continue;
        }
      }
      const reqLine = lines.find((l) => l.startsWith('|request|'));
      if (!reqLine) continue;
      let req;
      try { req = JSON.parse(reqLine.slice(9)); } catch (_) { continue; }
      if (!req || req.wait) continue;
      if (req.teamPreview) {
        s.write('team ' + Array.from({ length: req.side.pokemon.length }, (_, i) => i + 1).join(''));
        continue;
      }
      const b = stream.battle;
      const turn = b.turn;
      // Past maxTurns the omniscient drain answers `|turn|N` with
      // `>forcetie`; leave the request unanswered so that turn is neither
      // played nor recorded. (End-of-turn replacements still carry the old
      // turn number.)
      if (turn > maxTurns) continue;
      let phase = 'main';
      if (req.forceSwitch) phase = b.queue.list.some((a) => a.choice === 'residual') ? 'mid' : 'rep';
      // A job may script exact commands: script[turn][side] (main phase).
      const scripted = phase === 'main' && job.script && job.script[turn] && job.script[turn][sideId];
      const cmd = scripted || pickers[sideId](req, turn, phase);
      lastReq = req;
      lastPhase = phase;
      choiceLog.push({ side: sideId, turn, phase, cmd, eng: engineCmd(sideId, cmd, req) });
      s.write(cmd);
    }
  }

  try {
    RAW = raw;
    const d1 = driveSide('p1').catch((e) => errors.push(String(e)));
    const d2 = driveSide('p2').catch((e) => errors.push(String(e)));
    sides.omniscient.write('>start ' + JSON.stringify({ formatid: format, seed: parseSeed(job.seed) }));
    CUR = stream.battle;
    battleRef = stream.battle;
    sides.omniscient.write('>player p1 ' + JSON.stringify({ name: 'P1', team: Teams.pack(team1) }));
    rosters.p1 = captureRoster(stream.battle.sides[0]);
    sides.omniscient.write('>player p2 ' + JSON.stringify({ name: 'P2', team: Teams.pack(team2) }));
    rosters.p2 = captureRoster(stream.battle.sides[1]);
    const timeout = new Promise((res) => setTimeout(() => res('timeout'), 20000));
    const r = await Promise.race([Promise.all([d1, d2]), timeout]);
    if (r === 'timeout') errors.push('timeout');
    sides.omniscient.writeEnd();
    await Promise.race([drainOmni, new Promise((res) => setTimeout(res, 1000))]);
  } finally {
    restoreKeyed();
    restoreForce();
    Battle.prototype.sample = origSample;
    RAW = null;
    CUR = null;
  }
  if (!ended) snap(currentTurn);

  // --- assemble -------------------------------------------------------------
  const byTurn = (arr) => {
    const o = {};
    for (const d of arr) (o[d.turn || 0] = o[d.turn || 0] || []).push(d);
    return o;
  };
  const keyedBy = byTurn(keyed);
  const rawBy = byTurn(raw);
  const turnNums = new Set();
  for (const c of choiceLog) turnNums.add(c.turn);
  // `!(t > maxTurns)`: same bound as the forcetie (none when maxTurns is
  // undefined / Infinity). `_meta.lastTurn` can still be the cutoff marker.
  const turns = [...turnNums].filter((t) => t > 0 && !(t > maxTurns)).sort((a, b) => a - b).map((t) => {
    const ch = { p1: [], p2: [] }, mid = { p1: [], p2: [] }, rep = { p1: [], p2: [] };
    for (const c of choiceLog) {
      if (c.turn !== t) continue;
      (c.phase === 'main' ? ch : c.phase === 'mid' ? mid : rep)[c.side].push(c.eng);
    }
    return {
      turn: t,
      choices: ch,
      midturn: mid,
      replace: rep,
      draws: keyedBy[t] || [],
      raw: rawBy[t] || [],
      state: stateByTurn[t] || {},
      field: fieldByTurn[t] || conf.snapshotField(null),
      sides: sidesByTurn[t] || conf.snapshotSides(null),
      has_state: !!stateByTurn[t],
    };
  });

  return {
    id: job.id,
    format,
    seed: job.seed,
    p1team: job.p1team,
    p2team: job.p2team,
    start_raw: rawBy[0] || [],
    turns,
    _meta: {
      ok: errors.length === 0,
      ended,
      errors,
      lastTurn: currentTurn,
      choice_log: stats.log,
      choice_fallback: stats.fallback,
      totalRaw: raw.length,
      replay: job.meta && job.meta.replay,
      log: logChunks.join('\n'),
    },
  };
}

async function main() {
  const args = process.argv.slice(2);
  const jobsPath = args[0], outDir = args[1];
  if (!jobsPath || !outDir) {
    console.error('usage: ps-battle.js <jobs.jsonl> <out_dir> [--max-turns N] [--start K] [--count M]');
    process.exit(2);
  }
  const opt = (name, d) => { const i = args.indexOf(name); return i >= 0 ? parseInt(args[i + 1], 10) : d; };
  const maxTurns = opt('--max-turns', 40);
  const start = opt('--start', 0);
  const count = opt('--count', Infinity);
  fs.mkdirSync(outDir, { recursive: true });
  const jobs = fs.readFileSync(jobsPath, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
  let ok = 0, bad = 0;
  const t0 = Date.now();
  for (const job of jobs.slice(start, start + count)) {
    const outPath = path.join(outDir, `out_${job.id}.json`);
    if (fs.existsSync(outPath) && process.env.FORCE !== '1') continue;
    try {
      const res = await runBattle(job, maxTurns);
      fs.writeFileSync(outPath, JSON.stringify(res));
      if (res._meta.ok) ok++; else bad++;
    } catch (e) {
      bad++;
      process.stderr.write(`${job.id}: ${e && e.stack || e}\n`);
    }
  }
  process.stderr.write(`ps-battle: ${ok} ok, ${bad} with errors, ${((Date.now() - t0) / 1000).toFixed(1)}s\n`);
}

module.exports = { runBattle };

if (require.main === module) main();
