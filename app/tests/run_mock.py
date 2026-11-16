"""Runs the Mentor addon inside tests/mock_wow.lua and writes a SavedVariables file.
Usage: python run_mock.py <output wow folder>"""
import pathlib, sys
from lupa import LuaRuntime

here = pathlib.Path(__file__).resolve().parent
mock = pathlib.Path(sys.argv[1])
target = mock / "WTF" / "Account" / "1#1" / "SavedVariables"
target.mkdir(parents=True, exist_ok=True)
lua = LuaRuntime(unpack_returned_tuples=True)
g = lua.globals()
g.ADDON_DIR = str(here.parent / "src-tauri" / "resources" / "addon" / "Mentor").replace("\\", "/")
g.OUTPUT_FILE = str(target / "Mentor.lua").replace("\\", "/")
lua.execute("unpack = unpack or table.unpack")
lua.execute((here / "mock_wow.lua").read_text(encoding="utf-8"))
snap = g.MentorDB.snapshots[g.MentorDB.lastCharacterKey]
errors = snap.meta.errors
print("errors:", list(errors.values()) if errors else "none")
print("sections:", sorted(snap.keys()))
print("macros created in game:", [dict(m) for m in g.MOCK_CREATED_MACROS.values()])
print("bank items:", len(snap.bank["items"]), "| recipes:", list(snap.recipes.keys()), "| action slots:", len(snap.actionBars.slots), "| pet:", snap.pet.name, list(snap.pet.spells.values()))
# Talents come from the trait system of WoW Forever: trees in their order, talents on a grid.
talents = snap.talents
trees = [talents.trees[index] for index in sorted(talents.trees.keys())]
assert [tree.name for tree in trees] == ["Affliction", "Démonologie"], [tree.name for tree in trees]
affliction = [dict(trees[0].talents[index]) for index in sorted(trees[0].talents.keys())]
grid = [(t["name"], t["rank"], t["maxRank"], t["tier"], t["column"]) for t in affliction]
assert grid == [
    ("Sort 18174", 2, 5, 1, 1),
    ("Sort 17810", 5, 5, 1, 2),
    ("Sort 9999", 0, 5, 2, 2),
    ("Malédiction amplifiée", 0, 1, 2, 3),
    ("Sort 18220", 1, 1, 3, 2),
    ("Sort 18213", 0, 2, 4, 1),
    ("Sort 18094", 0, 3, 5, 1),
], grid
assert trees[0].points == 8 and talents.unspent == 3 and talents.source == "traits" and talents.problem is None
assert dict(affliction[2]["requires"]) == {"tier": 1, "column": 2, "name": "Sort 17810"}
assert affliction[3]["description"] == "Ta prochaine malédiction est plus forte."
assert affliction[2]["description"] == "SetSpellByID line 9999/nil Second line", affliction[2]["description"]
assert list(affliction[4]["choices"].values()) == ["Sort 18265", "Sort 18220"] and affliction[4]["chosen"] is True
assert talents.incomplete is None
assert talents.trace is None and snap.meta.addonVersion == "1.5.1"
print("talents:", [(tree.name, tree.points, len(tree.talents)) for tree in trees], "| unspent", talents.unspent)
print("spell sample:", dict(snap.spells[1]))
print("action bar sample:", [dict(s) for s in snap.actionBars.slots.values()])

# Chat commands: the addon must never rely on a name the game already uses.
run = g.RunSlash
assert run("/mentor show") == "MENTOR", "/mentor must reach the addon"
assert g.SLASH_MENTOR2 == "/men" and run("/men hide") == "MENTOR", "the short command must reach the addon"
assert g.MentorCharDB.trackerHidden is True
assert run("/mt sync") == "builtin", "the mock must reproduce the game's own /mt"
for index in (1, 2):
    name = g["SLASH_MENTOR%d" % index]
    taken = [key for key in list(g.keys()) if isinstance(key, str) and key.startswith(("SLASH_", "EMOTE")) and not key.startswith("SLASH_MENTOR") and g[key] == name]
    assert not taken, "%s is already used by %s" % (name, taken)

# When another command already has the short name, the addon gives it up.
other = LuaRuntime(unpack_returned_tuples=True)
o = other.globals()
o.ADDON_DIR, o.OUTPUT_FILE = g.ADDON_DIR, str(mock / "classic.lua").replace("\\", "/")  # keep the main file for the app tests
other.execute("unpack = unpack or table.unpack")
other.execute('SLASH_SOMEADDON1 = "/MEN"')
other.execute("MOCK_NO_TRAITS = true")
other.execute((here / "mock_wow.lua").read_text(encoding="utf-8"))
assert o.SLASH_MENTOR2 is None and o.RunSlash("/mentor show") == "MENTOR"
# Without the trait system (Classic clients) the original talent functions are used.
classic = o.MentorDB.snapshots[o.MentorDB.lastCharacterKey].talents
assert classic.source == "classic" and classic.trees[1].name == "Affliction" and classic.trees[1].talents[2].name == "Talent 1-2"
print("chat commands: /mentor and /men reach the addon, /mt stays the game's command")
