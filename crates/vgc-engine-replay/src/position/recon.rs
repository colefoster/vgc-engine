//! [`reconstruct`]: the log up to the stop -> a battle, a prefix and a report.

use super::build::{board_mon, field_elapsed, mon_report, request_mon, resolve_set, stats_for, SetPick};
use super::*;

fn format_from_tier(tier: &str) -> Option<String> {
    let id = slug(tier.trim());
    vgc_engine_core::ps_formats::lookup(&id).map(|f| f.id.to_string()).or_else(|| {
        [
            "gen9championsvgc2026regmc",
            "gen9championsvgc2026regmb",
            "gen9championsvgc2026regma",
            "gen9vgc2025regi",
            "gen9vgc2024regg",
            "gen9vgc2025regj",
            "gen9vgc2024regh",
            "gen9championsbssregmc",
        ]
        .into_iter()
        .find(|f| vgc_engine_core::ps_formats::lookup(f).is_some_and(|x| slug(x.name) == id))
        .map(str::to_string)
    })
}

/// Rebuild the battle at the stop. `lines` is the log (any [`InputKind`]).
pub fn reconstruct(lines: &[String], opts: &ReconOptions, prior: &Prior) -> Result<Reconstruction, ReconError> {
    let kind = opts.kind.unwrap_or_else(|| detect_kind(lines));
    if let (InputKind::PerSide(own), Some(v)) = (kind, opts.viewer) {
        if own != v {
            return Err(err("viewer", format!("a {} stream cannot be viewed as {}", side_key(own), side_key(v))));
        }
    }
    let cut = cut_index(lines, opts.turn, opts.stop)?;
    let head = &lines[..cut];
    let tier = head.iter().find_map(|l| l.strip_prefix("|tier|")).map(str::to_string);
    let fid = match &opts.format {
        Some(f) => f.to_ascii_lowercase(),
        None => tier.as_deref().and_then(format_from_tier).unwrap_or_else(|| "gen9championsvgc2026regmc".into()),
    };
    let fmt = vgc_engine_core::ps_formats::lookup(&fid).ok_or_else(|| err("format", format!("unknown format {fid}")))?;
    let prior = prior.for_format(&fid);
    let champions = fmt.champions();
    let n_active = if fmt.doubles { 2 } else { 1 };

    // The lines the state is read from: the viewer's channel of a raw log
    // (the omniscient one without a viewer), else the input as is.
    let (view, exact_side) = channel_view(head, kind, opts.viewer);
    let mut tr = Tracker::new(champions);
    tr.feed(&view, exact_side);
    let mut requests = opts.requests.clone();
    if let InputKind::PerSide(own) = kind {
        if requests[own as usize].is_none() {
            requests[own as usize] = embedded_request(lines, cut);
        }
    }
    let know = |s: usize| opts.viewer.is_none_or(|v| v as usize == s);
    // A request names the real actives: an Illusion the log has not
    // revealed yet is one.
    for side in 0..2 {
        let Some(r) = requests[side].as_ref().filter(|_| know(side)) else { continue };
        let list: Vec<Value> = r.pointer("/side/pokemon").and_then(Value::as_array).cloned().unwrap_or_default();
        for k in 0..n_active {
            let Some(rm) = list.get(k).filter(|x| x.get("active").and_then(Value::as_bool) == Some(true)) else { continue };
            let name = rm.get("ident").and_then(Value::as_str).and_then(|x| x.split_once(": ")).map(|x| x.1.to_string()).unwrap_or_default();
            let det = rm.get("details").and_then(Value::as_str).unwrap_or("").to_string();
            let Some(cur) = tr.slots[side][k] else { continue };
            let c = &tr.mons[side][cur];
            if c.name.as_deref() == Some(name.as_str()) && same_family(&c.preview, &parse_details(&det).0) {
                continue;
            }
            let real = tr.illusion_target(side, &name, &det, usize::MAX);
            tr.illusion_swap(side, k, real);
        }
    }
    let later = if opts.lookahead {
        let (full_view, ex) = channel_view(lines, kind, opts.viewer);
        let mut t2 = Tracker::new(champions);
        t2.feed(&full_view, ex);
        Some(t2)
    } else {
        None
    };

    // Mechanics the reconstruction does not carry, live at the stop.
    let mut unsupported: Vec<String> = tr.unsupported.clone();
    for side in 0..2 {
        for m in &tr.mons[side] {
            if m.active.is_some() && !m.fainted {
                unsupported.extend(m.unsupported.iter().cloned());
            }
        }
    }
    for fx in &tr.pseudo {
        if !matches!(fx.name.as_str(), "Trick Room" | "Gravity" | "Magic Room" | "Wonder Room") {
            unsupported.push(format!("field {}", fx.name));
        }
    }
    if let Some(w) = &tr.weather {
        if !matches!(w.name.as_str(), "SunnyDay" | "RainDance" | "Sandstorm" | "Snowscape" | "Snow") {
            unsupported.push(format!("weather {}", w.name));
        }
    }
    for side in 0..2 {
        if n_active == 2 && tr.slots[side][0].is_some() && tr.slots[side][0] == tr.slots[side][1] {
            unsupported.push("illusion (ambiguous)".into());
        }
    }
    if let Some(u) = unsupported.first() {
        return Err(err(&format!("unsupported: {u}"), unsupported.join(", ")));
    }

    let mut team_orders: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    let mut picks: [Vec<SetPick>; 2] = [Vec::new(), Vec::new()];
    let mut team_src = [Src::Exact; 2];
    let mut order_src = [Src::Exact; 2];
    let mut brought_src: [Vec<(Src, Option<f64>)>; 2] = [Vec::new(), Vec::new()];

    for side in 0..2 {
        let req = if know(side) { requests[side].as_ref() } else { None };
        let user_sets = if know(side) { opts.sets[side].as_ref() } else { None };
        let size = tr.teamsize[side].unwrap_or(if fmt.picked_team_size > 0 { fmt.picked_team_size as usize } else { 6 }).min(6);
        // Which mons were brought.
        let mut brought: Vec<usize> = Vec::new();
        let mut bsrc: Vec<(Src, Option<f64>)> = Vec::new();
        if let Some(r) = req {
            for rm in r.pointer("/side/pokemon").and_then(Value::as_array).into_iter().flatten() {
                let (sp, lvl, g) = parse_details(rm.get("details").and_then(Value::as_str).unwrap_or(""));
                let ident_name = rm.get("ident").and_then(Value::as_str).and_then(|x| x.split_once(": ")).map(|x| x.1.to_string());
                let is_named = |m: &Mon| m.name.is_some() && m.name == ident_name;
                let named = (0..tr.mons[side].len())
                    .find(|&i| !brought.contains(&i) && is_named(&tr.mons[side][i]) && [&tr.mons[side][i].species, &tr.mons[side][i].set_species, &tr.mons[side][i].preview].iter().any(|x| slug(x) == slug(&sp)))
                    .or_else(|| (0..tr.mons[side].len()).find(|&i| !brought.contains(&i) && is_named(&tr.mons[side][i]) && same_family(&tr.mons[side][i].preview, &sp)));
                let i = match named {
                    Some(i) => i,
                    None => match (0..tr.mons[side].len()).find(|&i| !brought.contains(&i) && tr.mons[side][i].name.is_none() && same_family(&tr.mons[side][i].preview, &sp)) {
                        Some(i) => i,
                        None => {
                            tr.mons[side].push(Mon { preview: sp.clone(), species: sp.clone(), set_species: sp.clone(), level: lvl, gender: g, ..Default::default() });
                            tr.mons[side].len() - 1
                        }
                    },
                };
                if tr.mons[side][i].name.is_none() {
                    tr.mons[side][i].name = ident_name;
                }
                if !brought.contains(&i) {
                    brought.push(i);
                    bsrc.push((Src::Exact, None));
                }
            }
        }
        if brought.is_empty() {
            for (i, m) in tr.mons[side].iter().enumerate() {
                if m.seen {
                    brought.push(i);
                    bsrc.push((Src::Exact, None));
                }
            }
            let mut rest: Vec<usize> = (0..tr.mons[side].len()).filter(|i| !brought.contains(i)).collect();
            if let Some(us) = user_sets.filter(|u| u.len() == size) {
                for i in rest.clone() {
                    if brought.len() >= size {
                        break;
                    }
                    if us.iter().any(|m| same_family(&m.species, &tr.mons[side][i].preview)) {
                        brought.push(i);
                        bsrc.push((Src::Exact, None));
                        rest.retain(|&x| x != i);
                    }
                }
            }
            if let Some(t2) = &later {
                for i in rest.clone() {
                    if brought.len() >= size {
                        break;
                    }
                    let pv = tr.mons[side][i].preview.clone();
                    if t2.mons[side].iter().any(|m| m.seen && slug(&m.preview) == slug(&pv)) {
                        brought.push(i);
                        bsrc.push((Src::Exact, None));
                        rest.retain(|&x| x != i);
                    }
                }
            }
            // The rest: the most used of the unseen preview mons.
            let usage = |i: usize| -> f64 { prior.keys_for(tr.mons[side][i].preview.trim_end_matches("-*")).iter().map(|k| k.1).sum::<f64>().max(1e-6) };
            rest.sort_by(|&a, &b| usage(b).total_cmp(&usage(a)).then(a.cmp(&b)));
            let need = size.saturating_sub(brought.len());
            let total: f64 = rest.iter().map(|&i| usage(i)).sum();
            for &i in rest.iter().take(need) {
                brought.push(i);
                let p = (need as f64 * usage(i) / total.max(1e-9)).min(1.0);
                bsrc.push((if prior.is_empty() { Src::Default } else { Src::Prior }, Some(p)));
            }
            if bsrc.iter().any(|b| b.0 != Src::Exact) {
                team_src[side] = Src::Prior;
            }
        }
        if brought.len() < n_active {
            return Err(err("team", format!("{}: {} mons known", side_key(side_ref(side)), brought.len())));
        }
        // Team (pick) order: the leads, then the rest by first sight.
        let leads: Vec<usize> = (0..n_active).filter_map(|k| tr.leads[side][k]).collect();
        if leads.len() != n_active {
            return Err(err("parse", format!("{}: no leads in the log", side_key(side_ref(side)))));
        }
        for &l in &leads {
            if !brought.contains(&l) {
                brought.push(l);
                bsrc.push((Src::Exact, None));
            }
        }
        let mut rest: Vec<usize> = brought.iter().copied().filter(|i| !leads.contains(i)).collect();
        rest.sort_by_key(|&i| (tr.mons[side][i].first_seen.unwrap_or(usize::MAX), i));
        let mut order: Vec<usize> = leads.iter().copied().chain(rest.iter().copied()).collect();
        if req.is_some() {
            // The bench order whose switch history ends in the request's order.
            let mut found = false;
            for p in permutations(&rest) {
                let o: Vec<usize> = leads.iter().copied().chain(p.iter().copied()).collect();
                if ps_positions(&tr, side, &o) == brought {
                    order = o;
                    found = true;
                    break;
                }
            }
            if !found {
                order_src[side] = Src::Inferred;
            }
        } else if rest.len() > 1 {
            order_src[side] = Src::Inferred;
        }
        let mut used_items: HashSet<String> = order.iter().filter_map(|&i| tr.mons[side][i].item_orig.clone()).map(|x| slug(&x)).collect();
        // Mons with exact items claim them before the prior picks.
        let mut ordered_for_prior: Vec<usize> = order.clone();
        ordered_for_prior.sort_by_key(|&i| tr.mons[side][i].item_orig.is_none());
        let mut by_mon: Vec<Option<SetPick>> = vec![None; tr.mons[side].len()];
        for &i in &ordered_for_prior {
            let mut sampler = opts.prior_sample_seed.map(|seed| PriorSampler::for_mon(seed, side, i));
            by_mon[i] = Some(resolve_set(&tr, side, i, req, user_sets, later.as_ref(), prior, champions, &mut used_items, sampler.as_mut())?);
        }
        picks[side] = order.iter().map(|&i| by_mon[i].clone().unwrap()).collect();
        brought_src[side] = order.iter().map(|i| bsrc[brought.iter().position(|b| b == i).unwrap()]).collect();
        team_orders[side] = order;
    }

    // HP, now that max HP is known.
    let mut hps: [Vec<(Value, Src, Option<(u16, u16)>)>; 2] = [Vec::new(), Vec::new()];
    for side in 0..2 {
        let req = if know(side) { requests[side].as_ref() } else { None };
        for (k, &i) in team_orders[side].iter().enumerate() {
            let m = &tr.mons[side][i];
            let sp = &picks[side][k];
            let (_, sdef) = species_def(&sp.species).ok_or_else(|| err("unknown species", sp.species.clone()))?;
            let max = stats_for(sdef, sp.level, &sp.nature.v, sp.spread.v, sp.sp, sp.ivs)[0];
            let req_hp = req.and_then(|r| request_mon(r, m)).and_then(|rm| rm.get("condition").and_then(Value::as_str).and_then(parse_hp));
            let h = match (req_hp, m.hp) {
                (Some((_, _, _, _, true)), _) => (json!(0.0), Src::Exact, None),
                (Some((n, d, _, _, false)), _) => (json!(n as f64 / d.max(1) as f64), Src::Exact, Some((n, n))),
                (None, _) if m.fainted => (json!(0.0), Src::Exact, None),
                (None, Some(HpObs::Exact(n, d))) => (json!(n as f64 / d.max(1) as f64), Src::Exact, Some((n, n))),
                (None, Some(HpObs::Pct(p, band))) => {
                    let (lo, hi) = pct_interval(p, band, max, champions);
                    let mid = lo + (hi - lo) / 2;
                    let src = if lo == hi && sp.spread.src == Src::Exact { Src::Exact } else { Src::Inferred };
                    (json!(mid as f64 / max as f64), src, Some((lo, hi)))
                }
                (None, _) => (json!(1.0), Src::Exact, Some((max, max))),
            };
            hps[side].push(h);
        }
    }

    // The board.
    let mut board = json!({});
    let mut sets_out = json!({});
    let mut board_orders: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    let mut board_pp: [Vec<Value>; 2] = [vec![Value::Null; team_orders[0].len()], vec![Value::Null; team_orders[1].len()]];
    let mut field = json!({"turn": opts.turn as i64});
    let setter_item = |s: usize, i: usize| -> Option<String> {
        let m = &tr.mons[s][i];
        if let Some(Some(x)) = &m.item_now {
            return Some(x.clone());
        }
        if let Some(x) = &m.item_orig {
            return Some(x.clone());
        }
        let k = team_orders[s].iter().position(|&x| x == i)?;
        picks[s][k].item.v.clone()
    };
    for side in 0..2 {
        let key = if side == 0 { "own" } else { "opp" };
        let req = if know(side) { requests[side].as_ref() } else { None };
        let order = &team_orders[side];
        let mut bidx: Vec<usize> = Vec::new();
        for slot in 0..n_active {
            let i = tr.slots[side][slot].ok_or_else(|| err("parse", "an empty active slot"))?;
            bidx.push(order.iter().position(|&x| x == i).ok_or_else(|| err("team", "an active mon is not in the team"))?);
        }
        for k in 0..order.len() {
            if !bidx.contains(&k) {
                bidx.push(k);
            }
        }
        let (mut act, mut ben, mut sact, mut sben) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (bpos, &k) in bidx.iter().enumerate() {
            let m = &tr.mons[side][order[k]];
            let (bm, set) = board_mon(&view, side, m, &picks[side][k], &hps[side][k], bpos < n_active, req, champions);
            board_pp[side][k] = bm["move_pp"].clone();
            if bpos < n_active {
                act.push(bm);
                sact.push(set);
            } else {
                ben.push(bm);
                sben.push(set);
            }
        }
        board[format!("{key}_active")] = json!(act);
        board[format!("{key}_bench")] = json!(ben);
        sets_out[format!("{key}_active")] = json!(sact);
        sets_out[format!("{key}_bench")] = json!(sben);
        board_orders[side] = bidx;
        field[format!("tera_used_{key}")] = json!(tr.tera_used[side]);
        field[format!("mega_used_{key}")] = json!(tr.mega_used[side]);
        let mut hazards = json!({"stealth_rock": 0, "spikes": 0, "toxic_spikes": 0, "sticky_web": 0, "safeguard": 0, "mist": 0});
        let mut screens = [false; 3];
        for fx in &tr.side_fx[side] {
            let e = field_elapsed(&view, fx.line);
            match fx.name.as_str() {
                "Tailwind" => {
                    field[format!("tailwind_{key}")] = json!(true);
                    field[format!("tailwind_{key}_elapsed")] = json!(e);
                }
                "Light Screen" | "Reflect" | "Aurora Veil" => {
                    let (k2, n) = match fx.name.as_str() {
                        "Light Screen" => (0, "light_screen"),
                        "Reflect" => (1, "reflect"),
                        _ => (2, "aurora_veil"),
                    };
                    screens[k2] = true;
                    let ext = fx.setter.and_then(|(s, i)| setter_item(s, i)).is_some_and(|x| slug(&x) == "lightclay");
                    field[format!("{n}_{key}_elapsed")] = json!(e);
                    field[format!("{n}_{key}_duration")] = json!(if ext { 8 } else { 5 });
                }
                "Stealth Rock" => hazards["stealth_rock"] = json!(1),
                "Sticky Web" => hazards["sticky_web"] = json!(1),
                "Spikes" => hazards["spikes"] = json!((hazards["spikes"].as_i64().unwrap_or(0) + 1).min(3)),
                "Toxic Spikes" => hazards["toxic_spikes"] = json!((hazards["toxic_spikes"].as_i64().unwrap_or(0) + 1).min(2)),
                "Safeguard" => hazards["safeguard"] = json!(1),
                "Mist" => hazards["mist"] = json!(1),
                other => return Err(err(&format!("unsupported: side {other}"), other.to_string())),
            }
        }
        field[format!("screens_{key}")] = json!(screens);
        field[format!("hazards_{key}")] = hazards;
    }
    if let Some(w) = &tr.weather {
        let name = if w.name == "Snow" { "Snowscape".to_string() } else { w.name.clone() };
        let rock = match name.as_str() {
            "SunnyDay" => "heatrock",
            "RainDance" => "damprock",
            "Sandstorm" => "smoothrock",
            _ => "icyrock",
        };
        let ext = w.setter.and_then(|(s, i)| setter_item(s, i)).is_some_and(|x| slug(&x) == rock);
        field["weather"] = json!(name);
        field["weather_elapsed"] = json!(field_elapsed(&view, w.line));
        field["weather_duration"] = json!(if ext { 8 } else { 5 });
    }
    if let Some(t) = &tr.terrain {
        let ext = t.setter.and_then(|(s, i)| setter_item(s, i)).is_some_and(|x| slug(&x) == "terrainextender");
        field["terrain"] = json!(t.name.trim_end_matches(" Terrain"));
        field["terrain_elapsed"] = json!(field_elapsed(&view, t.line));
        field["terrain_duration"] = json!(if ext { 8 } else { 5 });
    }
    for fx in &tr.pseudo {
        if fx.name == "Trick Room" {
            field["trick_room"] = json!(true);
            field["trick_room_elapsed"] = json!(field_elapsed(&view, fx.line));
        }
    }
    board["field"] = field;

    let mut b = vgc_engine_core::ps_board::position_from_board(&board, &sets_out, &fid, SideRef::P1).map_err(|e| err("board", e))?;

    // What a board does not carry.
    let receiver = [&b.p1, &b.p2].iter().any(|sd| sd.team.iter().any(|m| matches!(data::ABILITIES.get(m.ability_id as usize).map(|a| a.slug), Some("receiver" | "powerofalchemy"))));
    let foe_alive: [Vec<u8>; 2] = [0, 1].map(|side: usize| {
        let foe = if side == 0 { &b.p2 } else { &b.p1 };
        (0..n_active).filter(|&s| foe.team.get(foe.active[s] as usize).is_some_and(|m| m.is_alive())).map(|s| ((1 - side) * 2 + s) as u8).collect()
    });
    for side in 0..2 {
        let order = &team_orders[side];
        let bidx = &board_orders[side];
        let sd = if side == 0 { &mut b.p1 } else { &mut b.p2 };
        let built = std::mem::take(&mut sd.team);
        sd.team = (0..order.len()).map(|k| built[bidx.iter().position(|&x| x == k).unwrap()].clone()).collect();
        for slot in 0..n_active {
            sd.active[slot] = bidx[slot] as u8;
        }
        // Faint events are recorded only while Receiver / Power of Alchemy
        // could act on them (battle.rs receiver_possible).
        let mut handled = 0u8;
        for (k, m) in sd.team.iter().enumerate() {
            if m.fainted && k < 8 {
                handled |= 1 << k;
            }
        }
        sd.conditions.faint_handled = if receiver { handled } else { 0 };
        let pos = ps_positions(&tr, side, order);
        let mut ps = [0u8, 1, 2, 3, 4, 5];
        for (p, i) in pos.iter().enumerate().take(6) {
            ps[p] = order.iter().position(|x| x == i).unwrap_or(p) as u8;
        }
        let mut seen = [false; 6];
        let perm = ps[..order.len().min(6)].iter().all(|&x| (x as usize) < 6 && !std::mem::replace(&mut seen[x as usize], true));
        if perm {
            sd.ps_order = ps;
        }
        let unburden = data::ABILITIES.iter().position(|a| a.slug == "unburden").map(|x| x as u16).unwrap_or(u16::MAX);
        for (k, &i) in order.iter().enumerate() {
            let m = &tr.mons[side][i];
            let pm = &mut sd.team[k];
            // PS `lastItem` (Recycle, Belch) lasts the active stint: an item
            // used up since the mon last came in. A knocked-off item is not one.
            pm.consumed_item = m.used_item.as_deref().and_then(|x| data::ITEMS.iter().position(|d| d.slug == slug(x))).map(|x| x as u16).unwrap_or(u16::MAX);
            // A board skips a fainted mon's PP.
            for (j, v) in board_pp[side][k].as_array().into_iter().flatten().enumerate().take(4) {
                if let Some(x) = v.as_u64() {
                    pm.pp[j] = x.min(pm.max_pp(j) as u64) as u8;
                }
            }
            if m.active.is_some() {
                // A fainted active keeps these too (a request still lists its
                // moves, Champions Fake Out reads `move_actions`).
                pm.turns_active = m.turns_active.min(255) as u8;
                pm.move_actions = m.move_actions.min(255) as u8;
            }
            if pm.fainted {
                continue;
            }
            pm.times_attacked = m.times_attacked.min(255) as u8;
            if m.active.is_none() {
                continue;
            }
            if m.last_move.is_some() {
                pm.last_used_move_target = m.last_target.unwrap_or(255);
            }
            if let Some(mv) = &m.charging {
                // Mid two-turn move: the release is locked in. Its target is
                // not in the log (the charge line names none): the first
                // live foe.
                if let Some(j) = pm.moves.iter().position(|&x| x != u16::MAX && data::MOVES[x as usize].slug == slug(mv)) {
                    pm.charging_turns = 1;
                    pm.charging_move_slot = j as u8;
                    pm.last_used_move_slot = j as u8;
                    pm.semi_invuln = match slug(mv).as_str() {
                        "dig" => 1,
                        "dive" => 2,
                        "fly" => 3,
                        "bounce" => 4,
                        "phantomforce" => 5,
                        "shadowforce" => 6,
                        "skydrop" => 7,
                        _ => 0,
                    };
                    let single = data::MOVES[pm.moves[j] as usize].target;
                    pm.last_used_move_target = if matches!(single, 0 | 4 | 10 | 13) { foe_alive[side].first().copied().unwrap_or(255) } else { 255 };
                }
            }
            if m.vols.iter().any(|v| v.name == "?TORMENT") && !pm.volatiles.has(VolatileKind::Torment) {
                let _ = pm.volatiles.add(vgc_engine_core::Volatile { kind: VolatileKind::Torment, turns_remaining: 0, payload: 0 });
            }
            if let Some(v) = m.vols.iter().find(|v| v.name == "?OCTOLOCK") {
                // Payload: the source's side << 16 | slot << 8 | team index.
                let src = v.arg.split_once(',').and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)));
                if let Some((ss, si)) = src {
                    let slot = tr.mons[ss][si].active.unwrap_or(0) as u32;
                    let idx = team_orders[ss].iter().position(|&x| x == si).unwrap_or(0) as u32;
                    let _ = pm.volatiles.add(vgc_engine_core::Volatile { kind: VolatileKind::Octolock, turns_remaining: 0, payload: ((ss as u32) << 16) | (slot << 8) | idx });
                }
            }
            if let Some(v) = m.vols.iter().find(|v| v.name == "?DRAGONCHEER") {
                let _ = v;
                let (types, nt) = pm.effective_types();
                let boost: u8 = if (0..nt as usize).any(|t| types[t] == 14) { 2 } else { 1 };
                if !pm.volatiles.has(VolatileKind::DragonCheer) {
                    let _ = pm.volatiles.add(vgc_engine_core::Volatile { kind: VolatileKind::DragonCheer, turns_remaining: 0, payload: boost as u32 });
                    pm.crit_stage_volatile = pm.crit_stage_volatile.saturating_add(boost);
                }
            }
            // Last Resort's record: the moves used since it came in.
            pm.moves_used = 0;
            for mv in &m.stint_moves {
                if let Some(j) = pm.moves.iter().position(|&x| x != u16::MAX && data::MOVES[x as usize].slug == slug(mv)) {
                    pm.moves_used |= 1 << j;
                }
            }
            if m.stall_count > 0 || m.stall_used_last {
                pm.set_stall(m.stall_count, m.stall_used_last);
            }
            if let Some(n) = m.fallen {
                pm.supreme_overlord_fallen = n;
            }
            if let Some(tc) = &m.type_change {
                let ids: Vec<u8> = tc.split('/').filter_map(|t| data::TYPE_NAMES.iter().position(|x| x.eq_ignore_ascii_case(t.trim())).map(|x| x as u8)).collect();
                if let Some(&a) = ids.first() {
                    pm.set_type_override(a, ids.get(1).copied());
                }
            }
            if m.item_lost && m.item_lost_line.is_some_and(|l| l > m.switch_line) && pm.effective_ability_id() == unburden {
                pm.unburden_active = true;
            }
        }
    }
    // Transform / Imposter: the target's species, ability, stats (its own HP),
    // moves at 5 PP, as battle.rs transform_into copies them.
    for side in 0..2 {
        for (k, &i) in team_orders[side].iter().enumerate() {
            let m = &tr.mons[side][i];
            let (Some((ts, ti)), Some(_)) = (m.transformed, m.active) else { continue };
            let Some(tk) = team_orders[ts].iter().position(|&x| x == ti) else { continue };
            let target = b.side(side_ref(ts)).team[tk].clone();
            let pm = if side == 0 { &mut b.p1.team[k] } else { &mut b.p2.team[k] };
            if pm.fainted {
                continue;
            }
            let mut pv = serde_json::to_value(&*pm).map_err(|e| err("board", e.to_string()))?;
            pv["transform_base"] = json!({"species_id": pm.species_id, "ability_id": pm.ability_id, "stats": pv["stats"].clone(), "moves": pv["moves"].clone(), "pp": pv["pp"].clone()});
            let mut np: vgc_engine_core::Pokemon = serde_json::from_value(pv).map_err(|e| err("board", e.to_string()))?;
            np.change_forme(target.species_id, false);
            np.ability_id = target.ability_id;
            let hp = np.stats.hp;
            np.stats = target.stats;
            np.stats.hp = hp;
            np.moves = target.moves;
            for j in 0..4 {
                let base = if np.moves[j] == u16::MAX { 0 } else { data::MOVES[np.moves[j] as usize].pp.min(5) };
                let used = m.trans_pp.iter().find(|(mv, _)| np.moves[j] != u16::MAX && slug(mv) == data::MOVES[np.moves[j] as usize].slug).map(|x| x.1).unwrap_or(0);
                np.pp[j] = (base as u32).saturating_sub(used) as u8;
            }
            np.last_used_move_slot = m.last_move.as_deref().and_then(|lm| np.moves.iter().position(|&x| x != u16::MAX && data::MOVES[x as usize].slug == slug(lm))).map(|x| x as u8).unwrap_or(255);
            *pm = np;
        }
    }
    // Speed Swap, Power / Guard Split and Power Trick, in log order.
    let mut ops: Vec<(usize, String, (usize, usize), Option<(usize, usize)>)> = Vec::new();
    for side in 0..2 {
        for (i, m) in tr.mons[side].iter().enumerate() {
            if m.active.is_some() {
                for (line, kind, other) in &m.stat_ops {
                    if !ops.iter().any(|o| o.0 == *line && o.1 == *kind) {
                        ops.push((*line, kind.clone(), (side, i), *other));
                    }
                }
            }
        }
    }
    ops.sort_by_key(|o| o.0);
    for (_, kind, a, other) in ops {
        let pos = |(s, i): (usize, usize)| team_orders[s].iter().position(|&x| x == i).map(|k| (s, k));
        let Some((sa, ka)) = pos(a) else { continue };
        let get = |b: &Battle, s: usize, k: usize| b.side(side_ref(s)).team[k].stats;
        let mut x = get(&b, sa, ka);
        let other_k = other.and_then(pos);
        let mut y = other_k.map(|(s, k)| get(&b, s, k));
        match (kind.as_str(), y.as_mut()) {
            ("speedswap", Some(y)) => std::mem::swap(&mut x.spe, &mut y.spe),
            ("powersplit", Some(y)) => {
                let (at, sa2) = ((x.atk + y.atk) / 2, (x.spa + y.spa) / 2);
                (x.atk, y.atk, x.spa, y.spa) = (at, at, sa2, sa2);
            }
            ("guardsplit", Some(y)) => {
                let (d, sd) = ((x.def + y.def) / 2, (x.spd + y.spd) / 2);
                (x.def, y.def, x.spd, y.spd) = (d, d, sd, sd);
            }
            ("powertrick", _) => std::mem::swap(&mut x.atk, &mut x.def),
            _ => continue,
        }
        let set = |b: &mut Battle, s: usize, k: usize, st| if s == 0 { b.p1.team[k].stats = st } else { b.p2.team[k].stats = st };
        set(&mut b, sa, ka, x);
        if let (Some((s, k)), Some(y)) = (other_k, y) {
            if tr.mons[s][team_orders[s][k]].active.is_some() {
                set(&mut b, s, k, y);
            }
        }
    }
    for fx in &tr.pseudo {
        let e = field_elapsed(&view, fx.line).clamp(1, 255) as u8;
        let left = vgc_engine_core::ps_board::remaining(5, e);
        match fx.name.as_str() {
            "Gravity" => b.gravity_turns = left,
            "Magic Room" => b.magic_room_turns = left,
            "Wonder Room" => b.wonder_room_turns = left,
            _ => {}
        }
    }
    for side in 0..2 {
        for fx in &tr.side_fx[side] {
            let e = field_elapsed(&view, fx.line).clamp(1, 255) as u8;
            let c = if side == 0 { &mut b.p1.conditions } else { &mut b.p2.conditions };
            match fx.name.as_str() {
                "Safeguard" => c.safeguard_turns = vgc_engine_core::ps_board::remaining(5, e),
                "Mist" => c.mist_turns = vgc_engine_core::ps_board::remaining(5, e),
                _ => {}
            }
        }
    }
    // Future Sight / Doom Desire: the target slot's pending hit (3 end-of-turn
    // ticks from the use). Set through the snapshot form: the field is the
    // engine's own.
    if !tr.futures.is_empty() {
        let mut v = serde_json::to_value(&b).map_err(|e| err("board", e.to_string()))?;
        for (ts, tslot, (ss, si), mv, line) in &tr.futures {
            let turns = 3 - (field_elapsed(&view, *line) - 1).clamp(0, 2);
            let src = team_orders[*ss].iter().position(|x| x == si).unwrap_or(0);
            let id = data::MOVES.iter().position(|d| d.slug == slug(mv)).unwrap_or(0);
            v["future_pending"][*ts][*tslot] = json!({"turns": turns, "source_side": ss, "source_team_idx": src, "move_id": id});
        }
        b = serde_json::from_value(v).map_err(|e| err("board", e.to_string()))?;
    }
    b.rehydrate_caches();

    // The prefix: the real history, as each player saw it.
    let kept: Vec<String> = if kind == InputKind::Raw { head.to_vec() } else { head.iter().filter(|l| keep_for_prefix(l)).cloned().collect() };
    let prefix = match kind {
        InputKind::Raw | InputKind::Spectator => Prefix::Raw(kept),
        InputKind::PerSide(own) => {
            let o = own as usize;
            let other: Vec<String> = kept.iter().map(|l| to_shared(l, o, champions)).collect();
            if o == 0 { Prefix::PerSide([kept, other]) } else { Prefix::PerSide([other, kept]) }
        }
    };

    // The report.
    let mut sides = Map::new();
    let mut active_prior = Map::new();
    let (mut n_prior, mut n_fields) = (0usize, 0usize);
    for side in 0..2 {
        let sref = side_ref(side);
        let mut mons = Vec::new();
        for (k, &i) in team_orders[side].iter().enumerate() {
            let m = &tr.mons[side][i];
            let built = &b.side(sref).team[k];
            let mut r = mon_report(m, &picks[side][k], built, (hps[side][k].1, hps[side][k].2), brought_src[side][k], champions);
            if let Some(slot) = (0..n_active).find(|&s| b.side(sref).active[s] as usize == k) {
                let tag = format!("{}{}", side_key(sref), (b'a' + slot as u8) as char);
                r["active_slot"] = json!(tag);
                active_prior.insert(tag, r["prior_fields"].clone());
            }
            r["team_index"] = json!(k);
            n_prior += r["prior_fields"].as_array().map(Vec::len).unwrap_or(0);
            n_fields += r["n_fields"].as_u64().unwrap_or(0) as usize;
            mons.push(r);
        }
        sides.insert(side_key(sref).to_string(), json!({
            "team": mons,
            "brought": team_src[side].as_str(),
            "team_order": order_src[side].as_str(),
            "knowledge": if know(side) { "full" } else { "viewer" },
            "player": tr.names[side],
        }));
    }
    let mut field_rep = Map::new();
    for (k, v) in board["field"].as_object().into_iter().flatten() {
        let src = if k.ends_with("_duration") { Src::Inferred } else { Src::Exact };
        field_rep.insert(k.clone(), prov(v.clone(), src, None));
    }
    let report = json!({
        "turn": opts.turn,
        "stop": if opts.stop == Stop::Move { "move" } else { "replace" },
        "format": fid,
        "input": kind.name(),
        "viewer": opts.viewer.map(side_key),
        "lookahead": opts.lookahead,
        "prior": prior.source,
        "prior_sha256": prior.sha256,
        "sides": Value::Object(sides),
        "field": Value::Object(field_rep),
        "active_prior_fields": Value::Object(active_prior),
        "n_prior_fields": n_prior,
        "n_fields": n_fields,
    });
    let mut report = report;
    if let Some(seed) = opts.prior_sample_seed {
        report["prior_sample_seed"] = json!(seed);
    }
    Ok(Reconstruction { battle: b, format_id: fid, prefix, player_names: tr.names.clone(), report, board, sets: sets_out })
}

