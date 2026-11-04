#!/usr/bin/env node
// Builds tools/protocol/ps-suite/scenarios.jsonl: battles from Showdown's own test suite and
// extra.jsonl, with Showdown's output, for the conformance test ps_suite.rs.
//
// 1. Capture (once per Showdown build), from the Showdown checkout:
//      CORPUS_OUT=/tmp/raw.jsonl npx mocha --no-config --exit --ui bdd \
//        --require <repo>/tools/protocol/ps-suite-capture.cjs test/main.js 'test/sim/**/*.js'
// 2. Build. Keeps gen 9 doubles and Champions battles that Showdown itself reproduces from the
//    input log, and whose moves, abilities and items exist in Reg M-C:
//      PS_DIST=<ps>/dist/sim VGC_BIN=<vgc> node tools/protocol/ps-suite.js /tmp/raw.jsonl
//    A scenario the engine doesn't match yet goes to known-gaps.jsonl instead of the gate.
'use strict';
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const PS_DIST = process.env.PS_DIST;
if (!PS_DIST) throw new Error('set PS_DIST');
const { BattleStream, Dex, Teams } = require(path.join(PS_DIST, 'index.js'));
const OUT = path.join(__dirname, 'ps-suite');
const VGC = process.env.VGC_BIN;
if (!VGC) throw new Error('set VGC_BIN');

const MESSAGE_BREAK = /\n\n(?=(?:update|sideupdate|end|requesteddata)\n)/;
const norm = s => s.replace(/\|t:\|\d+/g, '|t:|');
const dex = Dex.forFormat(Dex.formats.get('gen9championsvgc2026regmc'));
const available = x => x && x.exists && !x.isNonstandard;

function formatFor(row) {
	if (!/^gen9(anythinggoes|championscustomgame|customgame|doublescustomgame|championsdoublescustomgame)/.test(row.format)) return null;
	const champions = row.format.includes('champions');
	const doubles = row.gameType === 'doubles';
	if (!['singles', 'doubles'].includes(row.gameType)) return null;
	return champions ? (doubles ? 'gen9championsdoublescustomgame' : 'gen9championscustomgame') :
		(doubles ? 'gen9doublescustomgame' : 'gen9customgame');
}

function inReg(log) {
	for (const line of log.filter(x => x.startsWith('>player '))) {
		for (const set of Teams.unpack(JSON.parse(line.slice(line.indexOf('{'))).team) || []) {
			if (set.ability && set.ability !== 'No Ability' && !available(dex.abilities.get(set.ability))) return false;
			if (set.item && !available(dex.items.get(set.item))) return false;
			for (const move of set.moves || []) if (!available(dex.moves.get(move))) return false;
		}
	}
	return true;
}

function runShowdown(log) {
	const stream = new BattleStream();
	const messages = [];
	stream.push = m => messages.push(m);
	for (const line of log) stream.write(line);
	return messages.map(m => norm(m).replace(/\n+$/, ''));
}

function runEngine(log) {
	return new Promise(resolve => {
		const child = spawn(VGC, ['sim']);
		let out = '';
		let barriers = 0;
		let scanned = 0;
		let wake = null;
		child.stdout.setEncoding('utf8');
		child.stdout.on('data', d => {
			out += d;
			for (;;) {
				const at = out.indexOf('requesteddata\n', scanned);
				if (at < 0) break;
				if (at === 0 || out.startsWith('\n\n', at - 2)) barriers++;
				scanned = at + 1;
			}
			if (wake) wake();
		});
		child.stdin.on('error', () => {});
		child.on('close', () => {
			if (wake) wake();
			resolve(out.split(MESSAGE_BREAK).map(m => norm(m).replace(/\n+$/, '')).filter(m => m && !m.startsWith('requesteddata')));
		});
		(async () => {
			for (let i = 0; i < log.length; i++) {
				child.stdin.write(`${log[i]}\n${i ? '>requestteam p1' : ''}\n`);
				const started = Date.now();
				while (i && barriers < i && child.exitCode === null && !out.includes('\nend\n') && Date.now() - started < 3000) {
					await new Promise(r => {
						wake = r;
						setTimeout(r, 20);
					});
				}
			}
			child.stdin.end();
		})();
	});
}

