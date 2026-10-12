#!/usr/bin/env node
// Phase-3 gate: PS's `simulate-battle` vs `vgc sim`, byte for byte.
//
// For each battle: a seeded random legal-choice policy plays PS in-process
// (BattleStream, as simulate-battle wraps it), reading PS's requests and
// sometimes sending invalid choices, undoing or re-choosing. The input it
// wrote is the battle's script: one write per player message. The script is
// then fed, write by write, to PS's real `pokemon-showdown simulate-battle`
// and to `vgc sim` (each a subprocess; the next write goes once the previous
// one's output is in), and the two stdouts are compared (|t:| values
// normalized: they are wall-clock seconds).
//
// usage: PS_DIST=<ps>/dist/sim node tools/protocol/sim-diff.js \
//          --jobs a.jsonl[,b.jsonl] --count 1000 --out <dir> --vgc target/release/vgc \
//          [--formats gen9championsvgc2026regmc,gen9vgc2025regi] [--concurrency 2] [--start 0]
// Writes <dir>/results.jsonl (one line per battle) and, for each battle that
// differs, <dir>/diff/<id>.{in,ps,vgc}.
'use strict';
const path = require('path');
const fs = require('fs');
const crypto = require('crypto');
const { spawn } = require('child_process');

const args = Object.fromEntries(process.argv.slice(2).reduce((acc, a, i, all) => {
	if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
	return acc;
}, []));
const PS_DIST = process.env.PS_DIST;
if (!PS_DIST) throw new Error('set PS_DIST');
const PS_ROOT = path.resolve(PS_DIST, '../..');
const { BattleStream, Teams, Dex } = require(path.join(PS_DIST, 'index.js'));

const COUNT = Number(args.count || 100);
const START = Number(args.start || 0);
const OUT = args.out || 'sim-diff-out';
const VGC = path.resolve(args.vgc || 'target/release/vgc');
const FORMATS = (args.formats || 'gen9championsvgc2026regmc,gen9vgc2025regi,gen9championsvgc2026regmb,gen9vgc2024regg').split(',');
const CONC = Number(args.concurrency || 2);
const MAX_TURNS = Number(args['max-turns'] || 60);
const ONLY = args.only ? new Set(String(args.only).split(',')) : null;
fs.mkdirSync(path.join(OUT, 'diff'), { recursive: true });

const jobs = String(args.jobs).split(',').flatMap(f => fs.readFileSync(f, 'utf8').split('\n').filter(Boolean).map(l => JSON.parse(l)));

function mulberry32(a) {
	return () => {
		a |= 0; a = a + 0x6D2B79F5 | 0;
		let t = Math.imul(a ^ a >>> 15, 1 | a);
		t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t;
		return ((t ^ t >>> 14) >>> 0) / 4294967296;
	};
}
const sha = s => crypto.createHash('sha256').update(s).digest('hex');

function teamFor(text, format) {
	const sets = Teams.import(text);
	const champions = Dex.formats.get(format).mod.startsWith('champions');
	for (const s of sets) {
		// Mega Evolution is a Champions mechanic; standard gen 9 teams carry no stones.
		if (!champions && Dex.items.get(s.item).megaStone) s.item = 'Leftovers';
	}
	return Teams.pack(sets);
}

// ---- the policy ------------------------------------------------------------
function targetsFor(target, i, n) {
	if (n < 2 || !target) return [null];
	const ally = -(2 - i); // the other slot, negative
	switch (target) {
	case 'normal': case 'adjacentFoe': return [1, 2];
	case 'any': return [1, 2, ally];
	case 'adjacentAlly': return [ally];
	case 'adjacentAllyOrSelf': return [ally, -(i + 1)];
	default: return [null];
	}
}

function pickTeam(req, rng) {
	const n = req.side.pokemon.length;
	const k = req.maxChosenTeamSize || n;
	const perm = [...Array(n).keys()].map(x => x + 1);
	for (let i = n - 1; i > 0; i--) { const j = Math.floor(rng() * (i + 1)); [perm[i], perm[j]] = [perm[j], perm[i]]; }
	return 'team ' + perm.slice(0, k).join(n >= 10 ? ',' : '');
}