/// The lines the state is read from, and which sides show exact HP there.
fn channel_view(head: &[String], kind: InputKind, viewer: Option<SideRef>) -> (Vec<String>, [bool; 2]) {
    match kind {
        InputKind::Raw => {
            let (ch, ex) = match viewer {
                None => (-1, [true, true]),
                Some(SideRef::P1) => (1, [true, false]),
                Some(SideRef::P2) => (2, [false, true]),
            };
            (raw_channel(head, ch), ex)
        }
        InputKind::Spectator => (head.iter().filter(|l| !l.is_empty()).cloned().collect(), [false, false]),
        InputKind::PerSide(own) => {
            let mut ex = [false, false];
            ex[own as usize] = true;
            (head.iter().filter(|l| !l.is_empty() && !l.starts_with("|request|")).cloned().collect(), ex)
        }
    }
}

/// The last `|request|` at or just after the stop in a per-side stream.
fn embedded_request(lines: &[String], cut: usize) -> Option<Value> {
    let mut end = cut;
    while end < lines.len() && (lines[end].starts_with("|request|") || lines[end].is_empty()) {
        end += 1;
    }
    lines[..end].iter().rev().find_map(|l| l.strip_prefix("|request|")).and_then(|j| serde_json::from_str::<Value>(j).ok()).filter(|v| !v.is_null())
}

