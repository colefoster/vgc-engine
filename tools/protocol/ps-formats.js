#!/usr/bin/env node
// Dumps the battle-start facts the engine needs for every gen 9 PS format
// that takes player teams (no random teams), by starting one battle per
// format and reading what PS writes:
//   id, name (|tier|), gameType, the |rule| lines (rule-table order),
//   team preview (|teampreview| and its picked size), Open Team Sheets,
//   debug (exact HP), supportCancel (Cancel Mod) and the mod (champions).
// Output: crates/vgc-engine-core/src/ps_formats.json (sorted by id).
//
// usage: PS_DIST=<pokemon-showdown>/dist/sim node tools/protocol/ps-formats.js [out.json]
'use strict';
const path = require('path');
const fs = require('fs');
const PS = process.env.PS_DIST;
if (!PS) throw new Error('set PS_DIST to a built pokemon-showdown dist/sim');
const { Battle, Dex, Teams } = require(path.join(PS, 'index.js'));

const OUT = process.argv[2] || path.join(__dirname, '../../crates/vgc-engine-core/src/ps_formats.json');

const mon = (species, ability, moves) => ({ name: '', species, item: '', ability, moves, nature: 'Hardy', gender: 'M', evs: {}, ivs: {}, level: 50 });
const TEAM = Teams.pack([
	mon('Incineroar', 'Intimidate', ['fakeout', 'protect']),
	mon('Amoonguss', 'Regenerator', ['spore', 'protect']),
	mon('Rillaboom', 'Grassy Surge', ['fakeout', 'protect']),
	mon('Garchomp', 'Rough Skin', ['earthquake', 'protect']),
	mon('Gholdengo', 'Good as Gold', ['makeitrain', 'protect']),
	mon('Kingambit', 'Defiant', ['suckerpunch', 'protect']),
]);

const out = [];
for (const format of Dex.formats.all()) {
	if (format.effectType !== 'Format' || Dex.forFormat(format).gen !== 9) continue;
	if (!['singles', 'doubles'].includes(format.gameType || 'singles')) continue;
	if (format.team) continue; // random / generated teams
	// Mechanics the engine models: standard gen 9 (pre-DLC data included) and Champions.
	if (!['gen9', 'gen9predlc', 'champions', 'championsregmb'].includes(format.mod)) continue;
	let battle;
	const log = [];
	try {
		battle = new Battle({ formatid: format.id, seed: 'sodium,00000000000000000000000000000001', send: (t, d) => { if (t === 'update') log.push(...(Array.isArray(d) ? d : d.split('\n'))); } });
		battle.setPlayer('p1', { name: 'A', team: TEAM });
		battle.setPlayer('p2', { name: 'B', team: TEAM });
		battle.sendUpdates();
	} catch (e) {
		continue;
	}
	const lines = battle.log;
	const tier = lines.findIndex(l => l.startsWith('|tier|'));
	if (tier < 0) continue;
	const rules = [];
	let i = tier + 1;
	let clean = true;
	for (; i < lines.length; i++) {
		const l = lines[i];
		if (l.startsWith('|rule|')) rules.push(l.slice(6));
		else if (l === '|clearpoke' || l === '|start' || l === '|' || l.startsWith('|split|')) break;
		else { clean = false; break; }
	}
	const tp = lines.find(l => l === '|teampreview' || l.startsWith('|teampreview|'));
	out.push({
		id: format.id,
		name: format.name,
		gameType: format.gameType || 'singles',
		mod: format.mod,
		rules,
		teamPreview: !!tp,
		pickedTeamSize: tp && tp.startsWith('|teampreview|') ? Number(tp.slice(13)) : 0,
		openTeamSheets: lines.some(l => l.startsWith('|uhtml|otsrequest|')),
		debug: !!format.debug,
		supportCancel: !!battle.supportCancel,
		// other |...| lines between |tier| and team preview (onBegin extras): the
		// engine does not write them, so such formats keep the fallback preamble.
		clean,
	});
	battle.destroy();
}
out.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
fs.writeFileSync(OUT, JSON.stringify(out, null, '\t') + '\n');
console.error(`${out.length} formats -> ${OUT}`);