function pickMove(req, rng) {
	const n = req.active.length;
	const mons = req.side.pokemon;
	const parts = [];
	const switched = new Set();
	let tera = false, mega = false;
	for (let i = 0; i < n; i++) {
		const a = req.active[i];
		const mon = mons[i];
		if (!a || !mon || mon.condition.endsWith(' fnt') || mon.commanding) { parts.push('pass'); continue; }
		const moves = [];
		a.moves.forEach((m, j) => {
			if (m.disabled) return;
			for (const t of targetsFor(m.target, i, n)) moves.push({ j: j + 1, t });
		});
		const switches = [];
		if (!a.trapped) {
			mons.forEach((m, k) => {
				if (k < n || m.condition.endsWith(' fnt') || switched.has(k)) return;
				switches.push(k + 1);
			});
		}
		const useMove = moves.length && (rng() < 0.85 || !switches.length);
		if (useMove) {
			const m = moves[Math.floor(rng() * moves.length)];
			let s = `move ${m.j}${m.t !== null ? ' ' + m.t : ''}`;
			if (a.canMegaEvo && !mega && rng() < 0.5) { s += ' mega'; mega = true; }
			else if (a.canTerastallize && !tera && rng() < 0.15) { s += ' terastallize'; tera = true; }
			parts.push(s);
		} else if (switches.length) {
			const k = switches[Math.floor(rng() * switches.length)];
			switched.add(k - 1);
			parts.push(`switch ${k}`);
		} else {
			parts.push('move 1');
		}
	}
	return parts.join(', ');
}

function pickSwitch(req, rng) {
	const n = req.forceSwitch.length;
	const mons = req.side.pokemon;
	const parts = [];
	const used = new Set();
	for (let i = 0; i < n; i++) {
		if (!req.forceSwitch[i]) { parts.push('pass'); continue; }
		const reviving = mons[i] && mons[i].reviving;
		const cands = [];
		mons.forEach((m, k) => {
			if (used.has(k)) return;
			if (reviving) { if (m.condition.endsWith(' fnt')) cands.push(k); return; }
			if (k < n || m.condition.endsWith(' fnt')) return;
			cands.push(k);
		});
		if (!cands.length) { parts.push('pass'); continue; }
		const k = cands[Math.floor(rng() * cands.length)];
		used.add(k);
		parts.push(`switch ${k + 1}`);
	}
	return parts.join(', ');
}

function pick(req, rng) {
	if (req.teamPreview) return pickTeam(req, rng);
	if (req.forceSwitch) return pickSwitch(req, rng);
	return pickMove(req, rng);
}

function invalid(req, rng) {
	const opts = ['move 9', 'switch 1', 'move 1 7', 'foo', 'pass, pass, pass', 'switch 99', 'team 0', 'move 1, move 1, move 1', 'move 1 terastallize, move 1 terastallize', 'shift', 'switch'];
	return opts[Math.floor(rng() * opts.length)];
}

// ---- script generation (PS in-process) ---------------------------------------
function frame(m) { return (m.endsWith('\n') ? m : m + '\n') + '\n'; }

