#!/usr/bin/env node
// Finds the first PRNG draw where Showdown and `vgc sim` part ways on one input log, and whether
// that comes before the first visible log difference (a hidden draw) or not (a real number bug).
//
//   PS_DIST=<ps>/dist/sim VGC_BIN=<vgc> node tools/protocol/draw-trace.cjs <log> [--game N] [--around K] [--out f.json]
//   PS_DIST=<ps>/dist/sim VGC_BIN=<vgc> node tools/protocol/draw-trace.cjs --batch <sim_check dir | inputs.jsonl> [--tag -damage] [--out f.jsonl]
//
// <log> is a sim_check diff/<id>.in (one write per line) or an .inputs.jsonl (--game picks the row,
// default the first). The engine side is `VGC_DRAW_SITES`, off unless asked for: `vgc` built with
// the default `ps-rng` feature keeps no trace otherwise.
//
// Both sims run the same stream, so the raw value at index i is equal by construction; two draws
// differ in their arguments or, when the arguments match, in which code drew them. `kind`:
//   hidden   arguments differ at a draw made before the first differing log line
//   visible  no argument difference before that line: the log line itself is wrong
//   draws    logs are equal and the draws differ
//   same     nothing differs
'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');

const PS_DIST = process.env.PS_DIST;
const VGC_BIN = process.env.VGC_BIN || 'vgc';
if (!PS_DIST) throw new Error('set PS_DIST to <ps>/dist/sim');
const { BattleStream } = require(path.join(PS_DIST, 'battle-stream.js'));
const prngModule = require(path.join(PS_DIST, 'prng.js'));

// --- Showdown side: every PRNG.random call goes through this. ---
let recording = null;
let lastRaw = 0;
for (const cls of [prngModule.SodiumRNG, prngModule.Gen5RNG]) {
	const next = cls.prototype.next;
	cls.prototype.next = function () { lastRaw = next.call(this); return lastRaw; };
}
const origRandom = prngModule.PRNG.prototype.random;
Error.stackTraceLimit = 24;
function callers(stack) {
	const out = [];
	for (const line of stack.split('\n').slice(1)) {
		const m = /at (?:(.+?) \()?(.*?):(\d+):\d+\)?$/.exec(line.trim());
		if (!m) continue;
		const file = path.basename(m[2]);
		if (file === 'prng.js' || file === path.basename(__filename)) continue;
		if (!m[2].includes('dist/')) continue;
		out.push(`${(m[1] || '?').replace(/^(?:async )?/, '')} ${file}:${m[3]}`);
		if (out.length >= 8) break;
	}
	return out;
}
prngModule.PRNG.prototype.random = function (a, b) {
	const value = origRandom.call(this, a, b);
	if (recording) {
		const battle = recording.stream.battle;
		recording.draws.push({
			i: recording.draws.length,
			args: b ? [a, b] : a === undefined ? [] : [a],
			span: b ? b - a : (a || 0),
			raw: lastRaw,
			value,
			turn: battle ? battle.turn : 0,
			line: battle ? battle.log.length : 0,
			move: battle && battle.activeMove ? battle.activeMove.id : null,
			stack: callers(new Error().stack),
			sort: sorting,
		});
	}
	return value;
};

// A speedSort draw is about ties in the sorted list, so keep what was tied. The shuffle's draws
// come out of Battle.speedSort, one frame below this wrapper's caller.
const { Battle } = require(path.join(PS_DIST, 'battle.js'));
const origSpeedSort = Battle.prototype.speedSort;
let sorting = null;
function sortLabel(x) {
	const holder = x.effectHolder ? ` @${x.effectHolder.name || x.effectHolder.id || '?'}` : '';
	const what = x.effect ? x.effect.id : x.pokemon ? `${x.choice}:${x.pokemon.name}` : x.name || '?';
	return `${what}${holder}[o${x.order} p${x.priority} s${x.speed} sub${x.subOrder}]`;
}
Battle.prototype.speedSort = function (list, comparator) {
	const saved = sorting;
	sorting = recording ? list.map(sortLabel) : null;
	try {
		return origSpeedSort.call(this, list, comparator);
	} finally {
		sorting = saved;
	}
};

const norm = (text) => text.replace(/\|t:\|\d+/g, '|t:|');
const MESSAGE_BREAK = /\n\n(?=(?:update|sideupdate|end|requesteddata)\n)/;
function logLines(messages) {
	const log = [];
	for (const raw of messages) {
		const [kind, ...rest] = norm(raw).replace(/\n+$/, '').split('\n');
		if (kind === 'update') log.push(...rest);
	}
	return log;
}

function runShowdown(inputLog) {
	const stream = new BattleStream();
	const messages = [];
	stream.push = (m) => { messages.push(m); };
	const rec = { stream, draws: [] };
	recording = rec;
	try {
		for (const line of inputLog) stream.write(line);
	} finally {
		recording = null;
	}
	return { draws: rec.draws, log: logLines(messages) };
}

