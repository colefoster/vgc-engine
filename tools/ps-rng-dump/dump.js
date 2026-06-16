#!/usr/bin/env node
// PS RNG dumper — drives a Pokemon Showdown BattleStream with a fixed
// PRNG seed against a pre-determined action sequence and captures every
// `Battle.random(...)` / `Battle.randomChance(...)` call as a JSON
// event stream. The output is consumed by `vgc-engine-replay`'s
// `score_replay_oracle` via `Rng::oracle_partial`.
//
// Usage:
//   node dump.js                    # reads JSON job from stdin, writes JSON dump to stdout
//
// Job shape (input on stdin):
// {
//   "seed": [1,2,3,4],              // optional; default [1,2,3,4]
//   "format": "gen9customgame",     // optional; default gen9customgame
//   "gametype": "doubles",          // optional; default singles
//   "teams": [ [ ...PokemonSet... ], [ ...PokemonSet... ] ],
//   "actions": [                    // per-turn pairs of side commands
//     { "p1": "move 1, move 1", "p2": "move 1, move 1" },
//     ...
//   ]
// }
//
// Output shape (stdout):
// {
//   "ok": true,
//   "seed": [...],
//   "turns": <number of turns driven>,
//   "events": [
//     { "kind": "Crit", "value": true },
//     { "kind": "DamageRoll", "value": 7 },
//     { "kind": "PercentRoll", "value": 73 },
//     { "kind": "Range", "value": 2 },
//     { "kind": "Tiebreak", "value": "0xabcd…" }
//   ],
//   "log": "...the produced PS protocol log..."
// }
//
// Errors → exit code 1, stderr message, `{"ok": false, "error": "..."}` on stdout.

'use strict';

const PS_PATH = process.env.PS_DIST || '/tmp/pokemon-showdown-research/dist/sim';

const ps = require(PS_PATH);
const { BattleStream, Teams } = ps;
const Battle = require(PS_PATH + '/battle').Battle;

async function readStdin() {
  let data = '';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) data += chunk;
  return data;
}

// --- Replay → BattleStream action extractor -------------------------------
//
// Parses a PS replay's `log` field into per-turn `>p1` / `>p2` commands.
// Doubles-first; singles falls out of the same machinery (target index
// is just the lone foe slot).
//
// Heuristics:
//   |move|p1a: NICK|MoveName|target → move <slot> <targetIdx>
//   |switch|p1b: NICK|... (outside a [from]) → switch <teamIdx>
//
// Skips:
//   * any |switch| with [from] (Volt Switch, U-turn, Eject Button, …)
//   * any |move| where the user's slot is currently fainted (PS emits
//     these for moves like Pursuit / scripted swaps)
//
// Returns: { teams: [[set,…], [set,…]], actions: [{p1, p2}, …], gametype }

