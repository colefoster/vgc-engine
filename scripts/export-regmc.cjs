// Usage: node scripts/export-regmc.cjs /absolute/path/to/pokemon-showdown
const fs=require('node:fs');
const {Dex}=require(process.argv[2]);
const d=Dex.mod('champions');
const species=d.species.all().filter(s=>!s.isNonstandard&&!s.battleOnly&&!s.isMega);
const learnsets={};
for(const mon of species){const moves=new Set();let s=mon;while(s?.exists){for(const id of Object.keys(d.species.getLearnsetData(s.id).learnset||{}))if(!d.moves.get(id).isNonstandard)moves.add(id);s=s.prevo?d.species.get(s.prevo):null;}learnsets[mon.id]=[...moves].sort();}
const out={source:'smogon/pokemon-showdown',revision:'aa6d5f0856d24679be8f5df167d1b528c2dcbd71',format:'gen9championsvgc2026regmc',species:species.map(s=>s.id).sort(),items:d.items.all().filter(i=>!i.isNonstandard).map(i=>i.id).sort(),moves:d.moves.all().filter(m=>!m.isNonstandard).map(m=>m.id).sort(),learnsets};
fs.writeFileSync(require('node:path').join(__dirname,'../data/mods/regmc.json'),JSON.stringify(out,null,2)+'\n');