/// An own-side exact HP line as the other side sees it (a percentage).
fn to_shared(l: &str, own: usize, champions: bool) -> String {
    let p: Vec<&str> = l.split('|').collect();
    let k = match p.get(1).copied() {
        Some("switch" | "drag" | "replace") => 4,
        Some("-damage" | "-heal" | "-sethp") => 3,
        _ => return l.to_string(),
    };
    let Some((side, _, _)) = p.get(2).and_then(|id| parse_ident(id)) else { return l.to_string() };
    if side != own {
        return l.to_string();
    }
    let Some(hp) = p.get(k) else { return l.to_string() };
    let Some((n, d, _, status, fnt)) = parse_hp(hp) else { return l.to_string() };
    if fnt {
        return l.to_string();
    }
    let mut txt = pct_text(n, d, champions);
    if let Some(s) = status {
        txt.push(' ');
        txt.push_str(&s);
    }
    let mut q: Vec<String> = p.iter().map(|x| x.to_string()).collect();
    q[k] = txt;
    q.join("|")
}

fn permutations(v: &[usize]) -> Vec<Vec<usize>> {
    if v.len() <= 1 {
        return vec![v.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..v.len() {
        let mut rest = v.to_vec();
        let x = rest.remove(i);
        for mut p in permutations(&rest) {
            p.insert(0, x);
            out.push(p);
        }
    }
    out
}

/// PS `side.pokemon` order (mon indices) after the log's switches, for a
/// team picked in `order`: every switch swaps the incoming and outgoing
/// mons' places; Ally Switch swaps the two actives'.
fn ps_positions(tr: &Tracker, side: usize, order: &[usize]) -> Vec<usize> {
    let mut pos: Vec<usize> = order.to_vec();
    for &(s, slot, i) in &tr.switches {
        if s != side {
            continue;
        }
        if slot == usize::MAX {
            if pos.len() >= 2 {
                pos.swap(0, 1);
            }
            continue;
        }
        if let Some(p) = pos.iter().position(|&x| x == i) {
            if p != slot && slot < pos.len() {
                pos.swap(p, slot);
            }
        }
    }
    pos
}