function extractFromReplay(replay) {
  const log = replay.log || '';
  const lines = log.split('\n');

  // 1. Teams from team preview |poke|p1|Species, L50, M|item-marker
  const teams = [[], []];
  let gametype = 'singles';
  // The corpus replays only include species + level + gender at poke
  // time. Items, abilities, EVs are hidden. For PS BattleStream we
  // need a legal team — use `canonical defaults`: empty item slot,
  // first ability of the species (PS Teams.pack tolerates blank).
  const teamPreviewOrder = [[], []];
  let teamSize = [null, null];
  for (const line of lines) {
    if (!line.startsWith('|')) continue;
    const [, kind, ...rest] = line.split('|');
    if (kind === 'gametype') gametype = rest[0];
    else if (kind === 'teamsize') {
      teamSize[parseInt(rest[0].slice(1)) - 1] = parseInt(rest[1]);
    } else if (kind === 'poke') {
      const player = parseInt(rest[0].slice(1)) - 1;
      const details = rest[1];
      teamPreviewOrder[player].push(details);
    }
  }
  // Build minimal legal sets. PS will tolerate missing item / EV / IV.
  for (let p = 0; p < 2; p++) {
    for (const details of teamPreviewOrder[p]) {
      const species = details.split(',')[0].trim();
      teams[p].push({
        species,
        name: species,
        ability: '', // PS picks the species' first ability
        item: '',
        moves: ['tackle'], // overridden per-turn below if we see different
        nature: 'Hardy',
        evs: { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 },
        ivs: { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 },
        level: 50,
      });
    }
  }

  // 2. Walk per-turn move/switch events to build per-mon move sets and
  //    `>p1 ...` / `>p2 ...` action strings. Doubles target syntax:
  //      slot 1 = foe a (+1), 2 = foe b (+2), -1 = ally a, -2 = ally b.
  //    PS replay slot letters: p1a=slot 0, p1b=slot 1.

  const slotChars = ['a', 'b'];
  // active[player][slot] = team-index of the mon currently in that slot.
  // Initialized null so the first lead-switch fills it in. Singles uses
  // only [slot 0]; doubles uses [0, 1].
  const active = [[null, null], [null, null]];

  // Per-mon move-slot map: moves[player][teamIdx] = [moveSlug…] (up to 4)
  const moves = [[], []];
  // Per-mon already-terastallized flag.
  const teraDone = [[], []];
  for (let p = 0; p < 2; p++) {
    for (let i = 0; i < teams[p].length; i++) {
      moves[p].push([]);
      teraDone[p].push(false);
    }
  }

  function moveSlug(name) {
    return name.toLowerCase().replace(/[^a-z0-9]/g, '');
  }
  function ensureMove(player, teamIdx, slug) {
    const arr = moves[player][teamIdx];
    if (!arr.includes(slug) && arr.length < 4) arr.push(slug);
  }
  function moveSlot(player, teamIdx, slug) {
    return moves[player][teamIdx].indexOf(slug) + 1; // 1-based
  }
  function targetIdxForReplay(actorSide, actorSlot, targetSide, targetSlot) {
    // PS doubles target syntax from actor's perspective.
    if (targetSide === actorSide) {
      return -(targetSlot + 1); // ally slot
    }
    return targetSlot + 1; // foe slot
  }

  // Per-turn structure now carries:
  //   { p1: <main command>, p2: <main command>,
  //     replacements: [ { side: "p1"|"p2", slot, teamIdx }, ... ] }
  // Forced replacements (post-faint mid-turn) are SENT after the main
  // turn command and before the next turn — PS prompts for them with
  // a sideupdate of `forceSwitch: [true,...]`.
  const actions = [];
  let pending = { p1: [null, null], p2: [null, null], replacements: [] };
  // Mark a slot as "needs replacement" the moment we see its faint;
  // the NEXT non-[from] switch on that slot becomes the replacement.
  const needsReplace = [[false, false], [false, false]];

  function pushTurn() {
    function pack(actionArr) {
      const slots = gametype === 'doubles' ? 2 : 1;
      const parts = [];
      for (let s = 0; s < slots; s++) parts.push(actionArr[s] || 'pass');
      return parts.join(', ');
    }
    actions.push({
      p1: pack(pending.p1),
      p2: pack(pending.p2),
      replacements: pending.replacements,
    });
    pending = { p1: [null, null], p2: [null, null], replacements: [] };
  }

  // Track when each side terastallizes — `|-terastallize|p1a: X|Type`
  // immediately follows the `|move|` that triggered it.
  const pendingTera = [[false, false], [false, false]];

  let sawFirstTurn = false;
  // Pending tera attribution. PS may emit -terastallize a few events
  // before/after the move; we set the flag for the next move command
  // on that slot in the current turn.
  let lastTeraSlot = null;

  for (const line of lines) {
    if (!line.startsWith('|')) continue;
    const parts = line.split('|');
    const kind = parts[1];
    if (kind === 'turn') {
      if (sawFirstTurn) pushTurn();
      sawFirstTurn = true;
      continue;
    }
    const fromIdx = parts.indexOf('[from]', 3);
    const isFromCause = fromIdx > 0;
    // Pre-turn-1 lead switches: just update active[] without emitting
    // an action. PS sends `|switch|p1a: <lead>` before `|turn|1` to
    // declare the starting state.
    if (kind === 'switch' && !isFromCause) {
      const slotStr = parts[2].split(': ')[0];
      const actorPlayer = parseInt(slotStr[1]) - 1;
      const actorSlot = slotChars.indexOf(slotStr[2]);
      if (actorSlot < 0) continue;
      const details = parts[3];
      const incomingSpecies = details.split(',')[0].trim();
      const teamIdx = teamPreviewOrder[actorPlayer]
        .findIndex((d) => d.split(',')[0].trim() === incomingSpecies);
      if (teamIdx < 0) continue;
      active[actorPlayer][actorSlot] = teamIdx;
      if (sawFirstTurn) {
        const sideKey = actorPlayer === 0 ? 'p1' : 'p2';
        if (needsReplace[actorPlayer][actorSlot]) {
          // Forced replacement after a faint — PS prompts via a
          // separate sideupdate, NOT batched with the turn command.
          pending.replacements.push({
            side: sideKey,
            slot: actorSlot,
            teamIdx,
          });
          needsReplace[actorPlayer][actorSlot] = false;
        } else {
          pending[sideKey][actorSlot] = `switch ${teamIdx + 1}`;
        }
      }
      continue;
    }
    if (!sawFirstTurn) continue;

    if (kind === 'faint') {
      // |faint|p1a: NICK
      const slotStr = parts[2].split(': ')[0];
      const actorPlayer = parseInt(slotStr[1]) - 1;
      const actorSlot = slotChars.indexOf(slotStr[2]);
      if (actorSlot >= 0) {
        needsReplace[actorPlayer][actorSlot] = true;
        active[actorPlayer][actorSlot] = null;
      }
      continue;
    }

    if (kind === '-terastallize') {
      // |-terastallize|p1a: NICK|<TeraType>
      const slotStr = parts[2].split(': ')[0];
      const teraPlayer = parseInt(slotStr[1]) - 1;
      const teraSlot = slotChars.indexOf(slotStr[2]);
      if (teraSlot >= 0) lastTeraSlot = { teraPlayer, teraSlot };
      continue;
    }

    if (kind === 'move') {
      // |move|p1a: NICK|MoveName|TargetSlotOrSelf|[spread] ... |[from] ...
      const slotStr = parts[2].split(': ')[0];
      const actorPlayer = parseInt(slotStr[1]) - 1;
      const actorSlot = slotChars.indexOf(slotStr[2]);
      if (actorSlot < 0) continue;
      // Skip `[from]` move triggers — Sleep Talk, Magic Bounce, Copycat,
      // Snatch etc. These weren't ordered by the player.
      if (isFromCause) continue;
      const moveName = parts[3];
      const targetStr = parts[4] || '';
      // Detect [spread] / [notarget] / [still] markers.
      const isSpread = parts.includes('[spread]') || parts.some((p) => p && p.startsWith('[spread]'));
      const teamIdx = active[actorPlayer][actorSlot];
      if (teamIdx === null) continue;
      const slug = moveSlug(moveName);
      ensureMove(actorPlayer, teamIdx, slug);
      const slot = moveSlot(actorPlayer, teamIdx, slug);
      let targetCmd = '';
      if (gametype === 'doubles' && !isSpread && targetStr && targetStr.startsWith('p')) {
        const targetPlayer = parseInt(targetStr[1]) - 1;
        const targetSlot = slotChars.indexOf(targetStr[2]);
        // Self-target → no target index (PS rejects targets for self
        // and spread moves). Same source side WITH ally-slot OK.
        const isSelfTarget =
          targetPlayer === actorPlayer && targetSlot === actorSlot;
        if (targetSlot >= 0 && !isSelfTarget) {
          targetCmd = ' ' + targetIdxForReplay(actorPlayer, actorSlot, targetPlayer, targetSlot);
        }
      }
      let teraSuffix = '';
      if (lastTeraSlot
          && lastTeraSlot.teraPlayer === actorPlayer
          && lastTeraSlot.teraSlot === actorSlot
          && !teraDone[actorPlayer][teamIdx]) {
        teraSuffix = ' terastallize';
        teraDone[actorPlayer][teamIdx] = true;
        lastTeraSlot = null;
      }
      const sideKey = actorPlayer === 0 ? 'p1' : 'p2';
      pending[sideKey][actorSlot] = `move ${slot}${targetCmd}${teraSuffix}`;
    }
  }
  if (sawFirstTurn) pushTurn();
  // Mark `pendingTera` use to satisfy the linter (kept for future use).
  void pendingTera;

  // Pour the move slugs we observed into each set's `moves` array so PS
  // doesn't reject the move choice. If we never observed a move for a
  // mon (it sat in the back), keep the tackle placeholder.
  for (let p = 0; p < 2; p++) {
    for (let i = 0; i < teams[p].length; i++) {
      if (moves[p][i].length > 0) teams[p][i].moves = moves[p][i].slice();
    }
  }

  return { teams, actions, gametype };
}