// One write per read, as sim_check.mjs feeds it, so the engine sees what PS's stream sees.
function runEngine(inputLog) {
	const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'draw-trace-'));
	const spans = path.join(tmp, 'spans');
	const sites = path.join(tmp, 'sites');
	return new Promise((resolve, reject) => {
		const child = spawn(VGC_BIN, ['sim'], {
			stdio: ['pipe', 'pipe', 'pipe'],
			env: { ...process.env, VGC_DRAW_TRACE: spans, VGC_DRAW_SITES: sites },
		});
		let out = '';
		let err = '';
		let barriers = 0;
		let scanned = 0;
		let ended = false;
		let wake = null;
		child.stdout.setEncoding('utf8');
		child.stdout.on('data', (d) => {
			out += d;
			for (;;) {
				const at = out.indexOf('requesteddata\n', scanned);
				if (at < 0) break;
				if (at === 0 || out.startsWith('\n\n', at - 2)) barriers += 1;
				scanned = at + 1;
			}
			ended = ended || out.includes('\n\nend\n') || out.startsWith('end\n');
			if (wake) wake();
		});
		child.stderr.on('data', (d) => { err += d; });
		child.stdin.on('error', () => {});
		child.on('error', reject);
		child.on('close', (code) => {
			if (wake) wake();
			if (code) return reject(new Error(`vgc sim exited ${code}: ${err.slice(0, 200)}`));
			let text = '';
			try { text = fs.readFileSync(sites, 'utf8'); } catch {}
			fs.rmSync(tmp, { recursive: true, force: true });
			const draws = text.split('\n').filter(Boolean).map((row, i) => {
				const [span, op, site, a, b, raw, result, actor, moveId, target, decision] = row.split(' ');
				return { i, span: +span, op, site, args: +span ? [+a, +b] : [], raw: +raw, result: +result, actor: +actor, moveId: +moveId, target: +target, decision };
			});
			resolve({ draws, log: logLines(out.split(MESSAGE_BREAK)) });
		});
		(async () => {
			for (let i = 0; i < inputLog.length; i++) {
				child.stdin.write(`${inputLog[i]}\n${i ? '>requestteam p1' : ''}\n`);
				const started = Date.now();
				while (i && !ended && barriers < i && child.exitCode === null) {
					if (Date.now() - started > 5000) { child.kill(); return; }
					await new Promise((r) => { wake = r; setTimeout(r, 50); });
				}
			}
			child.stdin.end();
		})();
	});
}

function compare(ps, vgc) {
	let line = 0;
	while (line < ps.log.length && line < vgc.log.length && ps.log[line] === vgc.log[line]) line++;
	const logDiffers = line < ps.log.length || line < vgc.log.length;
	let at = 0;
	const n = Math.min(ps.draws.length, vgc.draws.length);
	while (at < n && ps.draws[at].span === vgc.draws[at].span && ps.draws[at].raw === vgc.draws[at].raw) at++;
	const drawDiffers = at < ps.draws.length || at < vgc.draws.length;
	// Draws PS made before it logged the first differing line.
	const before = logDiffers ? ps.draws.filter(d => d.line <= line).length : ps.draws.length;
	// The trailing case: everything PS drew matched, the engine drew on after the log parted.
	const hidden = logDiffers && drawDiffers && at < before;
	const kind = !logDiffers ? (drawDiffers ? 'draws' : 'same') : hidden ? 'hidden' : 'visible';
	return { kind, line, logDiffers, ps_line: ps.log[line] ?? null, vgc_line: vgc.log[line] ?? null, at, drawDiffers, before };
}

const turnOf = (log, line) => {
	for (let i = line; i >= 0; i--) if (log[i] && log[i].startsWith('|turn|')) return +log[i].split('|')[2];
	return 0;
};
// The first frame past the Battle.random* wrappers, which name no mechanic.
const callerOf = (d) => (d.stack.find(f => !/^Battle\.(?:random|randomChance|sample)\b/.test(f)) || '?').split(' ')[0];
const fmtPs = (d) => (d ? `random(${d.args.join(',')})=${d.value} turn=${d.turn} move=${d.move} @ ${d.stack.slice(0, 4).join(' < ')}` : '(no draw)');
const fmtVgc = (d) => (d ? `span=${d.span} ${d.op} args=${d.args.join(',')} ctx=${d.decision}/mv${d.moveId}/${d.actor}->${d.target} @ ${d.site}` : '(no draw)');

function loadOne(file, game) {
	const text = fs.readFileSync(file, 'utf8');
	if (file.endsWith('.jsonl')) {
		const rows = text.split('\n').filter(Boolean).map(JSON.parse);
		const row = game === null ? rows[0] : rows.find(r => r.game === game);
		if (!row) throw new Error(`no game ${game} in ${file}`);
		return row.input_log;
	}
	return text.split('\n').filter(Boolean);
}

