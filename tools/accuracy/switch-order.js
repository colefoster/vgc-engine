// Switch-target remapping for ps-battle.js, kept free of PS imports so it can
// be tested without a built simulator.
//
// PS numbers `switch N` by the side's CURRENT `side.pokemon` order, which it
// permutes on team preview and on every switch; the engine keeps the original
// team order and an active-slot map. Names can't identify a member: a
// reconstructed team may hold Palafin and Palafin-Hero, two identical
// nicknames, or a mon whose species changed (formes, Transform). PS keeps one
// Pokemon object per member for the whole battle, so the object is the
// identity.

'use strict';

// The side's members in original team order. Take it right after `>player`,
// before team preview can reorder `side.pokemon`.
function captureRoster(side) {
  if (!side || !Array.isArray(side.pokemon) || !side.pokemon.length) {
    throw new Error('cannot capture roster: side has no pokemon yet');
  }
  return side.pokemon.slice();
}

// Rewrite each `switch N` in `cmd` (an answer to `req`) to the member's
// 1-based index in `original`. `current` is the live `side.pokemon`, which
// must still be in the order `req` was built from (the side hasn't answered
// yet, so PS hasn't moved); the request's ident/details are checked against
// it so a stale order throws instead of mapping silently wrong.
function toEngineCmd(cmd, req, current, original) {
  return cmd.split(', ').map((part) => {
    const m = /^switch (\d+)$/.exec(part);
    if (!m) return part;
    const n = parseInt(m[1], 10);
    const p = current[n - 1];
    const r = req.side.pokemon[n - 1];
    if (!p || !r) throw new Error(`cannot map ${part}: no member ${n}`);
    if (r.ident !== p.fullname || r.details !== p.details) {
      throw new Error(`cannot map ${part}: request shows ${r.ident} (${r.details}), side has ${p.fullname} (${p.details})`);
    }
    const idx = original.indexOf(p);
    if (idx < 0) throw new Error(`cannot map ${part} to the original team`);
    return `switch ${idx + 1}`;
  }).join(', ');
}

module.exports = { captureRoster, toEngineCmd };
