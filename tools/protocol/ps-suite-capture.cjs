// Mocha preload for ps-suite.js: records each battle Showdown's test suite creates. Run from a
// Showdown checkout with CORPUS_OUT set.
'use strict';
const fs = require('fs');
const path = require('path');

const { Battle } = require(path.join(process.cwd(), 'dist/sim/battle.js'));
const stream = fs.createWriteStream(process.env.CORPUS_OUT);
const seen = new WeakSet();
let title = '';

const destroy = Battle.prototype.destroy;
Battle.prototype.destroy = function (...args) {
	if (!seen.has(this) && this.inputLog && this.inputLog.length >= 3) {
		seen.add(this);
		stream.write(JSON.stringify({
			test: title, format: this.format && this.format.id, gen: this.gen, gameType: this.gameType,
			input_log: this.inputLog, log: this.log,
		}) + '\n');
	}
	return destroy.apply(this, args);
};

exports.mochaHooks = {
	beforeEach() {
		title = this.currentTest.fullTitle();
	},
	afterAll(done) {
		stream.end(done);
	},
};
