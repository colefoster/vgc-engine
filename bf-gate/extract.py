#!/usr/bin/env python3
"""Extract single-hit damage observations and same-bracket move-order pairs
from spectated PS logs (gen9championsvgc2026regmc).

    python3 extract.py data/10 data/11 ... data/ots > hits.jsonl

Emits JSONL records of kind "hit" and "order". State tracked per mon: species
forme, HP% (spectator %), status, boosts, damage-relevant volatiles, item and
ability knowledge (OTS sheet, or revealed anywhere in the log), plus field
state (weather, terrain, screens, Tailwind, Trick Room, Helping Hand).
"""
import json, os, re, sys
from pathlib import Path

DEX = Path.home() / "Dev/localdex/data"
MOVES = json.load(open(DEX / "moves.json"))
POKEDEX = json.load(open(DEX / "pokedex.json"))
# Current PS Champions-mod dex (mega abilities differ from the localdex dump).
PSDEX = json.load(open(Path(__file__).resolve().parent / "ps_champions_dex.json"))

def slug(s): return re.sub(r"[^a-z0-9]", "", s.lower())

# Moves whose damage depends on state the one-shot calc can't see.
EXCLUDE_MOVES = {slug(x) for x in """Eruption, Water Spout, Dragon Energy, Flail, Reversal, Rage Fist,
Last Respects, Gyro Ball, Electro Ball, Payback, Avalanche, Revenge, Bolt Beak, Fishious Rend,
Stomping Tantrum, Temper Flare, Retaliate, Fling, Beat Up, Present, Psywave, Super Fang, Ruination,
Nature's Madness, Endeavor, Final Gambit, Counter, Mirror Coat, Metal Burst, Comeuppance, Seismic Toss,
Night Shade, Future Sight, Doom Desire, Hard Press, Crush Grip, Wring Out, Assurance, Pursuit,
Echoed Voice, Fury Cutter, Round, Spit Up, Punishment, Stored Power, Power Trip, Rollout, Ice Ball,
Hex, Brine, Venoshock, Barb Barrage, Infernal Parade, Bitter Malice, Acrobatics, Facade, Knock Off,
Poltergeist, Weather Ball, Terrain Pulse, Rising Voltage, Misty Explosion, Grassy Glide, Explosion,
Self-Destruct, Beak Blast, Focus Punch, Shell Trap, Sky Drop, Fickle Beam, Upper Hand, Burning Jealousy,
Lash Out, Collision Course, Electro Drift, Tera Blast, Photon Geyser, Shell Side Arm, Pollen Puff,
Alluring Voice, Steel Roller, Ice Spinner, Psyblade, Hydro Steam, Solar Beam, Solar Blade""".replace("\n", " ").split(", ")}
# Excluded because the observable hinges on hidden per-hit state; Knock Off and
# Acrobatics/Facade/Hex/Weather Ball are left out too so an unknown item/status
# interaction doesn't masquerade as an engine miss. (Documented in README.)

SCREENS = ("Reflect", "Light Screen", "Aurora Veil")
DMG_VOLATILES = {"flashfire", "charge", "protosynthesisatk", "protosynthesisspa", "quarkdriveatk",
                 "quarkdrivespa", "protosynthesisdef", "protosynthesisspd", "quarkdrivedef", "quarkdrivespd",
                 "supremeoverlord", "focusenergy", "laserfocus", "powertrick", "minimize", "tarshot",
                 "glaiverush", "dragoncheer"}
BOOST_IDX = {"atk": 0, "def": 1, "spa": 2, "spd": 3, "spe": 4, "accuracy": 5, "evasion": 6}
WEATHER = {"RainDance": "rain", "SunnyDay": "sun", "Sandstorm": "sand", "Snowscape": "snow", "Snow": "snow",
           "Hail": "snow", "none": None, "PrimordialSea": "rain", "DesolateLand": "sun"}


def parse_hp(s):
    """'64/100 brn' -> (64, 'brn'); '0 fnt' -> (0, 'fnt')."""
    parts = s.strip().split()
    # Champions shows floor(100*hp/max) || 1, plus a y/r (at 20) or g/y (at 50)
    # colour suffix; keep the raw token ("50g") so eval.py can use it.
    tok = parts[0].split("/")[0]
    st = parts[1] if len(parts) > 1 else None
    return tok, st


