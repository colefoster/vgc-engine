#!/usr/bin/env node
'use strict';
/**
 * PS side of consumer-rollout-bench.py: a steady-state Node process that runs
 * the PS analogue of mimikyu's `RolloutOracle._one_rollout` on request.
 *
 * Protocol: one JSON request per stdin line, one JSON response per stdout
 * line. Diagnostics go to stderr.
 *   {"op":"init","format":..,"p1":[set..],"p2":[set..],"init_seed":"a,b,c,d"}
 *       builds the base battle once and plays team preview (`team 12`).
 *   {"op":"run","seeds":[..],"p1_initial":"move 1 1, move 1 2",
 *    "p2_initial":"move 1, move 1","own_side":0,"max_turns":8,"record":false}
 *       one rollout per seed; returns per-rollout outcomes plus in-process ns.
 *   {"op":"exit"}
 *
 * One rollout mirrors `_one_rollout`: serialize the base battle, set the PRNG
 * seed in the serialized state (`reseed_battle` edits the engine JSON's RNG
 * the same way), deserialize, play the explicit initial joint action, then
 * the fixed policy for both sides while the battle continues and
 * `t < max_turns`; a truncated game scores the same HP-fraction soft value.
 *
 * Fixed PS policy (explicit, independent of the engine's legal_choices
 * order): each living active uses move 1, at the first living foe position
 * (1, then 2) when the move takes a target; fainted actives pass (PS
 * auto-passes them, sim/side.ts:1303-1316).
 *
 * PS a5df8274: Battle.toJSON / Battle.fromJSON (sim/battle.ts:318-323),
 * State.serializeBattle / deserializeBattle with `state.prng` restored at
 * sim/state.ts:68,149.
 *
 * Env: PS_DIST — built pokemon-showdown `dist/sim`.
 */

