-- Minimal World of Warcraft client mock to run the Mentor addon outside the game.
-- Usage (from Python/lupa or any Lua 5.1+): set ADDON_DIR and OUTPUT_FILE globals, then run this file.

local events = {}
local frames = {}
local createdMacros = {}
MOCK_CREATED_MACROS = createdMacros

local function noop() end

local Frame = {}
Frame.__index = function(self, key)
    local method = rawget(Frame, key)
    if method then
        return method
    end
    return noop
end

function Frame:SetScript(name, handler)
    local scripts = rawget(self, "scripts") or {}
    rawset(self, "scripts", scripts)
    scripts[name] = handler
end
function Frame:RegisterEvent(name)
    events[name] = events[name] or {}
    table.insert(events[name], self)
end
function Frame:UnregisterEvent() end
function Frame:CreateFontString()
    return setmetatable({}, Frame)
end
function Frame:GetStringHeight()
    return 12
end
function Frame:NumLines()
    return #(rawget(self, "lines") or {})
end
function Frame:IsShown()
    return true
end
function Frame:GetText()
    return rawget(self, "text")
end
local function tooltipSetter(method)
    return function(self, ...)
        local args = { ... }
        local lines = { "Header", method .. " line " .. tostring(args[1]) .. "/" .. tostring(args[2]), "Second line" }
        rawset(self, "lines", lines)
        for index = 1, 30 do
            rawset(_G[rawget(self, "name") .. "TextLeft" .. index], "text", lines[index])
        end
    end
end
Frame.SetTalent = tooltipSetter("SetTalent")
Frame.SetSpellByID = tooltipSetter("SetSpellByID")
Frame.SetBagItem = tooltipSetter("SetBagItem")

function CreateFrame(kind, name)
    local frame = setmetatable({ kind = kind, name = name }, Frame)
    if name then
        _G[name] = frame
        for index = 1, 30 do
            _G[name .. "TextLeft" .. index] = setmetatable({}, Frame)
            _G[name .. "TextRight" .. index] = setmetatable({}, Frame)
        end
    end
    table.insert(frames, frame)
    return frame
end

function FireEvent(name, ...)
    for _, frame in ipairs(events[name] or {}) do
        rawget(frame, "scripts").OnEvent(frame, name, ...)
    end
end

UIParent, WorldFrame, ChatFontNormal = CreateFrame("Frame"), CreateFrame("Frame"), {}
UISpecialFrames = {}
SlashCmdList = {}
-- Commands of the game itself: they win over an addon command with the same name.
SLASH_MAINTANKON1, SLASH_MAINTANKON2 = "/maintank", "/mt"
SLASH_MAINASSISTON1, SLASH_MAINASSISTON2 = "/mainassist", "/ma"
EMOTE1_CMD1 = "/moo"

-- Runs a chat command like the client: built-in commands first, then addon commands.
-- Returns "builtin", the name of the addon command that handled it, or nil.
function RunSlash(text)
    local command, rest = text:match("^(/%S+)%s*(.*)$")
    command = command:lower()
    for name, value in pairs(_G) do
        if type(name) == "string" and type(value) == "string" and value == command
            and (name:find("^SLASH_MAIN") or name:find("^EMOTE%d+_CMD")) then
            return "builtin"
        end
    end
    for key, handler in pairs(SlashCmdList) do
        local index = 1
        while _G["SLASH_" .. key .. index] do
            if _G["SLASH_" .. key .. index]:lower() == command then
                handler(rest)
                return key
            end
            index = index + 1
        end
    end
    return nil
end
time, date = os.time, os.date
function strtrim(text) return (text:gsub("^%s+", ""):gsub("%s+$", "")) end
function strlenutf8(text) return #text end
ReloadUI = noop
function InCombatLockdown() return false end
C_Timer = { After = function(_, callback) callback() end }
function GetBuildInfo() return "1.60.1", "70862", "Nov 1 2026", 16001 end
C_AddOns = { GetAddOnMetadata = function() return "1.3.0" end }