function generate(k) {
	const job = jobs[(k * 7919) % jobs.length];
	const format = FORMATS[k % FORMATS.length];
	const rng = mulberry32(parseInt(sha(`policy-${k}`).slice(0, 8), 16));
	const seed = 'sodium,' + sha(`simdiff-${k}`).slice(0, 32);
	const names = [['Alice', 'Bob'], ['p1 player', 'Zoë'], ['Red', 'Blue']][k % 3];
	const s = new BattleStream({ noCatch: true });
	const script = [];
	const req = { p1: null, p2: null };
	const errs = { p1: 0, p2: 0 };
	let ended = false;
	let turn = 0;
	const send = (chunk) => {
		s.write(chunk);
		const out = s.buf.splice(0);
		script.push({ chunk, out: out.map(frame).join('') });
		for (const m of out) {
			const [type, ...rest] = m.split('\n');
			if (type === 'end') ended = true;
			if (type === 'update') for (const l of rest) if (l.startsWith('|turn|')) turn = Number(l.slice(6));
			if (type === 'sideupdate') {
				const side = rest[0];
				const line = rest.slice(1).join('\n');
				if (line.startsWith('|request|')) req[side] = JSON.parse(line.slice(9));
				if (line.startsWith('|error|')) errs[side]++;
			}
		}
		return out;
	};
	send(`>start ${JSON.stringify({ formatid: format, seed })}`);
	send(`>player p1 ${JSON.stringify({ name: names[0], team: teamFor(job.p1team, format) })}`);
	send(`>player p2 ${JSON.stringify({ name: names[1], team: teamFor(job.p2team, format) })}`);
	let guard = 0;
	while (!ended && guard++ < 2000) {
		const sides = ['p1', 'p2'].filter(x => req[x] && !req[x].wait);
		if (!sides.length) break;
		if (turn > MAX_TURNS) { send(rng() < 0.5 ? '>tiebreak' : '>forcetie'); break; }
		if (rng() < 0.01) { send(['>forcewin p1', '>forcelose p1', '>forcetie'][Math.floor(rng() * 3)]); break; }
		if (rng() < 0.02) send('>chat hello world');
		if (rng() < 0.5) sides.reverse();
		const snapshot = { ...req };
		for (const side of sides) {
			let r = snapshot[side];
			if (rng() < 0.06) {
				const before = errs[side];
				send(`>${side} ${invalid(r, rng)}`);
				if (errs[side] === before) continue; // it was accepted after all
				r = req[side] || r;
			}
			// Undo / re-choose only while the other side still has to answer
			// (an answer from the last side commits the turn).
			const first = sides.length === 2 && side === sides[0];
			if (first && rng() < 0.05) { send(`>${side} ${pick(r, rng)}`); send(`>${side} undo`); r = req[side] || r; }
			else if (first && rng() < 0.03) { send(`>${side} ${pick(r, rng)}`); }
			let ok = false;
			for (let tries = 0; tries < 6 && !ok; tries++) {
				const before = errs[side];
				send(`>${side} ${tries < 5 ? pick(r, rng) : 'default'}`);
				ok = errs[side] === before;
				if (!ok && req[side]?.update) r = req[side];
			}
		}
	}
	if (!ended) send('>forcetie');
	return { id: `b${String(k).padStart(5, '0')}`, k, format, seed, script };
}

// ---- subprocess replay ---------------------------------------------------------
const norm = s => s.replace(/\|t:\|\d+/g, '|t:|');

function replay(cmd, argv, cwd, script, strict) {
	return new Promise((resolve) => {
		const p = spawn(cmd, argv, { cwd, stdio: ['pipe', 'pipe', 'pipe'] });
		let out = '';
		let err = '';
		let lastData = Date.now();
		p.stdout.on('data', d => { out += d; lastData = Date.now(); });
		p.stderr.on('data', d => { err += d; });
		let exited = false;
		p.on('exit', () => { exited = true; });
		p.stdin.on('error', () => {});
		(async () => {
			let expect = 0;
			let expected = '';
			let diverged = false;
			for (const step of script) {
				if (exited) break;
				p.stdin.write(step.chunk + '\n');
				expect += Buffer.byteLength(step.out);
				expected += step.out;
				if (!step.out) continue;
				const t0 = Date.now();
				// Wait for this write's output: its byte count from the in-process run,
				// or, once the process' output went its own way, a quiet 100 ms.
				for (;;) {
					await new Promise(r => setTimeout(r, 1));
					const n = Buffer.byteLength(out);
					if (n >= expect) break;
					if (exited) break;
					if (!strict && !diverged && Date.now() - t0 > 200 && !norm(expected).startsWith(norm(out))) diverged = true;
					if (!strict && diverged && Date.now() - lastData > 100 && Date.now() - t0 > 100) break;
					if (Date.now() - t0 > 10000) break;
				}
				if (!diverged && norm(out) !== norm(expected)) diverged = true;
			}
			p.stdin.end();
			const t1 = Date.now();
			while (!exited && Date.now() - t1 < 3000) await new Promise(r => setTimeout(r, 2));
			if (!exited) p.kill();
			resolve({ out, err });
		})();
	});
}

