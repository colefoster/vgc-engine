'use strict';
/**
 * pokemon-showdown throughput benchmark.
 *
 * Runs N random battles to completion under a fixed PRNG seed and reports
 * battles/sec, turns/sec (a "turn" is PS's per-side decision cycle, the closest
 * analogue to vgc-engine's step()), and per-turn latency.
 *
 * This drives PS through the `sim` module directly (BattleStream + the bundled
 * RandomPlayerAI, the same harness PS itself uses in sim/tools/runner.ts) rather
 * than the network protocol, for clean throughput numbers. Note that PS still
 * does full protocol-string formatting on every event -- that overhead is
 * intrinsic to PS and is documented as an asymmetry vs. vgc-engine.
 *
 * Usage:
 *   node ps_bench.js [--ps <path>] [--battles N] [--format ID] [--seed s1,s2,s3,s4]
 *
 * Requires PS to be built first (`node build` in the PS checkout).
 *
 * Source patterns: pokemon-showdown sim/tools/runner.ts (Runner.runGame),
 * sim/tools/random-player-ai.ts (RandomPlayerAI), sim/battle-stream.ts
 * (getPlayerStreams).
 */

function parseArgs(argv) {
	const out = {
		ps: process.env.PS_PATH || '/tmp/pokemon-showdown-research',
		battles: 100,
		format: 'gen9randomdoublesbattle',
		seed: '1,2,3,4',
		warmup: 5,
	};
	for (let i = 2; i < argv.length; i++) {
		const a = argv[i];
		if (a === '--ps') out.ps = argv[++i];
		else if (a === '--battles') out.battles = parseInt(argv[++i], 10);
		else if (a === '--format') out.format = argv[++i];
		else if (a === '--seed') out.seed = argv[++i];
		else if (a === '--warmup') out.warmup = parseInt(argv[++i], 10);
	}
	return out;
}

async function main() {
	const opts = parseArgs(process.argv);
	const path = require('path');
	const simDir = path.join(opts.ps, 'dist', 'sim');
	let BattleStreams, RandomPlayerAI, PRNG, Dex;
	try {
		BattleStreams = require(path.join(simDir, 'battle-stream.js'));
		RandomPlayerAI = require(path.join(opts.ps, 'dist', 'sim', 'tools', 'random-player-ai.js')).RandomPlayerAI;
		PRNG = require(path.join(simDir, 'prng.js')).PRNG;
		Dex = require(path.join(simDir, '..', 'sim', 'index.js')).Dex || require(path.join(opts.ps, 'dist', 'sim', 'index.js')).Dex;
	} catch (e) {
		console.error('Failed to load PS sim modules. Did you build PS? (cd ' + opts.ps + ' && node build)');
		console.error(e.message);
		process.exit(2);
	}

	// Master PRNG drives per-battle seeds deterministically.
	const seedArr = opts.seed.split(',').map(Number);
	const master = new PRNG(seedArr);
	const newSeed = () => [
		master.random(2 ** 16), master.random(2 ** 16),
		master.random(2 ** 16), master.random(2 ** 16),
	];

	async function runOneBattle(format) {
		const battleStream = new BattleStreams.BattleStream();
		const streams = BattleStreams.getPlayerStreams(battleStream);
		const spec = { formatid: format, seed: newSeed() };
		const p1 = new RandomPlayerAI(streams.p1, { seed: newSeed() });
		const p2 = new RandomPlayerAI(streams.p2, { seed: newSeed() });
		void p1.start();
		void p2.start();
		const p1spec = { name: 'Bot 1', seed: newSeed() };
		const p2spec = { name: 'Bot 2', seed: newSeed() };
		void streams.omniscient.write(
			`>start ${JSON.stringify(spec)}\n` +
			`>player p1 ${JSON.stringify(p1spec)}\n` +
			`>player p2 ${JSON.stringify(p2spec)}`
		);
		let turns = 0;
		for await (const chunk of streams.omniscient) {
			// Count |turn|N markers; the highest turn number == battle length.
			const lines = chunk.split('\n');
			for (const line of lines) {
				if (line.startsWith('|turn|')) turns++;
			}
		}
		return turns;
	}

	// Warmup (JIT) — not counted.
	for (let i = 0; i < opts.warmup; i++) await runOneBattle(opts.format);

	let totalTurns = 0;
	const t0 = process.hrtime.bigint();
	for (let i = 0; i < opts.battles; i++) {
		totalTurns += await runOneBattle(opts.format);
	}
	const t1 = process.hrtime.bigint();

	const elapsedNs = Number(t1 - t0);
	const elapsedS = elapsedNs / 1e9;
	const battlesPerSec = opts.battles / elapsedS;
	const turnsPerSec = totalTurns / elapsedS;
	const nsPerTurn = elapsedNs / totalTurns;

	const result = {
		engine: 'pokemon-showdown',
		format: opts.format,
		battles: opts.battles,
		total_turns: totalTurns,
		avg_turns_per_battle: totalTurns / opts.battles,
		elapsed_s: elapsedS,
		battles_per_sec: battlesPerSec,
		turns_per_sec: turnsPerSec,
		ns_per_turn: nsPerTurn,
	};
	// Machine-readable line for the comparison harness, prefixed for easy grep.
	console.log('PSBENCH_JSON ' + JSON.stringify(result));
	console.error(
		`pokemon-showdown: ${opts.battles} battles, ${totalTurns} turns in ${elapsedS.toFixed(3)}s\n` +
		`  ${battlesPerSec.toFixed(1)} battles/s | ${turnsPerSec.toFixed(0)} turns/s | ${nsPerTurn.toFixed(0)} ns/turn`
	);
}

main().catch(e => { console.error(e); process.exit(1); });