-- Character
function UnitName(unit) return ({ player = "Jeanne", pet = "Kinnixa", party1 = "Ami" })[unit] end
function GetRealmName() return "Ocre" end
function UnitClass(unit) return "Démoniste", "WARLOCK" end
function UnitRace() return "Mort-vivant" end
function UnitFactionGroup() return "Horde" end
function UnitLevel(unit) return unit == "pet" and 11 or 12 end
function UnitXP() return 5661 end
function UnitXPMax() return 10100 end
function GetXPExhaustion() return nil end
function GetMoney() return 450 end
function GetZoneText() return "Fossoyeuse" end
function GetSubZoneText() return "Canaux" end
C_Map = {
    GetBestMapForUnit = function() return 1458 end,
    GetMapInfo = function() return { name = "Fossoyeuse" } end,
    GetPlayerMapPosition = function() return { GetXY = function() return 0.688, 0.365 end } end,
}
function UnitExists(unit) return unit == "player" or unit == "pet" end
function UnitHealthMax(unit) return unit == "pet" and 180 or 258 end
function UnitPowerMax() return 493 end
function UnitStat(_, index) return 20, 20 + index end
function UnitArmor() return 200, 240 end
function UnitAttackPower() return 30, 2, 0 end
function UnitRangedAttackPower() return 10, 0, 0 end
function GetCritChance() return 5.123 end
function GetRangedCritChance() return 4 end
function GetHitModifier() return 0 end
function GetSpellHitModifier() return 1 end
function GetDodgeChance() return 4.5 end
function GetParryChance() return 0 end
function GetBlockChance() return 0 end
function GetSpellBonusHealing() return 0 end
function GetSpellCritChance(school) return school == 6 and 2.5 or 1.7 end
function GetSpellBonusDamage(school) return school == 6 and 12 or 3 end
function UnitDefense() return 60, 0 end
function GetManaRegen() return 1.234, 0.3 end
function UnitAttackSpeed() return 2.0, nil end
function UnitDamage() return 7.2, 14.9 end
function UnitResistance(_, index) return 0, index == 5 and 10 or 0 end

-- Items
local LINK = "|cff9d9d9d|Hitem:1433::::::::12:::::|h[Armure en patchwork]|h|r"
local STAFF = "|cff1eff00|Hitem:2000::::::::12:::::|h[Bâton du crépuscule]|h|r"
function GetInventoryItemLink(_, slot) return slot == 5 and LINK or nil end
function GetInventoryItemID() return 1433 end
C_Item = {
    GetItemInfo = function(query)
        if tostring(query):find("Bâton") or query == 2000 then
            return "Bâton du crépuscule", STAFF, 2, 14, 10, "Arme", "Bâtons", 1, "INVTYPE_2HWEAPON", 0, 300
        end
        return "Armure en patchwork", LINK, 0, 7, 2, "Armure", "Tissu", 1, "INVTYPE_CHEST", 0, 14
    end,
    DoesItemExist = function() return true end,
    GetCurrentItemLevel = function() return 7 end,
    GetItemNameByID = function(id) return id == 1433 and "Armure en patchwork" or "Objet " .. id end,
}
ItemLocation = { CreateFromEquipmentSlot = function() return {} end, CreateFromBagAndSlot = function() return {} end }
C_TooltipInfo = {
    GetInventoryItem = function() return { lines = { { leftText = "Armure en patchwork" }, { leftText = "Armure : 13" } } } end,
}
NUM_BAG_SLOTS, BANK_CONTAINER, NUM_BANKBAGSLOTS = 4, -1, 6
C_Container = {
    GetContainerNumSlots = function(bag) return ({ [0] = 4, [-1] = 3, [5] = 2 })[bag] or 0 end,
    GetContainerItemInfo = function(bag, slot)
        if slot == 1 then
            return { hyperlink = bag == -1 and STAFF or LINK, itemID = bag == -1 and 2000 or 1433, stackCount = 1, quality = 1, isBound = true }
        end
        if slot == 2 and bag == 0 then
            return { hyperlink = "|cffffffff|Hitem:858::::::::12:::::|h[Potion de soins inférieure]|h|r", itemID = 858, stackCount = 5, quality = 1 }
        end
        return nil
    end,
}

-- Quests
C_QuestLog = {
    GetNumQuestLogEntries = function() return 1 end,
    GetInfo = function() return { questID = 1, title = "Les rats des canaux", level = 10, isHeader = false } end,
    IsComplete = function() return false end,
    GetQuestObjectives = function() return { { text = "Rats tués : 3/10", finished = false } } end,
}

-- Talents (classic API)
function GetNumTalentTabs() return 3 end
function GetTalentTabInfo(tab) return ({ "Affliction", "Démonologie", "Destruction" })[tab], "icon", tab == 1 and 2 or 0, "file" end
function GetNumTalents() return 2 end
function GetTalentInfo(tab, index) return "Talent " .. tab .. "-" .. index, "icon", index, 2, (tab == 1 and index == 1) and 2 or 0, 5 end
function GetTalentPrereqs(_, index) if index == 2 then return 1, 2, true end end
function UnitCharacterPoints() return 1 end