async function single(file, game, around, outFile) {
	const input = loadOne(file, game);
	const ps = runShowdown(input);
	const vgc = await runEngine(input);
	const c = compare(ps, vgc);
	console.log(`kind: ${c.kind}`);
	console.log(`draws: ps ${ps.draws.length}, engine ${vgc.draws.length}`);
	if (c.logDiffers) {
		console.log(`first log difference: line ${c.line}, turn ${turnOf(ps.log, c.line)}, after ${c.before} PS draws`);
		console.log(`  ps : ${c.ps_line}\n  vgc: ${c.vgc_line}`);
	} else console.log('logs identical');
	if (c.drawDiffers) {
		const p = ps.draws[c.at];
		console.log(`first draw difference: #${c.at}, turn ${p ? p.turn : '(past last PS draw)'}`);
		console.log(`  ps : ${fmtPs(p)}`);
		if (p) for (const f of p.stack) console.log(`       ${f}`);
		if (p && p.sort) console.log(`       sorting: ${p.sort.join('  ')}`);
		console.log(`  vgc: ${fmtVgc(vgc.draws[c.at])}`);
	} else console.log('draws identical');
	const center = c.drawDiffers ? c.at : Math.min(c.before, ps.draws.length);
	console.log(`aligned draws around #${center}:`);
	for (let i = Math.max(0, center - around); i <= center + around; i++) {
		if (!ps.draws[i] && !vgc.draws[i]) break;
		const mark = ps.draws[i] && vgc.draws[i] && ps.draws[i].span === vgc.draws[i].span ? ' ' : '*';
		console.log(`${mark}${String(i).padStart(5)} ps  ${fmtPs(ps.draws[i])}\n       vgc ${fmtVgc(vgc.draws[i])}`);
	}
	if (outFile) fs.writeFileSync(outFile, JSON.stringify({ compare: c, ps: ps.draws, vgc: vgc.draws }, null, 1));
}

async function batch(source, tag, outFile) {
	const games = [];
	if (source.endsWith('.jsonl')) {
		for (const l of fs.readFileSync(source, 'utf8').split('\n').filter(Boolean)) {
			const row = JSON.parse(l);
			games.push({ id: `${row.game}`, input: row.input_log });
		}
	} else {
		for (const f of fs.readdirSync(path.join(source, 'diff'))) {
			if (f.endsWith('.in')) games.push({ id: f.slice(0, -3), input: loadOne(path.join(source, 'diff', f), null) });
		}
	}
	const out = outFile ? fs.createWriteStream(outFile) : null;
	const kinds = {};
	const byCaller = {};
	let next = 0;
	const worker = async () => {
		while (next < games.length) {
			const g = games[next++];
			const ps = runShowdown(g.input);
			let vgc;
			try { vgc = await runEngine(g.input); } catch (e) { kinds.tool_error = (kinds.tool_error || 0) + 1; continue; }
			const c = compare(ps, vgc);
			const psTag = c.ps_line ? c.ps_line.split('|')[1] : '';
			if (tag && psTag !== tag.replace(/^\|/, '')) continue;
			kinds[c.kind] = (kinds[c.kind] || 0) + 1;
			const p = ps.draws[c.at];
			const v = vgc.draws[c.at];
			const key = c.kind === 'hidden' || c.kind === 'draws' ?
				`${c.kind} ${p ? `ps ${callerOf(p)}(${p.span})` : 'ps none'} | ${v ? `vgc ${v.op}(${v.span}) ${v.site}` : 'vgc none'}` :
				`${c.kind} ${psTag}`;
			(byCaller[key] = byCaller[key] || []).push(g.id);
			if (out) {
				out.write(`${JSON.stringify({
					id: g.id, ...c, turn: turnOf(ps.log, c.line),
					ps: p ? { args: p.args, turn: p.turn, move: p.move, stack: p.stack } : null,
					vgc: v ? { span: v.span, op: v.op, site: v.site, decision: v.decision, moveId: v.moveId } : null,
				})}\n`);
			}
		}
	};
	await Promise.all([worker(), worker()]);
	if (out) out.end();
	console.log(JSON.stringify(kinds));
	for (const [k, ids] of Object.entries(byCaller).sort((a, b) => b[1].length - a[1].length)) {
		console.log(`${String(ids.length).padStart(5)} ${k}   e.g. ${ids[0]}`);
	}
}

(async () => {
	const argv = process.argv.slice(2);
	const take = (flag, dflt) => {
		const i = argv.indexOf(flag);
		if (i < 0) return dflt;
		const [v] = argv.splice(i, 2).slice(1);
		return v;
	};
	const game = take('--game', null);
	const around = +take('--around', 5);
	const outFile = take('--out', null);
	const tag = take('--tag', null);
	const source = take('--batch', null);
	if (source) return batch(source, tag, outFile);
	if (!argv[0]) throw new Error('usage: see the top of tools/protocol/draw-trace.cjs');
	return single(argv[0], game === null ? null : +game, around, outFile);
})().catch((e) => { console.error(e); process.exitCode = 1; });