function patchRng(stream, events) {
  // Wait until the BattleStream constructs its `battle` object on
  // `>start`, then monkey-patch the prototype. We patch the class
  // prototype (not the instance) because PS internals call through
  // `this.random` / `this.randomChance` on the Battle prototype.
  // Each call records one event; the return value is computed by the
  // PS PRNG exactly as before, so PS's behavior is unchanged.
  const origRandom = Battle.prototype.random;
  const origRandomChance = Battle.prototype.randomChance;

  Battle.prototype.random = function (m, n) {
    const v = origRandom.call(this, m, n);
    // PS `random(m, n)` semantics:
    //   no args         → 32-bit int          → emit as Tiebreak
    //   (m)             → integer in 0..m     → emit as Range or DamageRoll(if m===16)
    //   (m, n)          → integer in m..n     → emit as Range with offset baked in
    if (m === undefined && n === undefined) {
      events.push({ kind: 'Tiebreak', value: '0x' + v.toString(16) });
    } else if (m === 16 && n === undefined) {
      events.push({ kind: 'DamageRoll', value: v });
    } else if (n === undefined) {
      events.push({ kind: 'Range', value: v, bound: m });
    } else {
      // Shift to 0-based when caller used the two-arg form so the
      // engine consumer can match its own `range(n)` semantics.
      events.push({ kind: 'Range', value: v - m, bound: n - m });
    }
    return v;
  };

  Battle.prototype.randomChance = function (numerator, denominator) {
    const v = origRandomChance.call(this, numerator, denominator);
    // Crit in PS is `randomChance(1, 24)` (gen 9 base); higher crit
    // ratios use (1, 8), (1, 2), etc. The engine's `crit()` site is
    // the only `randomChance(1, 24)` in our hot loop currently.
    if (numerator === 1 && denominator === 24) {
      events.push({ kind: 'Crit', value: v });
    } else if (denominator === 100) {
      // Percent rolls (secondary fire, accuracy < 100, status chance,
      // etc.). PS emits these as randomChance(numerator, 100).
      events.push({ kind: 'PercentRoll', value: v, threshold: numerator });
    } else {
      events.push({ kind: 'Chance', value: v, num: numerator, denom: denominator });
    }
    return v;
  };

  // Restore on stream end so re-runs in the same process don't leak.
  const restore = () => {
    Battle.prototype.random = origRandom;
    Battle.prototype.randomChance = origRandomChance;
  };
  return restore;
}

