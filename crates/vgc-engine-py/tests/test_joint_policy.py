import json
import vgc_engine


def test_exact_solver_exposes_executable_doubles_policy():
    team = json.dumps([{'species': 'rillaboom', 'moves': ['protect']}] * 2)
    battle = vgc_engine.Battle.from_teams(team, team)
    result = vgc_engine.solve_endgame_exact(battle, max_depth=1, node_budget=100, exact_hp=True)
    for side in ['row', 'col']:
        policy = result[side + '_joint_policy']
        assert abs(sum(p for _, p in policy) - 1) < 1e-8
        assert all({c[1] for c in joint} == {0, 1} for joint, p in policy if p > 0)
    assert result['nodes_visited'] > 0
    a = result['row_joint_policy'][0][0]
    b = result['col_joint_policy'][0][0]
    assert battle.step_move(a, b) == 'continue'
    assert battle.turn == 1


def test_replacements_are_separate_and_can_fill_one_of_two_empty_slots():
    team = json.dumps([{'species': 'rillaboom', 'moves': ['protect']}] * 3)
    b = vgc_engine.Battle.from_teams(team, team, tera_allowed=False, decision_phases=True)
    state = json.loads(b.to_json())
    for p in state['p1']['team'][:2]:
        p['current_hp'] = 0
        p['fainted'] = True
    b = vgc_engine.Battle.from_json(json.dumps(state))
    assert b.needs_replacements()
    hp = b.active_hp(1, 0)
    result = vgc_engine.solve_endgame_exact(b, max_depth=1, node_budget=100, exact_hp=True)
    assert result['row_joint_policy']
    for joint, probability in result['row_joint_policy']:
        assert sum(c[0] == 'switch' for c in joint) == 1
    b.step_move(result['row_joint_policy'][0][0], result['col_joint_policy'][0][0])
    assert b.turn == 0
    assert b.active_hp(1, 0) == hp
    assert not b.needs_replacements()


def test_pp_overlay_roundtrip_and_no_tera():
    team = json.dumps([{'species': 'rillaboom', 'moves': ['protect']}]*2)
    b = vgc_engine.Battle.from_teams(team, team, move_pp_json='{"protect":8}', tera_allowed=False)
    b = vgc_engine.Battle.from_json(b.to_json())
    assert b.observe()['sides'][0]['active'][0]['moves'][0]['max_pp'] == 8
    assert all(c[0] != 'tera' for c in b.legal_choices(0, 0))
    actions = [('move', 0, 0, -1, -1), ('move', 1, 0, -1, -1)]
    b.step_move(actions, actions)
    assert b.observe()['sides'][0]['active'][0]['moves'][0]['pp'] == 7


def test_chance_budget_reports_unsearched_cells_and_is_repeatable():
    team = json.dumps([{'species': 'rillaboom', 'moves': ['woodhammer', 'protect']}] * 2)
    b = vgc_engine.Battle.from_teams(team, team, tera_allowed=False, decision_phases=True)
    args = dict(max_depth=1, node_budget=64, max_actions=4, max_chance_combinations=1)
    a = vgc_engine.solve_endgame_exact(b, **args)
    c = vgc_engine.solve_endgame_exact(b, **args)
    assert a == c
    assert a['chance_cutoffs'] > 0
    assert a['provenance'] == 'chance_limit'
    assert b.turn == 0
