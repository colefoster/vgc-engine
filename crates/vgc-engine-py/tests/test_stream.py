"""vgc_engine.BattleStream: Pokémon Showdown's BattleStream in-process
(docs/protocol.md, "Use as a local Showdown replacement").

Build/install the extension first (`maturin develop`), then run
`python -m pytest crates/vgc-engine-py/tests/`.
"""

import json
import random

import pytest
import vgc_engine

TEAM = ("Incineroar||SitrusBerry|Intimidate|FakeOut,FlareBlitz,PartingShot,Protect||252,,,,,||||50|]"
        "Amoonguss||RockyHelmet|Regenerator|Spore,RagePowder,PollenPuff,Protect||||||50|]"
        "Rillaboom||AssaultVest|GrassySurge|FakeOut,GrassyGlide,WoodHammer,Uturn||||||50|]"
        "Urshifu-Rapid-Strike||MysticWater|UnseenFist|SurgingStrikes,CloseCombat,AquaJet,Protect||||||50|")
START = '>start {"formatid":"gen9vgc2025regi","seed":"sodium,00000000000000000000000000000001"}'


def started(**kw):
    s = vgc_engine.BattleStream(timestamp=1, **kw)
    s.write(START)
    s.write('>player p1 ' + json.dumps({"name": "A", "team": TEAM}))
    s.write('>player p2 ' + json.dumps({"name": "B", "team": TEAM}))
    return s


def test_messages_have_ps_framing():
    s = started()
    m = s.read_all()
    assert m[0] == "update\n|t:|1\n|gametype|doubles"
    assert m[1] == "update\n|player|p1|A||"
    assert m[2].startswith("update\n|player|p2|B||\n|gen|9\n|tier|[Gen 9] VGC 2025 Reg I\n")
    assert m[3].startswith('sideupdate\np1\n|request|{"teamPreview":true,"maxChosenTeamSize":4')
    assert m[4].startswith("sideupdate\np2\n|request|")
    s.write(">p1 team 9")
    assert s.read_all() == ["sideupdate\np1\n|error|[Invalid choice] Can't choose for Team Preview: "
                            "You do not have a Pokémon in slot 9"]
    s.write(">p1 team 1234")
    assert s.read() is None
    s.write(">p1 undo\n>p1 team 2134\n>p2 team 1234")
    m = s.read_all()
    assert m[0].startswith("update\n|\n|t:|1\n|teamsize|p1|4")
    s.write(">forcewin p2")
    m = s.read_all()
    assert m[0] == "update\n|\n|win|B"
    end = json.loads(m[1].split("\n", 1)[1])
    assert end["winner"] == "B" and end["turns"] == 1 and end["score"] == [4, 4]
    assert end["inputLog"][-3:] == [">p1 team 2, 1, 3, 4", ">p2 team 1, 2, 3, 4", ">forcewin p2"]
    assert s.ended


def test_player_streams_play_a_battle_to_the_end():
    s = started()
    streams = s.player_streams()
    rng = random.Random(7)
    for _ in range(300):
        if s.ended:
            break
        for side in (streams.p1, streams.p2):
            req = None
            for chunk in side.read_all():
                for line in chunk.split("\n"):
                    if line.startswith("|request|"):
                        req = json.loads(line[9:])
            if req and not req.get("wait"):
                if req.get("teamPreview"):
                    side.write("team 1234")
                else:
                    side.write("default")
        if rng.random() < 0.01:
            streams.omniscient.write(">tiebreak")
    else:
        streams.omniscient.write(">forcetie")
    assert s.ended
    spect = "\n".join(streams.spectator.read_all())
    assert "|split|" not in spect


def test_bad_commands_raise_like_ps():
    s = vgc_engine.BattleStream()
    with pytest.raises(ValueError):
        s.write('>start {"formatid":"gen1notaformat"}')
    s = started()
    with pytest.raises(ValueError):
        s.write(">bogus 1")