// Offline guard: initialise the stdio streams first (they are net.Sockets
// on pipes), then make every outbound network / subprocess entry point throw.
const stdin = process.stdin;
const stdout = process.stdout;
function block(modName, fns) {
	const mod = require(modName);
	for (const fn of fns) {
		if (typeof mod[fn] === 'function') {
			mod[fn] = () => { throw new Error(`offline guard: ${modName}.${fn} is blocked`); };
		}
	}
}
block('net', ['connect', 'createConnection', 'createServer']);
block('tls', ['connect', 'createServer']);
block('http', ['request', 'get', 'createServer']);
block('https', ['request', 'get', 'createServer']);
block('http2', ['connect', 'createServer', 'createSecureServer']);
block('dgram', ['createSocket']);
block('dns', ['lookup', 'resolve', 'resolve4', 'resolve6', 'resolveAny']);
block('child_process', ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']);

const PS_DIST = process.env.PS_DIST;
if (!PS_DIST) {
	process.stderr.write('consumer-rollout-ps: PS_DIST is not set\n');
	process.exit(2);
}
const { Battle } = require(PS_DIST + '/battle');
const { Dex } = require(PS_DIST);

let base = null;
let format = null;

function toSet(s) {
	const zero = { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 };
	const max = { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 };
	return {
		name: s.species, species: s.species, gender: s.gender || '', item: s.item || '',
		ability: s.ability, nature: s.nature, level: s.level, moves: s.moves,
		evs: { ...zero, ...(s.evs || {}) }, ivs: { ...max, ...(s.ivs || {}) },
	};
}

function hpFrac(side) {
	let t = 0;
	for (const p of side.pokemon) t += p.hp / (p.maxhp || 1);
	return t;
}

function softValue(b, own) {
	const ho = hpFrac(b.sides[own]);
	const hp = hpFrac(b.sides[1 - own]);
	if (ho + hp <= 0) return 0.5;
	return Math.max(0, Math.min(1, 0.5 + 0.5 * (ho - hp) / Math.max(ho + hp, 1e-9)));
}

function result(b) {
	if (!b.ended) return 'continue';
	if (b.winner === b.sides[0].name) return 'p1_win';
	if (b.winner === b.sides[1].name) return 'p2_win';
	return 'tie';
}

function score(res, own) {
	if (res === 'tie') return 0.5;
	if (res === 'p1_win') return own === 0 ? 1 : 0;
	if (res === 'p2_win') return own === 0 ? 0 : 1;
	return null;
}

const NEEDS_TARGET = new Set(['normal', 'any', 'adjacentFoe', 'adjacentAlly', 'adjacentAllyOrSelf']);

/** The fixed policy's per-slot tokens for one side (fainted -> 'pass'). */
function policy(b, sideIdx) {
	const side = b.sides[sideIdx];
	const foe = b.sides[1 - sideIdx];
	const tokens = [];
	for (const p of side.active) {
		if (!p || p.fainted) { tokens.push('pass'); continue; }
		const move = Dex.moves.get(p.moveSlots[0].id);
		let tok = 'move 1';
		if (NEEDS_TARGET.has(move.target)) {
			const i = foe.active.findIndex((f) => f && !f.fainted);
			if (i >= 0) tok += ` ${i + 1}`;
		}
		tokens.push(tok);
	}
	return tokens;
}

/** PS choice string: living actives only, since PS auto-passes fainted ones. */
function choiceString(tokens) {
	return tokens.filter((t) => t !== 'pass').join(', ') || 'pass';
}

function finalHp(b) {
	return b.sides.map((s) => s.pokemon.map((p) => [p.species.id, p.hp, p.maxhp]));
}

function rollout(seed, req) {
	const d = JSON.parse(JSON.stringify(base.toJSON()));
	d.prng = seed;
	const b = Battle.fromJSON(JSON.stringify(d));
	const rec = req.record ? [[req.p1_initial, req.p2_initial]] : null;
	b.makeChoices(req.p1_initial, req.p2_initial);
	let steps = 1;
	let t = 0;
	while (!b.ended && t < req.max_turns) {
		const a = policy(b, 0);
		const c = policy(b, 1);
		if (rec) rec.push([a.join(', '), c.join(', ')]);
		b.makeChoices(choiceString(a), choiceString(c));
		steps++;
		t++;
	}
	const res = result(b);
	let value = score(res, req.own_side);
	const decisive = value !== null;
	if (!decisive) value = softValue(b, req.own_side);
	const out = { seed, result: res, steps, value, decisive };
	if (rec) {
		out.choices = rec;
		out.final_hp = finalHp(b);
		out.turn = b.turn;
	}
	return out;
}

function handle(req) {
	if (req.op === 'init') {
		format = req.format;
		base = new Battle({
			formatid: req.format, seed: req.init_seed,
			p1: { name: 'P1', team: req.p1.map(toSet) },
			p2: { name: 'P2', team: req.p2.map(toSet) },
		});
		base.makeChoices('team ' + '123456'.slice(0, req.p1.length), 'team ' + '123456'.slice(0, req.p2.length));
		return {
			ok: true, turn: base.turn, request_state: base.requestState, final_hp: finalHp(base),
			stats: base.sides.map((s) => s.pokemon.map((p) => [p.species.id, p.maxhp, p.storedStats])),
			node: process.version, format: base.format.id, mod: base.dex.currentMod,
		};
	}
	if (req.op === 'run') {
		if (!base) throw new Error('run before init');
		const t0 = process.hrtime.bigint();
		const rollouts = req.seeds.map((s) => rollout(s, req));
		const ns = Number(process.hrtime.bigint() - t0);
		return { ok: true, internal_ns: ns, rollouts };
	}
	throw new Error(`unknown op ${req.op}`);
}

const rl = require('readline').createInterface({ input: stdin });
rl.on('line', (line) => {
	if (!line.trim()) return;
	let req;
	try {
		req = JSON.parse(line);
	} catch (e) {
		stdout.write(JSON.stringify({ ok: false, error: `bad json: ${e.message}` }) + '\n');
		return;
	}
	if (req.op === 'exit') {
		rl.close();
		return;
	}
	let resp;
	try {
		resp = handle(req);
	} catch (e) {
		resp = { ok: false, error: String((e && e.stack) || e) };
	}
	stdout.write(JSON.stringify(resp) + '\n');
});
rl.on('close', () => process.exit(0));
