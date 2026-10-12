#!/usr/bin/env node
// Dumps the dex names PS's `Teams.unpack` resolves packed names to
// (sim/teams.ts unpackName: `Dex.<table>.get(name).name`), plus each
// species' base species and ability slots ('0', '1', 'H', 'S').
// Output: crates/vgc-engine-core/src/ps_names.json.
//
// usage: PS_DIST=<pokemon-showdown>/dist/sim node tools/protocol/ps-names.js [out.json]
'use strict';
const path = require('path');
const fs = require('fs');
const PS = process.env.PS_DIST;
if (!PS) throw new Error('set PS_DIST to a built pokemon-showdown dist/sim');
const { Dex } = require(path.join(PS, 'index.js'));

const OUT = process.argv[2] || path.join(__dirname, '../../crates/vgc-engine-core/src/ps_names.json');
const sorted = obj => Object.fromEntries(Object.entries(obj).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));

const species = {};
for (const s of Dex.species.all()) {
	const ab = {};
	for (const k of ['0', '1', 'H', 'S']) if (s.abilities[k]) ab[k] = s.abilities[k];
	species[s.id] = [s.name, s.baseSpecies, ab];
}
const names = table => sorted(Object.fromEntries(table.all().map(x => [x.id, x.name])));
const out = {
	species: sorted(species),
	items: names(Dex.items),
	abilities: names(Dex.abilities),
	moves: sorted(Object.fromEntries(Dex.moves.all().map(m => [m.id, [m.name, m.type]]))),
	natures: names(Dex.natures),
};
fs.writeFileSync(OUT, JSON.stringify(out) + '\n');
console.error(`${Object.keys(species).length} species, ${Object.keys(out.moves).length} moves -> ${OUT}`);