async function runJob(job) {
  if (!Array.isArray(job.teams) || job.teams.length !== 2) {
    throw new Error('job.teams must be [team1, team2]');
  }
  const seed = job.seed || [1, 2, 3, 4];
  const format = job.format || 'gen9customgame';
  const events = [];

  const stream = new BattleStream();
  const restore = patchRng(stream, events);

  const logChunks = [];
  const drain = (async () => {
    for await (const chunk of stream) {
      logChunks.push(chunk);
    }
  })();

  try {
    stream.write('>start ' + JSON.stringify({ formatid: format, seed }));
    stream.write('>player p1 ' + JSON.stringify({
      name: 'P1', team: Teams.pack(job.teams[0]),
    }));
    stream.write('>player p2 ' + JSON.stringify({
      name: 'P2', team: Teams.pack(job.teams[1]),
    }));

    const isDoubles = job.gametype === 'doubles' || /doubles/i.test(format);
    // Team Preview: send full team order (1..N).
    const order1 = job.teams[0].map((_, i) => i + 1).join('');
    const order2 = job.teams[1].map((_, i) => i + 1).join('');
    stream.write('>p1 team ' + order1);
    stream.write('>p2 team ' + order2);

    const actions = job.actions || [];
    let turns = 0;
    const sent = [];
    for (const turn of actions) {
      if (turn.p1) { stream.write('>p1 ' + turn.p1); sent.push('>p1 ' + turn.p1); }
      if (turn.p2) { stream.write('>p2 ' + turn.p2); sent.push('>p2 ' + turn.p2); }
      // Forced replacements (post-faint mid-turn). Group by side so we
      // send each side's replacement command in one shot — PS expects
      // `>p1 switch N` for a single forced switch (singles or doubles
      // with one slot needing replacement).
      if (turn.replacements && turn.replacements.length) {
        const byP1 = turn.replacements.filter((r) => r.side === 'p1');
        const byP2 = turn.replacements.filter((r) => r.side === 'p2');
        if (byP1.length) {
          const cmd = byP1.map((r) => `switch ${r.teamIdx + 1}`).join(', ');
          stream.write('>p1 ' + cmd); sent.push('>p1 ' + cmd);
        }
        if (byP2.length) {
          const cmd = byP2.map((r) => `switch ${r.teamIdx + 1}`).join(', ');
          stream.write('>p2 ' + cmd); sent.push('>p2 ' + cmd);
        }
      }
      turns += 1;
    }
    if (job.debugCommands) {
      process.stderr.write('SENT COMMANDS:\n' + sent.join('\n') + '\n');
    }
    // Politely end the battle write side.
    stream.writeEnd();
    await drain;

    return {
      ok: true,
      seed,
      turns,
      events,
      log: logChunks.join(''),
    };
  } finally {
    restore();
  }
}

(async () => {
  try {
    const raw = await readStdin();
    if (!raw.trim()) {
      throw new Error('empty stdin (expected JSON job)');
    }
    const job = JSON.parse(raw);
    if (job.replay) {
      // job.replay is the entire replay JSON (id, log, …). Extract
      // teams + actions in-process, then run.
      const extracted = extractFromReplay(job.replay);
      job.teams = extracted.teams;
      job.actions = extracted.actions;
      job.gametype = extracted.gametype;
      // gen9customgame is permissive; corpus replays use a format slug
      // like `[Gen 9] Champions VGC 2026 Reg M-A` that we don't have
      // a local data overlay for. customgame lets us drive any
      // dex-legal set.
      job.format = job.format || (extracted.gametype === 'doubles'
        ? 'gen9doublescustomgame'
        : 'gen9customgame');
    }
    const result = await runJob(job);
    process.stdout.write(JSON.stringify(result));
  } catch (err) {
    process.stderr.write(String(err && err.stack || err) + '\n');
    process.stdout.write(JSON.stringify({ ok: false, error: String(err && err.message || err) }));
    process.exit(1);
  }
})();