def tags_of(parts):
    t = {}
    for p in parts:
        if p.startswith("["):
            m = re.match(r"\[(\w+)\]\s*(.*)", p)
            if m: t[m.group(1)] = m.group(2)
    return t


def pos_of(ident):  # 'p1a: Nick' -> ('p1', 'a', 'Nick')
    m = re.match(r"(p\d)([a-d]?): (.*)", ident)
    return (m.group(1), m.group(2), m.group(3)) if m else (None, None, None)


class Mon:
    def __init__(self, side, nick, species):
        self.side, self.nick, self.species = side, nick, species
        self.base_species = species
        self.hp = "100"
        self.status = None
        self.boosts = [0] * 7
        self.vol = set()
        self.boost_uncertain = False
        self.ots = None  # dict from showteam
        self.megaed = False
        self.mega_line = None
        self.transformed = False

    def key(self): return (self.side, self.nick)


def parse_showteam(s):
    out = {}
    for ent in s.split("]"):
        f = ent.split("|")
        if len(f) < 7: continue
        name, species, item, ability, moves, nature = f[0], f[1] or f[0], f[2], f[3], f[4], f[5]
        out[slug(species)] = {"item": item or None, "ability": ability or None, "nature": nature or None,
                              "moves": moves.split(",")}
    return out


def knowledge_pass(lines):
    """Pre-scan: for each (side,nick), item events and ability reveals by line index."""
    items, abil, tricked = {}, {}, set()
    for i, ln in enumerate(lines):
        p = ln.split("|")
        if len(p) < 3: continue
        cmd = p[1]
        tg = tags_of(p[3:])
        if cmd in ("-item", "-enditem") and p[2].startswith("p"):
            s, _, n = pos_of(p[2])
            frm = tg.get("from", "")
            if cmd == "-item" and ("Trick" in frm or "Switcheroo" in frm or "Bestow" in frm or "Thief" in frm
                                   or "Covet" in frm or "Pickpocket" in frm or "Magician" in frm or "Symbiosis" in frm):
                tricked.add((s, n))
            items.setdefault((s, n), []).append((i, cmd, p[3], frm))
        frm = tg.get("from", "")
        if frm.startswith("item:") and p[2].startswith("p"):
            it = frm[5:].strip()
            holder = p[2]
            if "of" in tg and it in ("Rocky Helmet", "Jaboca Berry", "Rowap Berry", "Sticky Barb"):
                holder = tg["of"]
            s, _, n = pos_of(holder)
            if s: items.setdefault((s, n), []).append((i, "from", it, ""))
        if cmd == "-ability" and p[2].startswith("p"):
            s, _, n = pos_of(p[2])
            if s:
                if "Trace" in frm or "Skill Swap" in frm or "Role Play" in frm or "Entrainment" in frm \
                        or "Doodle" in frm or "Mummy" in frm or "Lingering Aroma" in frm or "Wandering Spirit" in frm:
                    tricked.add(("abil",) + (s, n))
                abil.setdefault((s, n), []).append((i, p[3]))
        elif frm.startswith("ability:"):
            a = frm[8:].strip()
            # PS convention: "[from] ability: X|[of] Y" names Y as X's holder, except where
            # [of] is the other party (absorb heals/immunities, self-boosts, self-damage).
            holder = tg.get("of", p[2])
            if cmd in ("-heal", "-immune", "-boost", "-clearnegativeboost", "-block", "-fail",
                       "-curestatus", "-cureteam") or a in ("Solar Power", "Dry Skin"):
                holder = p[2]
            s, _, n = pos_of(holder) if holder.startswith("p") else (None, None, None)
            if s: abil.setdefault((s, n), []).append((i, a))
        if cmd == "-activate" and len(p) > 3 and p[3].startswith("ability:") and p[2].startswith("p"):
            s, _, n = pos_of(p[2])
            if s: abil.setdefault((s, n), []).append((i, p[3][8:].strip()))
    return items, abil, tricked