// Messages: BattleTextStream frames each as `msg\n\n` (msg may hold blank lines).
function messages(text) {
	return text.split(/\n\n(?=(?:update|sideupdate|end|requesteddata)\n)/).map(m => m.replace(/\n+$/, ''));
}

function compare(ps, vg) {
	const a = messages(norm(ps));
	const b = messages(norm(vg));
	const res = { identical: norm(ps) === norm(vg) };
	if (res.identical) return res;
	let i = 0;
	while (i < a.length && i < b.length && a[i] === b[i]) i++;
	res.msg = i;
	const ma = a[i] || '';
	const mb = b[i] || '';
	res.kind = (ma || mb).split('\n')[0];
	const la = ma.split('\n');
	const lb = mb.split('\n');
	let j = 0;
	while (j < la.length && j < lb.length && la[j] === lb[j]) j++;
	res.ps = la[j] ?? null;
	res.vgc = lb[j] ?? null;
	res.ctx = la.slice(Math.max(0, j - 3), j);
	// State check: every |request| and the end record (minus the input log) equal?
	const reqs = ms => ms.filter(m => m.startsWith('sideupdate') && m.includes('|request|'));
	const endRec = ms => {
		const e = ms.find(m => m.startsWith('end\n'));
		if (!e) return null;
		const o = JSON.parse(e.slice(4));
		delete o.inputLog;
		return JSON.stringify(o);
	};
	const ra = reqs(a), rb = reqs(b);
	res.requests_equal = ra.length === rb.length && ra.every((m, k) => m === rb[k]);
	res.end_equal = endRec(a) === endRec(b);
	res.state_equal = res.requests_equal && res.end_equal;
	return res;
}

async function runOne(k) {
	const g = generate(k);
	if (ONLY && !ONLY.has(g.id)) return null;
	const inproc = g.script.map(s => s.out).join('');
	const ps = await replay('node', ['pokemon-showdown', 'simulate-battle', '--skip-build'], PS_ROOT, g.script, true);
	const vg = await replay(VGC, ['sim'], process.cwd(), g.script, false);
	const ps_ok = norm(ps.out) === norm(inproc);
	const c = compare(ps.out, vg.out);
	if (!c.identical) {
		// The write whose output first differs, and the inputs leading to it.
		let seen = 0;
		for (const [i, st] of g.script.entries()) {
			seen += st.out ? messages(st.out).length : 0;
			if (seen > c.msg) {
				c.write = i;
				c.input = g.script.slice(Math.max(0, i - 3), i + 1).map(s => s.chunk);
				break;
			}
		}
	}
	const rec = { id: g.id, format: g.format, seed: g.seed, writes: g.script.length, ps_matches_inprocess: ps_ok, ...c };
	if (vg.err) rec.vgc_stderr = vg.err.slice(0, 500);
	if (!c.identical || !ps_ok) {
		const d = path.join(OUT, 'diff', g.id);
		fs.writeFileSync(d + '.in', g.script.map(s => s.chunk).join('\n') + '\n');
		fs.writeFileSync(d + '.ps', ps.out);
		fs.writeFileSync(d + '.vgc', vg.out);
		if (!ps_ok) fs.writeFileSync(d + '.inproc', inproc);
	}
	return rec;
}

(async () => {
	const res = fs.createWriteStream(path.join(OUT, 'results.jsonl'), { flags: args.append ? 'a' : 'w' });
	let next = START;
	let done = 0, same = 0;
	const worker = async () => {
		while (next < START + COUNT) {
			const k = next++;
			const r = await runOne(k);
			if (!r) continue;
			res.write(JSON.stringify(r) + '\n');
			done++;
			if (r.identical) same++;
			if (done % 50 === 0) console.error(`${done} battles, ${same} identical`);
		}
	};
	await Promise.all(Array.from({ length: CONC }, worker));
	res.end();
	console.error(`done: ${done} battles, ${same} identical (${(100 * same / Math.max(1, done)).toFixed(1)}%)`);
})();
