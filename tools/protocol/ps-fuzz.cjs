#!/usr/bin/env node
// Random legal Reg M-C teams played by Showdown's random player, written as input logs that
// mimikyu's ps_bot/scripts/arena/sim_check.mjs can replay on Showdown and the engine.
//   PS_DIST=<ps>/dist/sim node tools/protocol/ps-fuzz.cjs <games> <seed> <out.inputs.jsonl>
// Sets are chosen uniformly over legal species, moves, abilities and items, not by usage, so
// rare mechanics come up far more often than on the ladder.
'use strict';
const fs = require('fs');
const path = require('path');

const PS_DIST = process.env.PS_DIST;
if (!PS_DIST) throw new Error('set PS_DIST');
const { Dex, TeamValidator, Teams, PRNG } = require(path.join(PS_DIST, 'index.js'));
const BattleStreams = require(path.join(PS_DIST, 'battle-stream.js'));
const { RandomPlayerAI } = require(path.join(PS_DIST, 'tools/random-player-ai.js'));

const FORMAT = 'gen9championsvgc2026regmc';
const dex = Dex.forFormat(Dex.formats.get(FORMAT));
const validator = new TeamValidator(FORMAT);
const NATURES = dex.natures.all().map(n => n.name);

function statPoints(prng) {
	const evs = { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 };
	const keys = Object.keys(evs);
	let left = 66;
	while (left > 0) {
		const k = prng.sample(keys);
		const add = Math.min(left, prng.random(1, 17), 32 - evs[k]);
		evs[k] += add;
		left -= add;
		if (keys.every(x => evs[x] === 32)) break;
	}
	return evs;
}

// Species that validate alone, with the moves each one can legally carry.
function legalPool() {
	const pool = [];
	for (const s of dex.species.all()) {
		if (!s.exists || s.isNonstandard || s.battleOnly || s.requiredItem || s.forme === 'Mega') continue;
		const base = { species: s.name, name: s.name, level: 50, nature: 'Serious', item: '', evs: { hp: 32, atk: 32, def: 2, spa: 0, spd: 0, spe: 0 }, ivs: { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 } };
		const abilities = Object.values(s.abilities).filter(a => !validator.validateSet({ ...base, ability: a, moves: ['protect'] }, {}));
		if (!abilities.length) continue;
		const moves = [...dex.species.getMovePool(s.id)].map(m => dex.moves.get(m)).filter(m => m.exists && !m.isNonstandard)
			.filter(m => !validator.validateSet({ ...base, ability: abilities[0], moves: [m.id] }, {})).map(m => m.name);
		if (moves.length) pool.push({ name: s.name, abilities, moves });
	}
	return pool;
}

const ITEMS = dex.items.all().filter(i => i.exists && !i.isNonstandard);

function randomTeam(pool, prng) {
	for (;;) {
		const used = new Set();
		const items = new Set();
		const team = [];
		while (team.length < 6) {
			const s = prng.sample(pool);
			const dexNum = dex.species.get(s.name).num;
			if (used.has(dexNum)) continue;
			used.add(dexNum);
			const moves = [];
			while (moves.length < Math.min(4, s.moves.length)) {
				const m = prng.sample(s.moves);
				if (!moves.includes(m)) moves.push(m);
			}
			const stones = ITEMS.filter(i => i.megaStone && Object.keys(i.megaStone).includes(s.name) && !items.has(i.name));
			const held = stones.length && prng.random(2) ? prng.sample(stones) : prng.sample(ITEMS.filter(i => !items.has(i.name) && !i.megaStone));
			items.add(held.name);
			team.push({ name: s.name, species: s.name, item: held.name, ability: prng.sample(s.abilities), moves, nature: prng.sample(NATURES), evs: statPoints(prng), ivs: { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 }, level: 50, gender: '' });
		}
		if (!validator.validateTeam(team)) return team;
	}
}

async function play(p1, p2, seed, aiSeeds) {
	const stream = new BattleStreams.BattleStream();
	const streams = BattleStreams.getPlayerStreams(stream);
	new RandomPlayerAI(streams.p1, { seed: aiSeeds[0], mega: 0.5 }).start();
	new RandomPlayerAI(streams.p2, { seed: aiSeeds[1], mega: 0.5 }).start();
	streams.omniscient.write(`>start ${JSON.stringify({ formatid: FORMAT, seed })}\n>player p1 ${JSON.stringify({ name: 'p1', team: Teams.pack(p1) })}\n>player p2 ${JSON.stringify({ name: 'p2', team: Teams.pack(p2) })}`);
	// The stream drops its battle when the game ends, so keep a reference from the first chunk.
	let battle = null;
	for await (const chunk of streams.omniscient) {
		void chunk;
		battle = battle || stream.battle;
	}
	return battle ? battle.inputLog : null;
}

(async () => {
	const [games, seed, out] = [parseInt(process.argv[2], 10), parseInt(process.argv[3], 10), process.argv[4]];
	const prng = new PRNG([seed, seed + 1, seed + 2, seed + 3]);
	const pool = legalPool();
	const seen = { moves: new Set(), abilities: new Set(), items: new Set() };
	const file = fs.createWriteStream(out);
	for (let game = 0; game < games; game++) {
		const [p1, p2] = [randomTeam(pool, prng), randomTeam(pool, prng)];
		for (const set of [...p1, ...p2]) {
			set.moves.forEach(m => seen.moves.add(m));
			seen.abilities.add(set.ability);
			seen.items.add(set.item);
		}
		const draw = () => prng.random(65536);
		const input_log = await play(p1, p2, [draw(), draw(), draw(), draw()].join(','), [[draw(), draw(), draw(), draw()], [draw(), draw(), draw(), draw()]]);
		if (input_log) file.write(JSON.stringify({ game, input_log }) + '\n');
	}
	file.end();
	console.log(`${games} games, ${pool.length} species, ${seen.moves.size} moves, ${seen.abilities.size} abilities, ${seen.items.size} items`);
})();