def item_at(mon, idx, items, tricked):
    """Return (item_or_None, known: bool)."""
    evs = items.get(mon.key(), [])
    before = [e for e in evs if e[0] < idx]
    for e in reversed(before):
        if e[1] == "-enditem": return None, True
        if e[1] == "-item": return e[2], True
        if e[1] == "from": return e[2], True
    if mon.ots is not None:
        return mon.ots["item"], True
    if mon.key() in tricked: return None, False
    after = [e for e in evs if e[0] >= idx]
    if after:
        e = after[0]
        if e[1] in ("-enditem", "from"): return e[2], True
        if e[1] == "-item" and "Frisk" in e[3]: return e[2], True
    return None, False


def ability_at(mon, idx, abil, tricked):
    evs = abil.get(mon.key(), [])
    if mon.megaed:
        after_mega = [e for e in evs if mon.mega_line is not None and mon.mega_line <= e[0] <= idx]
        if after_mega: return after_mega[-1][1], True
        a = (PSDEX["species"].get(slug(mon.species)) or {}).get("abilities", [None])[0]
        return a, True
    before = [e for e in evs if e[0] <= idx]
    if before: return before[-1][1], True
    if mon.ots is not None: return mon.ots["ability"], True
    if ("abil",) + mon.key() in tricked: return None, False
    if evs: return evs[0][1], True
    return None, False


def snapshot(mon, idx, items, tricked, abil):
    it, ik = item_at(mon, idx, items, tricked)
    ab, ak = ability_at(mon, idx, abil, tricked)
    return {"species": mon.species, "hp": mon.hp, "status": mon.status, "boosts": list(mon.boosts),
            "vol": sorted(mon.vol), "boost_uncertain": mon.boost_uncertain, "item": it, "item_known": ik,
            "ability": ab, "ability_known": ak, "nature": (mon.ots or {}).get("nature"),
            "ots": mon.ots is not None, "transformed": mon.transformed}