-- Talents (WoW Forever): one trait tree whose groups are the class's three trees, as the
-- game's C_Traits functions return them. MOCK_NO_TRAITS leaves only the classic functions.
if not MOCK_NO_TRAITS then
    local GROUPS = {
        { groupID = 12, displayName = "Démonologie", orderIndex = 2 },
        { groupID = 13, displayName = "Destruction", orderIndex = 3 },
        { groupID = 11, displayName = "Affliction", orderIndex = 1 },
    }
    -- id = { group, x, y, rank, maxRanks, entries, unlocks, visible, active entry }
    local NODES = {
        [101] = { 11, 1200, 1200, 2, 5, { 1011 } },
        [102] = { 11, 1800, 1200, 5, 5, { 1021 }, { 104 } },
        [103] = { 11, 2403, 1800, 0, 1, { 1031 } },               -- a few units off its column
        [104] = { 11, 1800, 1800, 0, 5, { 1041 } },
        [105] = { 11, 1800, 24000, 1, 1, { 1051, 1052 }, nil, true, 1052 }, -- a zero too many, and a choice
        [106] = { 11, 2400, 1200, 0, 1, { 1061 }, nil, false },   -- hidden
        [107] = { 11, 1200, 3000, 0, 2, { 1071 } },
        [108] = { 11, 1200, 3600, 0, 3, { 1081 } },
        [201] = { 12, 4200, 1200, 0, 5, { 2011 } },
        [301] = { 99, 6000, 1200, 0, 1, {} },                      -- no talent in it
    }
    local DEFINITIONS = {
        [1011] = { spellID = 18174 },
        [1021] = { spellID = 17810 },
        [1031] = { spellID = 18179, overrideName = "Malédiction amplifiée", overrideDescription = "Ta prochaine malédiction est |cffffffffplus forte|r." },
        [1041] = { spellID = 9999 },
        [1051] = { spellID = 18265 },
        [1052] = { spellID = 18220 },
        [1061] = { spellID = 1 },
        [1071] = { spellID = 18213 },
        [1081] = { spellID = 18094 },
        [2011] = { spellID = 18692 },
    }
    C_ClassTalents = { GetActiveConfigID = function() return 7 end }
    C_Traits = {
        GenerateImportString = function() return "CIdBa0K/mock" end,
        GetConfigInfo = function(configID) return configID == 7 and { ID = 7, treeIDs = { 70 } } or nil end,
        GetGroupDisplayInfoByTreeID = function() return GROUPS end,
        GetTreeNodes = function(treeID)
            local ids = {}
            if treeID == 70 then
                for id in pairs(NODES) do ids[#ids + 1] = id end
                table.sort(ids, function(a, b) return a > b end)
            end
            return ids
        end,
        GetNodeInfo = function(configID, nodeID)
            local node = NODES[nodeID]
            if configID ~= 7 or not node then return nil end
            local edges = {}
            for _, target in ipairs(node[7] or {}) do edges[#edges + 1] = { targetNode = target, isActive = true } end
            return {
                ID = nodeID, groupIDs = { 5, node[1] }, posX = node[2], posY = node[3],
                activeRank = node[4], currentRank = node[4], ranksPurchased = node[4], maxRanks = node[5],
                entryIDs = node[6], visibleEdges = edges, isVisible = node[8] ~= false,
                activeEntry = node[9] and { entryID = node[9], rank = node[4] } or nil,
            }
        end,
        GetEntryInfo = function(_, entryID) return DEFINITIONS[entryID] and { definitionID = entryID, maxRanks = 5 } or nil end,
        GetDefinitionInfo = function(definitionID) return DEFINITIONS[definitionID] end,
        GetTreeCurrencyInfo = function() return { { traitCurrencyID = 1, quantity = 3, spent = 8 } } end,
    }
end

-- Skills
local SKILLS = {
    { "Métiers", true }, { "Couture", false, nil, 50, nil, 0, 75 }, { "Compétences d'armes", true }, { "Bâtons", false, nil, 45, nil, 0, 60 },
}
function GetNumSkillLines() return #SKILLS end
function GetSkillLineInfo(index) return unpack(SKILLS[index]) end

-- Spellbook (classic API)
BOOKTYPE_SPELL, BOOKTYPE_PET = "spell", "pet"
local BOOK = { { "Trait de l'ombre", "Rang 1" }, { "Trait de l'ombre", "Rang 2" }, { "Corruption", "Rang 1" }, { "Armure démoniaque", "Rang 1" } }
local PET_BOOK = { { "Eclair de feu", "Rang 2" }, { "Bouclier de feu", "Rang 1" } }
function GetNumSpellTabs() return 2 end
function GetSpellTabInfo(tab) return ({ "Général", "Affliction" })[tab], "icon", (tab - 1) * 2, 2 end
function GetSpellBookItemName(index, book)
    local entry = (book == "pet" and PET_BOOK or BOOK)[index]
    if entry then return entry[1], entry[2] end
end
function GetSpellBookItemInfo(index) return "SPELL", 680 + index end
function IsPassiveSpell() return false end
function GetSpellDescription(id) return id == 9999 and "" or ("Description du sort " .. id .. ".") end
function GetSpellPowerCost() return { { cost = 25, name = "MANA" } } end
function GetSpellInfo(id) return "Sort " .. id, "Rang 1", "icon", 2500, 0, 30, id end
function GetSpellBaseCooldown(id) return id == 683 and 8000 or 0 end
function HasPetSpells() return #PET_BOOK, "DEMON" end

-- Macros
MAX_ACCOUNT_MACROS = 120
function GetNumMacros() return 1, 1 end
function GetMacroInfo(index) return "Macro" .. index, "icon", "#showtooltip\n/cast Trait de l'ombre" end
function GetMacroIndexByName() return 0 end
function CreateMacro(name, icon, body, perCharacter)
    table.insert(createdMacros, { name = name, body = body, perCharacter = perCharacter })
    return 121
end
function EditMacro() return 1 end

-- Action bars
function HasAction(slot) return slot <= 3 or slot == 61 end
function GetActionInfo(slot)
    if slot == 1 then return "spell", 682 end
    if slot == 2 then return "item", 1433 end
    if slot == 3 then return "macro", 1 end
    return "spell", 683
end
function GetActionText() return nil end
function GetBindingKey(command)
    return ({ ACTIONBUTTON1 = "1", ACTIONBUTTON2 = "2", ["CLICK BT4Button61:Keybind"] = "SHIFT-1" })[command]
end
function GetActionBarPage() return 1 end

-- Pet
function UnitCreatureFamily() return "Diablotin" end
NUM_PET_ACTION_SLOTS = 10
PET_ACTION_ATTACK = "Attaquer"
function GetPetActionInfo(index)
    if index == 1 then return "PET_ACTION_ATTACK", "icon", true, false, false, false end
    if index == 4 then return "Eclair de feu", "icon", false, false, true, true end
end

-- Reputation, auras, group
FACTION_STANDING_LABEL5 = "Amical"
function GetNumFactions() return 2 end
function GetFactionInfo(index)
    if index == 1 then return "Horde", "", 0, 0, 0, 0, false, false, true, false, false end
    return "Fossoyeuse", "", 5, 0, 3000, 1200, false, false, false, false, true
end
function UnitBuff(_, index) if index == 1 then return "Armure démoniaque" end end
function GetNumGroupMembers() return 0 end
function IsInRaid() return false end

-- Professions windows
function GetTradeSkillLine() return "Couture", 50, 75 end
function GetNumTradeSkills() return 3 end
function GetTradeSkillInfo(index)
    if index == 1 then return "Tissus", "header" end
    if index == 2 then return "Chemise en lin", "trivial", 2 end
    return "Bottes en lin", "optimal", 0
end

-- Run the addon like the client does: Inbox.lua then Mentor.lua with (addonName, namespace)
local namespace = {}
assert(loadfile(ADDON_DIR .. "/Inbox.lua"))("Mentor", namespace)
MentorInbox = {
    id = "test-1",
    character = "",
    plan = { "Tuer 10 rats" },
    notes = {},
    macros = { { name = "Trait", body = "#showtooltip\n/cast Trait de l'ombre", icon = "", perCharacter = false } },
}
assert(loadfile(ADDON_DIR .. "/Mentor.lua"))("Mentor", namespace)

FireEvent("ADDON_LOADED", "Mentor")
FireEvent("PLAYER_ENTERING_WORLD")
FireEvent("TRADE_SKILL_SHOW")
FireEvent("BANKFRAME_OPENED")
FireEvent("BANKFRAME_CLOSED")
FireEvent("PLAYER_LOGOUT")

-- Serialize like WoW SavedVariables
local function serialize(value, indent)
    indent = indent or ""
    if type(value) == "table" then
        local parts = { "{\n" }
        local count = #value
        for index = 1, count do
            parts[#parts + 1] = indent .. "\t" .. serialize(value[index], indent .. "\t") .. ", -- [" .. index .. "]\n"
        end
        local keys = {}
        for key in pairs(value) do
            if not (type(key) == "number" and key >= 1 and key <= count and key % 1 == 0) then
                keys[#keys + 1] = key
            end
        end
        table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
        for _, key in ipairs(keys) do
            local label = type(key) == "string" and string.format("[%q]", key) or "[" .. tostring(key) .. "]"
            parts[#parts + 1] = indent .. "\t" .. label .. " = " .. serialize(value[key], indent .. "\t") .. ",\n"
        end
        parts[#parts + 1] = indent .. "}"
        return table.concat(parts)
    elseif type(value) == "string" then
        return string.format("%q", value)
    end
    return tostring(value)
end

local handle = assert(io.open(OUTPUT_FILE, "w"))
handle:write("\nMentorDB = " .. serialize(MentorDB) .. "\n")
handle:close()
