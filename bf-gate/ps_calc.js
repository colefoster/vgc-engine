// PS oracle for the same JSONL queries the Rust runner takes: 16 rolls via
// battle.actions.getDamage with the damage roll pinned. Usage: node ps_calc.js < q.jsonl
const PS = process.env.PS_DIR || '/Users/cole/Dev/metagame-lab/node_modules/pokemon-showdown';
const {Battle, Dex} = require(PS + '/dist/sim');
const readline = require('readline');
const dex = Dex.mod('champions');
const W = {rain: 'raindance', sun: 'sunnyday', sand: 'sandstorm', snow: 'snowscape'};
const T = {electric: 'electricterrain', grassy: 'grassyterrain', psychic: 'psychicterrain', misty: 'mistyterrain'};
const BI = ['atk', 'def', 'spa', 'spd', 'spe', 'accuracy', 'evasion'];
const SI = ['hp', 'atk', 'def', 'spa', 'spd', 'spe'];
function set(m, move) {
  const sp = dex.species.get(m.species);
  const evs = {}; SI.forEach((s, i) => evs[s] = (m.sp || [0,0,0,0,0,0])[i]);
  return {species: sp.name, name: sp.baseSpecies, item: m.item || '', ability: m.ability || sp.abilities[0],
    nature: m.nature || 'serious', evs, ivs: {hp:31,atk:31,def:31,spa:31,spd:31,spe:31}, level: 50, moves: [move], gender: ''};
}
function run(q) {
  // Optional ally abilities (Friend Guard, Power Spot, Battery, Steely Spirit)
  // ride on the Shuckle filler in each side's second slot.
  const filler = ab => ({species: 'Shuckle', name: 'Shuckle', ability: ab || 'Contrary', moves: ['splash'], nature: 'serious', level: 50, evs: {}, item: ''});
  const b = new Battle({formatid: 'gen9championsdoublescustomgame', seed: [1, 2, 3, 4]});
  b.setPlayer('p1', {team: [set(q.atk, q.move), filler(q.atk_ally_ability)]});
  b.setPlayer('p2', {team: [set(q.def, 'splash'), filler(q.def_ally_ability)]});
  if (b.requestState === 'teampreview') b.makeChoices('team 12', 'team 12');
  const a = b.p1.active[0], d = b.p2.active[0];
  const reset = () => { for (const [m, mon] of [[q.atk, a], [q.def, d]]) {
    for (const k of BI) mon.boosts[k] = 0;
    (m.boosts || []).forEach((v, i) => mon.boosts[BI[i]] = v);
    mon.status = m.status || ''; mon.statusState = {id: mon.status};
    mon.hp = m.hp_pct ? Math.max(1, Math.ceil(mon.maxhp * m.hp_pct / 100)) : mon.maxhp;
    // Start-of-battle effects (seeds, berries eaten on an earlier roll) must not leak.
    mon.item = dex.toID(m.item || ''); mon.itemState = {id: mon.item, target: mon};
    mon.volatiles = {};
  } };
  reset();
  for (const s of [b.p1, b.p2]) for (const c of Object.keys(s.sideConditions)) s.removeSideCondition(c);
  for (const c of q.screens || []) b.p2.addSideCondition(c, d);
  b.field.weather = W[q.weather] || ''; b.field.weatherState = {id: b.field.weather, duration: 5};
  b.field.terrain = T[q.terrain] || ''; b.field.terrainState = {id: b.field.terrain, duration: 5};
  b.field.pseudoWeather = {};
  const rolls = [];
  for (let k = 15; k >= 0; k--) {
    reset();
    if (q.helping_hand) a.addVolatile('helpinghand');
    let move = b.dex.getActiveMove(q.move);
    b.activeMove = move; b.activePokemon = a; b.activeTarget = d;
    // Mirror useMoveInner's type/move modifiers (-ate abilities, Weather Ball, etc).
    b.singleEvent('ModifyType', move, null, a, d, move, move);
    b.singleEvent('ModifyMove', move, null, a, d, move, move);
    move = b.runEvent('ModifyType', a, d, move, move);
    move = b.runEvent('ModifyMove', a, d, move, move);
    move.willCrit = !!q.crit;
    if (q.spread) move.spreadHit = true;
    b.activeMove = move; b.activePokemon = a; b.activeTarget = d;
    const orig = b.random.bind(b);
    b.random = (x, y) => (x === 16 && y === undefined) ? k : orig(x, y);
    b.randomChance = () => false;
    let dmg = b.actions.getDamage(a, d, move, true);
    b.random = orig;
    rolls.push(typeof dmg === 'number' ? dmg : 0);
  }
  return {hp: d.maxhp, rolls};
}
const rl = readline.createInterface({input: process.stdin});
const out = [];
rl.on('line', l => { try { out.push(JSON.stringify(run(JSON.parse(l)))); } catch (e) { out.push(JSON.stringify({err: String(e).slice(0, 120)})); } });
rl.on('close', () => process.stdout.write(out.join('\n') + '\n'));