def process(game):
    log = game["log"]
    lines = log.split("\n")
    gid = game["id"]
    ots_sheets = {}
    for ln in lines:
        if ln.startswith("|showteam|"):
            p = ln.split("|", 3)
            ots_sheets[p[2]] = parse_showteam(p[3])
    if any(ln.startswith("|replace|") for ln in lines):
        return []  # Illusion: identities unreliable
    items, abil, tricked = knowledge_pass(lines)
    mons, active = {}, {}
    weather, terrain, trick_room = None, None, False
    side_cond = {"p1": set(), "p2": set()}
    helped = set()
    turn = 0
    out = []
    cur = None  # current move context
    rating = game.get("rating")

    def get_mon(ident, details=None):
        s, slot, n = pos_of(ident)
        k = (s, n)
        if k not in mons:
            sp = details.split(",")[0] if details else n
            m = Mon(s, n, sp)
            sheet = ots_sheets.get(s)
            if sheet:
                m.ots = sheet.get(slug(sp)) or sheet.get(slug(sp.split("-")[0]))
            mons[k] = m
        return mons[k]

    def mon_at(ident):
        s, slot, n = pos_of(ident)
        if slot: return active.get(s + slot)
        return mons.get((s, n))

    for i, ln in enumerate(lines):
        p = ln.split("|")
        if len(p) < 2: continue
        cmd = p[1]
        tg = tags_of(p[2:])
        if cmd == "turn":
            turn = int(p[2]); helped.clear(); cur = None
            continue
        if cmd == "" or cmd == "upkeep":
            cur = None
            continue
        if cmd in ("switch", "drag"):
            s, slot, n = pos_of(p[2])
            old = active.get(s + slot)
            if old is not None:
                old.boosts = [0] * 7; old.vol = set(); old.boost_uncertain = False; old.transformed = False
            m = get_mon(p[2], p[3])
            hp, st = parse_hp(p[4])
            m.hp, m.status = hp, (st if st != "fnt" else None)
            active[s + slot] = m
            cur = None
            continue
        if cmd == "detailschange" or cmd == "-formechange":
            m = mon_at(p[2])
            if m:
                m.species = p[3].split(",")[0]
                if "-Mega" in m.species and not m.megaed: m.megaed = True; m.mega_line = i
            continue
        if cmd == "-transform":
            m = mon_at(p[2])
            if m: m.transformed = True
            continue
        if cmd == "faint":
            m = mon_at(p[2])
            if m: m.hp = "0"
            continue
        if cmd == "-weather":
            if "upkeep" not in tg: weather = WEATHER.get(p[2], None)
            continue
        if cmd == "-fieldstart":
            f = p[2]
            if "Trick Room" in f: trick_room = True
            for t in ("Electric", "Grassy", "Psychic", "Misty"):
                if t + " Terrain" in f: terrain = t.lower()
            continue
        if cmd == "-fieldend":
            f = p[2]
            if "Trick Room" in f: trick_room = False
            if "Terrain" in f: terrain = None
            continue
        if cmd in ("-sidestart", "-sideend"):
            s = p[2][:2]
            c = p[3].replace("move: ", "")
            (side_cond[s].add if cmd == "-sidestart" else side_cond[s].discard)(c)
            continue
        if cmd == "-singleturn" and "Helping Hand" in p[3]:
            m = mon_at(p[2])
            if m: helped.add(m.key())
            continue
        if cmd in ("-boost", "-unboost"):
            m = mon_at(p[2])
            if m and p[3] in BOOST_IDX:
                d = int(p[4]) * (1 if cmd == "-boost" else -1)
                j = BOOST_IDX[p[3]]
                m.boosts[j] = max(-6, min(6, m.boosts[j] + d))
            continue
        if cmd == "-setboost":
            m = mon_at(p[2])
            if m and p[3] in BOOST_IDX: m.boosts[BOOST_IDX[p[3]]] = int(p[4])
            continue
        if cmd in ("-clearboost",):
            m = mon_at(p[2]);
            if m: m.boosts = [0] * 7
            continue
        if cmd == "-clearallboost":
            for m in active.values():
                if m: m.boosts = [0] * 7
            continue
        if cmd == "-clearnegativeboost":
            m = mon_at(p[2])
            if m: m.boosts = [max(0, b) for b in m.boosts]
            continue
        if cmd in ("-copyboost", "-swapboost", "-invertboost", "-clearpositiveboost"):
            for x in p[2:4]:
                if x.startswith("p"):
                    m = mon_at(x)
                    if m: m.boost_uncertain = True
            continue
        if cmd == "-status":
            m = mon_at(p[2])
            if m: m.status = p[3]
            continue
        if cmd == "-curestatus":
            m = mon_at(p[2]) if p[2].startswith("p") else None
            if m: m.status = None
            continue
        if cmd == "-start":
            m = mon_at(p[2])
            if m:
                v = slug(p[3].replace("ability: ", "").replace("move: ", ""))
                m.vol.add(v)
            continue
        if cmd == "-end":
            m = mon_at(p[2])
            if m:
                v = slug(p[3].replace("ability: ", "").replace("move: ", ""))
                m.vol.discard(v)
            continue
        if cmd in ("-heal", "-sethp"):
            m = mon_at(p[2])
            if m:
                m.hp, st = parse_hp(p[3])
            continue
        if cmd == "move":
            atk = mon_at(p[2])
            mv = p[3]
            frm = tg.get("from", "")
            cur = None
            if atk is None: continue
            s_atk = snapshot(atk, i, items, tricked, abil)
            # Speed-order record (snapshot of all actives before this action).
            out.append({"kind": "order", "game": gid, "turn": turn, "line": i, "move": mv, "from": frm,
                        "actor": atk.side + ":" + atk.nick,
                        "actives": {k: (dict(snapshot(m, i, items, tricked, abil), key=m.side + ":" + m.nick) if m else None)
                                    for k, m in active.items()},
                        "tailwind": {s: ("Tailwind" in side_cond[s]) for s in side_cond},
                        "trick_room": trick_room, "weather": weather, "terrain": terrain,
                        "ots": bool(ots_sheets)})
            cur = {"atk": atk, "atk_snap": s_atk, "move": mv, "spread": "spread" in tg, "from": frm,
                   "line": i, "targets_hit": {}, "crit": set(), "eff": {}, "hitcount": False,
                   "pending": [], "berry": {}, "helped": atk.key() in helped}
            continue
        if cur is None:
            continue
        # Inside a move block.
        if cmd == "-crit":
            cur["crit"].add(p[2][:3] + p[2][3:]); continue
        if cmd in ("-supereffective", "-resisted"):
            cur["eff"][p[2]] = cmd; continue
        if cmd == "-hitcount":
            cur["hitcount"] = True
            for h in cur["pending"]: h["multihit"] = True
            continue
        if cmd == "-enditem" and ("[weaken]" in ln or "[eat]" in ln):
            d = mon_at(p[2])
            if d and "[weaken]" in ln: cur["berry"][d.key()] = p[3]
            if d and p[3] in ("Focus Sash",):
                cur.setdefault("sash", set()).add(d.key())
                for h in cur["pending"]:
                    if h["def_key"] == d.side + ":" + d.nick: h["censored"] = "sash"
            continue
        if cmd == "-enditem" and "Focus Sash" in ln:
            d = mon_at(p[2])
            if d: cur.setdefault("sash", set()).add(d.key())
            for h in cur["pending"]:
                if d and h["def_key"] == d.side + ":" + d.nick: h["censored"] = "sash"
            continue
        if cmd == "-activate" and any(x in ln for x in ("Sturdy", "Endure", "Disguise", "Ice Face", "Focus Band")):
            d = mon_at(p[2])
            if d: cur.setdefault("sash", set()).add(d.key())
            for h in cur["pending"]:
                if d and h["def_key"] == d.side + ":" + d.nick: h["censored"] = "endure"
            continue
        if cmd == "-damage" and "from" not in tg:
            d = mon_at(p[2])
            if d is None: continue
            new, st = parse_hp(p[3])
            prev = d.hp
            dk = d.side + ":" + d.nick
            if dk in cur["targets_hit"]:
                cur["targets_hit"][dk]["multihit"] = True
                d.hp = new
                continue
            s_def = snapshot(d, i, items, tricked, abil)
            berry = cur["berry"].get(d.key())
            if berry: s_def["item"], s_def["item_known"] = berry, True
            s_atk_side, d_side = cur["atk"].side, d.side
            ally_atk = [m.species for k, m in active.items() if m and k[:2] == s_atk_side and m is not cur["atk"] and m.hp != "0"]
            ally_def = [m.species for k, m in active.items() if m and k[:2] == d_side and m is not d and m.hp != "0"]
            h = {"kind": "hit", "game": gid, "turn": turn, "line": i, "move": cur["move"], "from": cur["from"],
                 "atk": cur["atk_snap"], "def": s_def, "def_key": dk, "prev": prev, "new": new,
                 "crit": p[2] in cur["crit"], "spread": cur["spread"], "weather": weather, "terrain": terrain,
                 "screens": sorted(c for c in side_cond[d_side] if c in SCREENS),
                 "helped": cur["helped"], "ally_atk": ally_atk, "ally_def": ally_def,
                 "friendly": s_atk_side == d_side, "eff": cur["eff"].get(p[2]),
                 "multihit": cur["hitcount"], "censored": ("sash" if d.key() in cur.get("sash", ()) else None), "ots": bool(ots_sheets), "rating": rating}
            cur["targets_hit"][dk] = h
            cur["pending"].append(h)
            out.append(h)
            d.hp = new
            if st and st != "fnt": d.status = st
            continue
        if cmd == "-damage":
            d = mon_at(p[2])
            if d: d.hp, _ = parse_hp(p[3])
            continue
    return out


def main():
    roots = sys.argv[1:]
    seen = set()
    w = sys.stdout
    for r in roots:
        for f in sorted(Path(r).rglob("*.json")):
            g = json.load(open(f))
            if g["id"] in seen: continue
            seen.add(g["id"])
            try:
                recs = process(g)
            except Exception as e:  # noqa
                print(json.dumps({"kind": "error", "game": g["id"], "err": repr(e)}), file=w)
                continue
            for rec in recs:
                w.write(json.dumps(rec) + "\n")


if __name__ == "__main__":
    main()
