'use strict';
// A stand-in for pokemon-showdown's `dist/sim/index.js` whose BattleStream is
// vgc-engine: each stream spawns `vgc sim` (PS's simulate-battle protocol) and
// speaks the text protocol to it. Everything else (Teams, TeamValidator, Dex,
// getPlayerStreams, ...) comes from a real PS checkout, so a harness written
// against PS's in-process BattleStream runs unchanged:
//
//   PS_DIR=tools/protocol/ps-shim PS_REAL_DIR=<pokemon-showdown> VGC_SIM="target/release/vgc sim" node arena.mjs ...
//
// docs/protocol.md, "Use as a local Showdown replacement".
const path = require('path');
const { spawn } = require('child_process');

const REAL = process.env.PS_REAL_DIR;
if (!REAL) throw new Error('ps-shim: set PS_REAL_DIR to a built pokemon-showdown checkout (for Teams / Dex / getPlayerStreams)');
const real = require(path.join(path.resolve(REAL), 'dist/sim/index.js'));
const { Streams } = real;

const CMD = (process.env.VGC_SIM || 'vgc sim').split(' ').filter(Boolean);
const TYPES = /\n\n(?=(?:update|sideupdate|end|requesteddata)\n)/;

class BattleStream extends Streams.ObjectReadWriteStream {
	constructor(options = {}) {
		super();
		const args = CMD.slice(1);
		if (options.replay === 'spectator') args.push('--spectate');
		else if (options.replay) args.push('--replay');
		this.child = spawn(CMD[0], args, { stdio: ['pipe', 'pipe', 'inherit'] });
		this.out = '';
		this.child.stdout.setEncoding('utf8');
		this.child.stdout.on('data', d => {
			this.out += d;
			// BattleTextStream frames each message as `msg\n` + `\n`; a message
			// may hold blank lines itself (an empty |split| half), so split only
			// where the next message's type starts.
			const parts = this.out.split(TYPES);
			this.out = parts.pop();
			for (const p of parts) this.push(restore(p));
			// `vgc sim` flushes once per input chunk, so a read ending on the
			// `\n\n` frame ends the last message too.
			if (this.out.endsWith('\n\n')) {
				const last = this.out;
				this.out = '';
				this.push(restore(last.slice(0, -2)));
				// The `end` message closes PS's stream (unless keepAlive).
				if (last.startsWith('end\n') && !options.keepAlive) this.finish();
			}
		});
		this.child.stdout.on('end', () => {
			if (this.out.trim()) this.push(restore(this.out.replace(/\n\n$/, '')));
			this.out = '';
			this.finish();
		});
	}
	finish() {
		if (this.finished) return;
		this.finished = true;
		if (!this.atEOF) this.pushEnd();
		this.child.stdin.end();
	}
	_write(chunk) {
		// One write is one chunk for the simulator, as for PS's BattleStream.
		this.child.stdin.write(chunk.endsWith('\n') ? chunk : chunk + '\n');
	}
	_writeEnd() {
		this.child.stdin.end();
	}
	_destroy() {
		this.child.kill();
	}
}

/** Put back the trailing empty `|split|` half the `\n\n` framing ate. */
function restore(msg) {
	const lines = msg.split('\n');
	const i = lines.lastIndexOf(lines.slice().reverse().find(l => /^\|split\|p[1-4]$/.test(l)));
	if (i >= 0 && lines.length - i === 2) return msg + '\n';
	return msg;
}

module.exports = { ...real, BattleStream };