function fromCapture(row) {
	const format = formatFor(row);
	if (!format || row.gen !== 9) return null;
	const start = JSON.parse(row.input_log[0].slice('>start '.length));
	start.formatid = format;
	const rest = row.input_log.slice(1);
	const players = rest.filter(x => x.startsWith('>player '));
	if (players.length < 2) return null;
	const size = x => (JSON.parse(x.slice(x.indexOf('{'))).team.match(/\]/g) || []).length + 1;
	const order = n => Array.from({ length: n }, (_, i) => i + 1).join(n >= 10 ? ',' : '');
	return {
		game: row.test, format, input_log: [
			`>start ${JSON.stringify(start)}`, ...players,
			`>p1 team ${order(size(players[0]))}`, `>p2 team ${order(size(players[1]))}`,
			...rest.filter(x => !x.startsWith('>player ')),
		],
	};
}

// The same battle facts on both sides, without message boundaries: all writes to `vgc sim` wait
// for a reply, but a chunk can still group lines differently.
function flat(messages) {
	const log = [];
	const requests = { p1: [], p2: [] };
	let end = null;
	for (const m of messages) {
		const [kind, ...rest] = m.split('\n');
		if (kind === 'update') {
			log.push(...rest.filter(l => l && l !== '|'));
		} else if (kind === 'sideupdate') {
			if (rest[1] && rest[1].startsWith('|request|')) requests[rest[0]].push(rest[1]);
		} else if (kind === 'end') {
			end = JSON.parse(rest.join('\n'));
			delete end.inputLog;
		}
	}
	return { log, requests, end };
}

function reproduces(scenario, capturedLog) {
	const body = lines => {
		const shown = lines.filter(l => l && l !== '|' && !l.startsWith('|t:|') && !l.startsWith('|debug|') && !l.startsWith('|split|'));
		const i = shown.indexOf('|start');
		return shown.slice(i < 0 ? 0 : i).map(l => l.replace(/\b\d+\/\d+\b/g, 'H')).filter((l, k, a) => l !== a[k - 1]);
	};
	return JSON.stringify(body(flat(runShowdown(scenario.input_log)).log)) === JSON.stringify(body(capturedLog));
}

(async () => {
	const rows = fs.readFileSync(process.argv[2], 'utf8').split('\n').filter(Boolean).map(JSON.parse);
	const seen = new Set();
	const scenarios = [];
	const skipped = {};
	const skip = why => { skipped[why] = (skipped[why] || 0) + 1; };
	for (const row of rows) {
		const s = fromCapture(row);
		if (!s) { skip('format'); continue; }
		if (seen.has(s.game)) { skip('duplicate'); continue; }
		if (!inReg(s.input_log)) { skip('outside Reg M-C'); continue; }
		seen.add(s.game);
		let ok = false;
		try { ok = reproduces(s, row.log); } catch {}
		if (ok) scenarios.push(s); else skip('not replayable');
	}
	const extra = path.join(OUT, 'extra.jsonl');
	if (fs.existsSync(extra)) {
		for (const line of fs.readFileSync(extra, 'utf8').split('\n').filter(Boolean)) scenarios.push(JSON.parse(line));
	}
	const pass = [];
	const gaps = [];
	for (const s of scenarios) {
		const expected = flat(runShowdown(s.input_log));
		const got = flat(await runEngine(s.input_log));
		(JSON.stringify(expected) === JSON.stringify(got) ? pass : gaps).push({ ...s, expected });
	}
	fs.writeFileSync(path.join(OUT, 'scenarios.jsonl'), pass.map(x => JSON.stringify(x)).join('\n') + '\n');
	fs.writeFileSync(path.join(OUT, 'known-gaps.jsonl'), gaps.map(x => JSON.stringify({ game: x.game, format: x.format })).join('\n') + '\n');
	console.log(`${pass.length} scenarios in the gate, ${gaps.length} known gaps`, skipped);
})();
