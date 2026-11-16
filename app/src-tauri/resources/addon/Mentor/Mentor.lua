local ADDON_NAME = ...
-- Version of this code. The game reads the .toc only when it starts, so after an update
-- the .toc version stays old until the game is restarted, while /reload runs the new code.
local CODE_VERSION = "1.5.1"

local PREFIX = "|cffe3b448Mentor|r : "
local TRACKER_WIDTH = 320
local SNAPSHOT_DELAY = 3

local EQUIPMENT_SLOTS = {
    { id = 1, name = "Head" },
    { id = 2, name = "Neck" },
    { id = 3, name = "Shoulder" },
    { id = 4, name = "Shirt" },
    { id = 5, name = "Chest" },
    { id = 6, name = "Waist" },
    { id = 7, name = "Legs" },
    { id = 8, name = "Feet" },
    { id = 9, name = "Wrist" },
    { id = 10, name = "Hands" },
    { id = 11, name = "Finger1" },
    { id = 12, name = "Finger2" },
    { id = 13, name = "Trinket1" },
    { id = 14, name = "Trinket2" },
    { id = 15, name = "Back" },
    { id = 16, name = "MainHand" },
    { id = 17, name = "OffHand" },
    { id = 18, name = "Ranged" },
    { id = 19, name = "Tabard" },
}

local STATS = {
    { index = 1, name = "Strength" },
    { index = 2, name = "Agility" },
    { index = 3, name = "Stamina" },
    { index = 4, name = "Intellect" },
    { index = 5, name = "Spirit" },
}

local PROFESSION_LABELS = {
    "Primary 1",
    "Primary 2",
    "Archaeology",
    "Fishing",
    "Cooking",
}

local SNAPSHOT_EVENTS = {
    "BAG_UPDATE_DELAYED",
    "PLAYER_EQUIPMENT_CHANGED",
    "QUEST_LOG_UPDATE",
    "PLAYER_MONEY",
    "PLAYER_LEVEL_UP",
    "PLAYER_XP_UPDATE",
    "ZONE_CHANGED_NEW_AREA",
    "SKILL_LINES_CHANGED",
    "PLAYER_REGEN_ENABLED",
    "CHARACTER_POINTS_CHANGED",
    "PLAYER_TALENT_UPDATE",
    "TRAIT_CONFIG_UPDATED",
    "LEARNED_SPELL_IN_TAB",
    "UPDATE_MACROS",
    "SPELLS_CHANGED",
    "ACTIONBAR_SLOT_CHANGED",
    "ACTIONBAR_PAGE_CHANGED",
    "UPDATE_BINDINGS",
    "UNIT_PET",
    "PET_BAR_UPDATE",
    "UPDATE_FACTION",
    "GROUP_ROSTER_UPDATE",
    "TRADE_SKILL_SHOW",
    "TRADE_SKILL_UPDATE",
    "TRADE_SKILL_LIST_UPDATE",
    "CRAFT_SHOW",
    "CRAFT_UPDATE",
    "BANKFRAME_OPENED",
    "BANKFRAME_CLOSED",
    "PLAYERBANKSLOTS_CHANGED",
}

local exportWindow
local planWindow
local tracker
local trackerRows = {}
local snapshotPending = false

local function Print(message)
    print(PREFIX .. message)
end

local function ReturnIfSuccess(ok, ...)
    if not ok then
        return nil
    end
    return ...
end

local function SafeCall(func, ...)
    if type(func) ~= "function" then
        return nil
    end
    return ReturnIfSuccess(pcall(func, ...))
end

local function IsUsable(value)
    if value == nil then
        return false
    end
    if type(issecretvalue) == "function" and issecretvalue(value) then
        return false
    end
    return true
end

local function Clean(value)
    if IsUsable(value) then
        return value
    end
    return nil
end

local function ToText(value, fallback)
    if IsUsable(value) then
        return tostring(value)
    end
    return fallback or "unknown"
end

local function AddLine(lines, text)
    lines[#lines + 1] = text
end

local function EnsureStorage()
    MentorDB = MentorDB or {}
    MentorDB.snapshots = MentorDB.snapshots or {}
    MentorCharDB = MentorCharDB or {}
    MentorCharDB.plan = MentorCharDB.plan or {}
end

local function StripCodes(text)
    if type(text) ~= "string" then
        return nil
    end
    local stripped = text:gsub("|c%x%x%x%x%x%x%x%x", "")
    stripped = stripped:gsub("|cn[^:]*:", "")
    stripped = stripped:gsub("|r", "")
    stripped = stripped:gsub("|T.-|t", "")
    stripped = stripped:gsub("|A.-|a", "")
    stripped = stripped:gsub("|H.-|h(.-)|h", "%1")
    stripped = stripped:gsub("|n", " ")
    stripped = stripped:gsub("^%s+", "")
    stripped = stripped:gsub("%s+$", "")
    return stripped
end

local function ExtractItemName(link)
    if not IsUsable(link) or type(link) ~= "string" then
        return nil
    end
    return link:match("%[(.-)%]")
end

local function FormatMoney(copper)
    if not IsUsable(copper) then
        return "unknown"
    end
    local gold = math.floor(copper / 10000)
    local silver = math.floor((copper % 10000) / 100)
    local remainingCopper = copper % 100
    return string.format("%dg %ds %dc", gold, silver, remainingCopper)
end

local function GetAddonVersion()
    if C_AddOns and C_AddOns.GetAddOnMetadata then
        return ToText(SafeCall(C_AddOns.GetAddOnMetadata, ADDON_NAME, "Version"))
    end
    return "unknown"
end

local function GetCharacterKey()
    local name = SafeCall(UnitName, "player")
    local realm = SafeCall(GetRealmName)
    return ToText(name) .. "-" .. ToText(realm)
end

local function ExtractTooltipLines(tooltipData)
    if not IsUsable(tooltipData) or type(tooltipData) ~= "table" or type(tooltipData.lines) ~= "table" then
        return nil
    end
    local result = {}
    for index, line in ipairs(tooltipData.lines) do
        if index > 1 and type(line) == "table" then
            local left = StripCodes(Clean(line.leftText))
            local right = StripCodes(Clean(line.rightText))
            local text = left
            if left and left ~= "" and right and right ~= "" then
                text = left .. " / " .. right
            elseif (not left or left == "") and right and right ~= "" then
                text = right
            end
            if text and text ~= "" then
                result[#result + 1] = text
            end
        end
    end
    return result
end

local function GetTooltipLines(getter, ...)
    if type(getter) ~= "function" then
        return nil
    end
    return ExtractTooltipLines(SafeCall(getter, ...))
end

local function GetItemLevelAt(location)
    if not location or not C_Item or not C_Item.GetCurrentItemLevel then
        return nil
    end
    if not SafeCall(C_Item.DoesItemExist, location) then
        return nil
    end
    return Clean(SafeCall(C_Item.GetCurrentItemLevel, location))
end

local function DescribeItem(itemID, link)
    local item = {
        itemID = Clean(itemID),
        name = ExtractItemName(link),
        link = Clean(link),
    }
    local query = item.link or item.itemID
    if C_Item and C_Item.GetItemInfo and query then
        local name, _, quality, _, minLevel, itemType, itemSubType, _, equipLoc, _, sellPrice = SafeCall(C_Item.GetItemInfo, query)
        item.name = item.name or Clean(name)
        item.quality = Clean(quality)
        item.requiredLevel = Clean(minLevel)
        item.itemType = Clean(itemType)
        item.itemSubType = Clean(itemSubType)
        item.equipLoc = Clean(equipLoc)
        item.sellPrice = Clean(sellPrice)
    end
    return item
end

local function CollectMeta(reason)
    local version, build, _, interfaceVersion = SafeCall(GetBuildInfo)
    return {
        updatedAt = time(),
        updatedAtText = date("%Y-%m-%d %H:%M:%S"),
        reason = reason,
        client = Clean(version),
        build = Clean(build),
        interface = Clean(interfaceVersion),
        addonVersion = CODE_VERSION,
        tocVersion = GetAddonVersion(),
    }
end

local function CollectCharacter()
    local name = SafeCall(UnitName, "player")
    local realm = SafeCall(GetRealmName)
    local className = SafeCall(UnitClass, "player")
    local raceName = SafeCall(UnitRace, "player")
    local faction = SafeCall(UnitFactionGroup, "player")
    local level = SafeCall(UnitLevel, "player")
    local currentXp = SafeCall(UnitXP, "player")
    local maxXp = SafeCall(UnitXPMax, "player")
    local restedXp = SafeCall(GetXPExhaustion)
    local money = SafeCall(GetMoney)
    return {
        key = GetCharacterKey(),
        name = Clean(name),
        realm = Clean(realm),
        level = Clean(level),
        race = Clean(raceName),
        class = Clean(className),
        faction = Clean(faction),
        xp = Clean(currentXp),
        xpMax = Clean(maxXp),
        restedXp = Clean(restedXp),
        money = Clean(money),
        moneyText = FormatMoney(money),
    }
end

local function CollectLocation()
    local zone = SafeCall(GetZoneText)
    local subZone = SafeCall(GetSubZoneText)
    local location = {
        zone = Clean(zone),
        subZone = Clean(subZone),
    }
    if location.subZone == "" then
        location.subZone = nil
    end
    if C_Map then
        local mapID = Clean(SafeCall(C_Map.GetBestMapForUnit, "player"))
        location.mapID = mapID
        if mapID then
            local mapInfo = SafeCall(C_Map.GetMapInfo, mapID)
            if type(mapInfo) == "table" then
                location.mapName = Clean(mapInfo.name)
            end
            local position = SafeCall(C_Map.GetPlayerMapPosition, mapID, "player")
            if IsUsable(position) and type(position) == "table" then
                local x, y = SafeCall(position.GetXY, position)
                x = Clean(x)
                y = Clean(y)
                if x and y then
                    location.x = math.floor(x * 1000 + 0.5) / 10
                    location.y = math.floor(y * 1000 + 0.5) / 10
                end
            end
        end
    end
    return location
end

local function Round(value, decimals)
    value = Clean(value)
    if type(value) ~= "number" then
        return nil
    end
    local factor = 10 ^ (decimals or 2)
    return math.floor(value * factor + 0.5) / factor
end

local function SpellName(spellID)
    if not Clean(spellID) then
        return nil
    end
    if C_Spell and C_Spell.GetSpellName then
        local name = Clean(SafeCall(C_Spell.GetSpellName, spellID))
        if name then
            return name
        end
    end
    return Clean((SafeCall(GetSpellInfo, spellID)))
end

local function ItemName(itemID)
    if not Clean(itemID) then
        return nil
    end
    if C_Item and C_Item.GetItemNameByID then
        local name = Clean(SafeCall(C_Item.GetItemNameByID, itemID))
        if name then
            return name
        end
    end
    if C_Item and C_Item.GetItemInfo then
        return Clean((SafeCall(C_Item.GetItemInfo, itemID)))
    end
    return Clean((SafeCall(GetItemInfo, itemID)))
end

local scanTooltip

-- Reads a tooltip through a hidden GameTooltip (works on clients without C_TooltipInfo).
local function ScanTooltip(method, ...)
    if not scanTooltip then
        local ok, frame = pcall(CreateFrame, "GameTooltip", "MentorScanTooltip", nil, "GameTooltipTemplate")
        if not ok or not frame then
            return nil
        end
        scanTooltip = frame
    end
    scanTooltip:SetOwner(WorldFrame, "ANCHOR_NONE")
    scanTooltip:ClearLines()
    local setter = scanTooltip[method]
    if type(setter) ~= "function" or not pcall(setter, scanTooltip, ...) then
        scanTooltip:Hide()
        return nil
    end
    local lines = {}
    for index = 2, scanTooltip:NumLines() do
        local left = _G["MentorScanTooltipTextLeft" .. index]
        local right = _G["MentorScanTooltipTextRight" .. index]
        local text = left and StripCodes(Clean(left:GetText()))
        local rightText = right and right:IsShown() and StripCodes(Clean(right:GetText()))
        if text and text ~= "" and rightText and rightText ~= "" then
            text = text .. " / " .. rightText
        elseif (not text or text == "") and rightText and rightText ~= "" then
            text = rightText
        end
        if text and text ~= "" then
            lines[#lines + 1] = text
        end
    end
    scanTooltip:Hide()
    return lines
end

local RESISTANCES = { { index = 2, name = "Fire" }, { index = 3, name = "Nature" }, { index = 4, name = "Frost" }, { index = 5, name = "Shadow" }, { index = 6, name = "Arcane" } }

local function SumAttackPower(base, positive, negative)
    base = Clean(base)
    if type(base) ~= "number" then
        return nil
    end
    return base + (Clean(positive) or 0) + (Clean(negative) or 0)
end

local function CollectStats()
    local maxHealth = SafeCall(UnitHealthMax, "player")
    local maxPower = SafeCall(UnitPowerMax, "player")
    local stats = {
        maxHealth = Clean(maxHealth),
        maxPower = Clean(maxPower),
        primary = {},
    }
    for _, stat in ipairs(STATS) do
        local _, effective = SafeCall(UnitStat, "player", stat.index)
        effective = Clean(effective)
        if effective then
            stats.primary[#stats.primary + 1] = { name = stat.name, value = effective }
        end
    end
    local _, effectiveArmor = SafeCall(UnitArmor, "player")
    stats.armor = Clean(effectiveArmor)
    stats.attackPower = SumAttackPower(SafeCall(UnitAttackPower, "player"))
    stats.rangedAttackPower = SumAttackPower(SafeCall(UnitRangedAttackPower, "player"))
    stats.meleeCrit = Round(SafeCall(GetCritChance))
    stats.rangedCrit = Round(SafeCall(GetRangedCritChance))
    stats.meleeHit = Round(SafeCall(GetHitModifier))
    stats.spellHit = Round(SafeCall(GetSpellHitModifier))
    stats.dodge = Round(SafeCall(GetDodgeChance))
    stats.parry = Round(SafeCall(GetParryChance))
    stats.block = Round(SafeCall(GetBlockChance))
    stats.healingBonus = Clean(SafeCall(GetSpellBonusHealing))
    local spellCrit, spellPower
    for school = 2, 7 do
        local crit = Clean(SafeCall(GetSpellCritChance, school))
        local power = Clean(SafeCall(GetSpellBonusDamage, school))
        if type(crit) == "number" and (not spellCrit or crit > spellCrit) then
            spellCrit = crit
        end
        if type(power) == "number" and (not spellPower or power > spellPower) then
            spellPower = power
        end
    end
    stats.spellCrit = Round(spellCrit)
    stats.spellPower = spellPower
    local baseDefense, defenseModifier = SafeCall(UnitDefense, "player")
    if Clean(baseDefense) then
        stats.defense = Clean(baseDefense) + (Clean(defenseModifier) or 0)
    end
    local baseRegen, castingRegen = SafeCall(GetManaRegen)
    stats.manaRegen = Round(baseRegen)
    stats.manaRegenCasting = Round(castingRegen)
    local mainSpeed, offSpeed = SafeCall(UnitAttackSpeed, "player")
    stats.mainHandSpeed = Round(mainSpeed)
    stats.offHandSpeed = Round(offSpeed)
    local minDamage, maxDamage = SafeCall(UnitDamage, "player")
    stats.mainHandDamage = Clean(minDamage) and Clean(maxDamage) and (math.floor(minDamage) .. "-" .. math.floor(maxDamage)) or nil
    stats.resistances = {}
    for _, resistance in ipairs(RESISTANCES) do
        local _, total = SafeCall(UnitResistance, "player", resistance.index)
        total = Clean(total)
        if type(total) == "number" and total > 0 then
            stats.resistances[#stats.resistances + 1] = { name = resistance.name, value = total }
        end
    end
    return stats
end

local function CollectEquipment()
    local equipment = {}
    for _, slot in ipairs(EQUIPMENT_SLOTS) do
        local entry = { slotId = slot.id, slot = slot.name }
        local link = Clean(SafeCall(GetInventoryItemLink, "player", slot.id))
        if link then
            local itemID = SafeCall(GetInventoryItemID, "player", slot.id)
            local item = DescribeItem(itemID, link)
            local location = ItemLocation and SafeCall(ItemLocation.CreateFromEquipmentSlot, ItemLocation, slot.id)
            item.itemLevel = GetItemLevelAt(location)
            item.tooltip = GetTooltipLines(C_TooltipInfo and C_TooltipInfo.GetInventoryItem, "player", slot.id)
            entry.item = item
        end
        equipment[#equipment + 1] = entry
    end
    return equipment
end

local function IsEquippable(item)
    return type(item.equipLoc) == "string" and item.equipLoc ~= "" and not item.equipLoc:find("NON_EQUIP")
end

local function ContainerItemTooltip(bag, slot)
    local lines = GetTooltipLines(C_TooltipInfo and C_TooltipInfo.GetBagItem, bag, slot)
    if lines then
        return lines
    end
    return ScanTooltip("SetBagItem", bag, slot)
end

-- Adds one bag (or bank container) to `target`; tooltips only for equippable items when `equippableTooltipsOnly`.
local function CollectContainer(bag, target, equippableTooltipsOnly)
    if not C_Container then
        return
    end
    local numSlots = Clean(SafeCall(C_Container.GetContainerNumSlots, bag)) or 0
    target.totalSlots = target.totalSlots + numSlots
    for slot = 1, numSlots do
        local info = SafeCall(C_Container.GetContainerItemInfo, bag, slot)
        if IsUsable(info) and type(info) == "table" and Clean(info.hyperlink) then
            local item = DescribeItem(info.itemID, info.hyperlink)
            item.bag = bag
            item.slot = slot
            item.count = Clean(info.stackCount) or 1
            item.quality = item.quality or Clean(info.quality)
            item.isBound = Clean(info.isBound)
            local location = ItemLocation and SafeCall(ItemLocation.CreateFromBagAndSlot, ItemLocation, bag, slot)
            item.itemLevel = GetItemLevelAt(location)
            if not equippableTooltipsOnly or IsEquippable(item) then
                item.tooltip = ContainerItemTooltip(bag, slot)
            end
            target.items[#target.items + 1] = item
        else
            target.freeSlots = target.freeSlots + 1
        end
    end
end

local function CollectBags()
    local bags = { freeSlots = 0, totalSlots = 0, items = {} }
    local lastBag = (NUM_BAG_SLOTS or 4) + (NUM_REAGENTBAG_SLOTS or 0)
    for bag = 0, lastBag do
        CollectContainer(bag, bags, false)
    end
    return bags
end

local bankOpen = false

-- The bank can only be read while it is open: the result is kept per character.
local function CacheBank()
    EnsureStorage()
    local bank = { updated = time(), updatedText = date("%Y-%m-%d %H:%M"), freeSlots = 0, totalSlots = 0, items = {} }
    CollectContainer(BANK_CONTAINER or -1, bank, true)
    local first = (NUM_BAG_SLOTS or 4) + (NUM_REAGENTBAG_SLOTS or 0) + 1
    for bag = first, first + (NUM_BANKBAGSLOTS or 7) - 1 do
        CollectContainer(bag, bank, true)
    end
    if bank.totalSlots > 0 then
        MentorCharDB.bank = bank
    end
end

local function CollectProfessions()
    local professions = {}
    if type(GetProfessions) ~= "function" then
        return professions
    end
    local first, second, archaeology, fishing, cooking = SafeCall(GetProfessions)
    local professionIndexes = { first, second, archaeology, fishing, cooking }
    for position = 1, #PROFESSION_LABELS do
        local professionIndex = Clean(professionIndexes[position])
        if professionIndex then
            local name, _, skillLevel, maxSkillLevel = SafeCall(GetProfessionInfo, professionIndex)
            name = Clean(name)
            if name then
                professions[#professions + 1] = {
                    label = PROFESSION_LABELS[position],
                    name = name,
                    skill = Clean(skillLevel),
                    maxSkill = Clean(maxSkillLevel),
                }
            end
        end
    end
    return professions
end

local function CollectQuests()
    local quests = {}
    if not C_QuestLog or not C_QuestLog.GetNumQuestLogEntries then
        return quests
    end
    local numEntries = Clean(SafeCall(C_QuestLog.GetNumQuestLogEntries)) or 0
    for index = 1, numEntries do
        local info = SafeCall(C_QuestLog.GetInfo, index)
        if IsUsable(info) and type(info) == "table" and not info.isHeader and not info.isHidden then
            local quest = {
                questID = Clean(info.questID),
                title = Clean(info.title),
                level = Clean(info.level),
                complete = SafeCall(C_QuestLog.IsComplete, info.questID) and true or false,
                objectives = {},
            }
            local objectives = SafeCall(C_QuestLog.GetQuestObjectives, info.questID)
            if IsUsable(objectives) and type(objectives) == "table" then
                for _, objective in ipairs(objectives) do
                    quest.objectives[#quest.objectives + 1] = {
                        text = Clean(objective.text),
                        finished = Clean(objective.finished) and true or false,
                    }
                end
            end
            quests[#quests + 1] = quest
        end
    end
    return quests
end

-- One axis of a talent grid, from the node positions: the first line, the step between
-- two lines and the last regular line. Some nodes sit a few units off their line, so a gap
-- under a quarter of the median gap is the same line.
local function GridAxis(values)
    table.sort(values)
    local gaps = {}
    for index = 2, #values do
        local gap = values[index] - values[index - 1]
        if gap > 0 then
            gaps[#gaps + 1] = gap
        end
    end
    local first = values[1] or 0
    if #gaps == 0 then
        return first, 1, first
    end
    table.sort(gaps)
    local least = gaps[math.floor(#gaps / 2) + 1] / 4
    local step = gaps[#gaps]
    for _, gap in ipairs(gaps) do
        if gap > least then
            step = gap
            break
        end
    end
    local last = first
    for index = 2, #values do
        if values[index] - values[index - 1] > 4 * step then
            break
        end
        last = values[index]
    end
    return first, step, last
end

-- Line number (from 1) of a position on one axis. A few nodes of the game have a position
-- ten times too big: they are read at a tenth when that falls on the grid.
local function GridLine(value, first, step, last)
    if value > last + 4 * step then
        local tenth = value / 10
        local position = (tenth - first) / step
        local nearest = math.floor(position + 0.5)
        if nearest >= 0 and tenth <= last + step / 4 and math.abs(position - nearest) < 0.25 then
            value = tenth
        end
    end
    return math.floor((value - first) / step + 0.5) + 1
end

local function TraitDefinition(configID, entryID)
    local entry = Clean(entryID) and SafeCall(C_Traits.GetEntryInfo, configID, entryID)
    local definitionID = type(entry) == "table" and Clean(entry.definitionID)
    local definition = definitionID and SafeCall(C_Traits.GetDefinitionInfo, definitionID)
    return type(definition) == "table" and definition or nil
end

local function TraitName(definition)
    local name = Clean(definition.overrideName)
    if type(name) == "string" and name ~= "" then
        return name
    end
    return SpellName(definition.spellID) or SpellName(definition.overriddenSpellID)
end

-- Text of a talent, or nil while the game has not loaded it yet.
local function TraitDescription(definition)
    local text = StripCodes(Clean(definition.overrideDescription))
    if text and text ~= "" then
        return text
    end
    local spellID = Clean(definition.spellID)
    if not spellID then
        return nil
    end
    if C_Spell and C_Spell.GetSpellDescription then
        text = SafeCall(C_Spell.GetSpellDescription, spellID)
    else
        text = SafeCall(GetSpellDescription, spellID)
    end
    text = StripCodes(Clean(text))
    if text and text ~= "" then
        return text
    end
    local lines = ScanTooltip("SetSpellByID", spellID)
    if lines and #lines > 0 then
        return table.concat(lines, " ")
    end
    if C_Spell and C_Spell.RequestLoadSpellData then
        SafeCall(C_Spell.RequestLoadSpellData, spellID)
    end
    return nil
end

-- WoW Forever keeps the three talent trees of a class as the groups of one trait tree
-- (C_Traits, as in the modern game). Fills talents.trees; returns false when this client
-- has no such tree.
local function CollectTraitTrees(talents)
    -- What the game answered at each step, kept in the sheet to understand a failure.
    local trace = {}
    talents.trace = trace
    if not (C_ClassTalents and C_ClassTalents.GetActiveConfigID and C_Traits and C_Traits.GetConfigInfo and C_Traits.GetTreeNodes and C_Traits.GetNodeInfo) then
        trace[#trace + 1] = "trait functions missing"
        return false
    end
    local configID = Clean(SafeCall(C_ClassTalents.GetActiveConfigID))
    trace[#trace + 1] = "config " .. ToText(configID, "none")
    local config = configID and SafeCall(C_Traits.GetConfigInfo, configID)
    local treeID = type(config) == "table" and type(config.treeIDs) == "table" and Clean(config.treeIDs[1])
    trace[#trace + 1] = "tree " .. ToText(treeID or nil, "none")
    local nodeIDs = treeID and SafeCall(C_Traits.GetTreeNodes, treeID)
    trace[#trace + 1] = "nodes " .. (type(nodeIDs) == "table" and #nodeIDs or "none")
    if type(nodeIDs) ~= "table" then
        talents.problem = configID and "talent tree not available yet" or "no active talent configuration"
        return false
    end

    local trees, byGroup = {}, {}
    local groups = SafeCall(C_Traits.GetGroupDisplayInfoByTreeID, treeID)
    if type(groups) == "table" then
        table.sort(groups, function(left, right)
            return (left.orderIndex or 0) < (right.orderIndex or 0)
        end)
        for _, group in ipairs(groups) do
            local tree = { name = Clean(group.displayName) or ("Tree " .. (#trees + 1)), points = 0, talents = {}, nodes = {} }
            trees[#trees + 1] = tree
            byGroup[group.groupID] = tree
        end
    end
    local ungrouped
    for _, nodeID in ipairs(nodeIDs) do
        local info = SafeCall(C_Traits.GetNodeInfo, configID, nodeID)
        if type(info) == "table" and info.isVisible and type(info.entryIDs) == "table" and info.entryIDs[1] then
            local tree
            for _, groupID in ipairs(type(info.groupIDs) == "table" and info.groupIDs or {}) do
                tree = byGroup[groupID]
                if tree then
                    break
                end
            end
            if not tree then
                ungrouped = ungrouped or { name = "Talents", points = 0, talents = {}, nodes = {} }
                tree = ungrouped
            end
            tree.nodes[#tree.nodes + 1] = { id = nodeID, info = info }
        end
    end
    if ungrouped then
        trees[#trees + 1] = ungrouped
    end
    local visible = 0
    for _, tree in ipairs(trees) do
        visible = visible + #tree.nodes
    end
    trace[#trace + 1] = "groups " .. (type(groups) == "table" and #groups or "none") .. ", visible talents " .. visible

    local byNode = {}
    for _, tree in ipairs(trees) do
        local columns, rows = {}, {}
        for index, node in ipairs(tree.nodes) do
            columns[index] = Clean(node.info.posX) or 0
            rows[index] = Clean(node.info.posY) or 0
        end
        local firstColumn, columnStep, lastColumn = GridAxis(columns)
        local firstRow, rowStep, lastRow = GridAxis(rows)
        for _, node in ipairs(tree.nodes) do
            local info = node.info
            local activeEntry = type(info.activeEntry) == "table" and Clean(info.activeEntry.entryID)
            local definition = TraitDefinition(configID, activeEntry or info.entryIDs[1])
            local name = definition and TraitName(definition)
            if name then
                local rank = Clean(info.activeRank) or Clean(info.currentRank) or Clean(info.ranksPurchased) or 0
                local talent = {
                    name = name,
                    rank = rank,
                    maxRank = Clean(info.maxRanks),
                    tier = GridLine(Clean(info.posY) or 0, firstRow, rowStep, lastRow),
                    column = GridLine(Clean(info.posX) or 0, firstColumn, columnStep, lastColumn),
                    spellID = Clean(definition.spellID),
                }
                talent.description = TraitDescription(definition)
                if not talent.description then
                    talents.incomplete = true
                end
                if #info.entryIDs > 1 then
                    -- A node where the player picks one talent among several.
                    talent.choices = {}
                    for _, entryID in ipairs(info.entryIDs) do
                        local option = TraitDefinition(configID, entryID)
                        local optionName = option and TraitName(option)
                        if optionName then
                            talent.choices[#talent.choices + 1] = optionName
                        end
                    end
                    talent.chosen = (activeEntry and rank > 0) and true or false
                end
                byNode[node.id] = talent
                tree.talents[#tree.talents + 1] = talent
                tree.points = tree.points + rank
            end
        end
    end
    -- An edge goes from a talent to the talent it unlocks.
    for _, tree in ipairs(trees) do
        for _, node in ipairs(tree.nodes) do
            local from = byNode[node.id]
            for _, edge in ipairs(type(node.info.visibleEdges) == "table" and node.info.visibleEdges or {}) do
                local to = byNode[edge.targetNode]
                if from and to then
                    to.requires = { tier = from.tier, column = from.column, name = from.name }
                end
            end
        end
        tree.nodes = nil
        table.sort(tree.talents, function(left, right)
            if left.tier ~= right.tier then
                return left.tier < right.tier
            end
            return left.column < right.column
        end)
        if #tree.talents > 0 then
            talents.trees[#talents.trees + 1] = tree
        end
    end
    if #talents.trees == 0 then
        talents.problem = "the talent tree has no readable talent"
        return false
    end
    local currencies = SafeCall(C_Traits.GetTreeCurrencyInfo, configID, treeID, false)
    local currency = type(currencies) == "table" and currencies[1]
    if type(currency) == "table" and Clean(currency.quantity) then
        talents.unspent = currency.quantity
    end
    talents.source = "traits"
    talents.problem = nil
    talents.trace = nil
    return true
end

local function CollectTalents()
    local talents = { trees = {} }
    local specialization = C_SpecializationInfo or {}
    local getSpecialization = specialization.GetSpecialization or GetSpecialization
    local getSpecializationInfo = specialization.GetSpecializationInfo or GetSpecializationInfo
    if type(getSpecialization) == "function" and type(getSpecializationInfo) == "function" then
        local specIndex = Clean(SafeCall(getSpecialization))
        if specIndex then
            local _, specName, _, _, role = SafeCall(getSpecializationInfo, specIndex)
            talents.spec = Clean(specName)
            talents.role = Clean(role)
        end
    end
    if C_ClassTalents and C_ClassTalents.GetActiveConfigID and C_Traits and C_Traits.GenerateImportString then
        local configID = Clean(SafeCall(C_ClassTalents.GetActiveConfigID))
        if configID then
            talents.loadoutString = Clean(SafeCall(C_Traits.GenerateImportString, configID))
        end
    end
    if type(UnitCharacterPoints) == "function" then
        talents.unspent = Clean(SafeCall(UnitCharacterPoints, "player"))
    end
    if CollectTraitTrees(talents) then
        return talents
    end
    -- Clients with the original talent functions (Classic).
    if type(GetNumTalentTabs) ~= "function" or type(GetTalentTabInfo) ~= "function" then
        return talents
    end
    local tabCount = Clean(SafeCall(GetNumTalentTabs)) or 0
    for tab = 1, tabCount do
        local info = { SafeCall(GetTalentTabInfo, tab) }
        local tree = { talents = {} }
        if type(info[1]) == "string" then
            tree.name = info[1]
            tree.points = Clean(info[3])
        elseif type(info[2]) == "string" then
            tree.name = info[2]
            tree.points = Clean(info[5])
        end
        if tree.name and type(GetNumTalents) == "function" and type(GetTalentInfo) == "function" then
            local talentCount = Clean(SafeCall(GetNumTalents, tab)) or 0
            for index = 1, talentCount do
                local name, _, tier, column, rank, maxRank = SafeCall(GetTalentInfo, tab, index)
                name = Clean(name)
                if name then
                    local talent = {
                        name = name,
                        rank = Clean(rank) or 0,
                        maxRank = Clean(maxRank),
                        tier = Clean(tier),
                        column = Clean(column),
                    }
                    local requiredTier, requiredColumn = SafeCall(GetTalentPrereqs, tab, index)
                    if Clean(requiredTier) and Clean(requiredColumn) then
                        talent.requires = { tier = requiredTier, column = requiredColumn }
                    end
                    local lines = ScanTooltip("SetTalent", tab, index)
                    if lines and #lines > 0 then
                        talent.description = table.concat(lines, " ")
                    end
                    tree.talents[#tree.talents + 1] = talent
                end
            end
            talents.trees[#talents.trees + 1] = tree
        end
    end
    if #talents.trees > 0 then
        talents.source = "classic"
        talents.problem = nil
    end
    return talents
end

local function CollectSkills()
    local skills = {}
    if type(GetNumSkillLines) ~= "function" or type(GetSkillLineInfo) ~= "function" then
        return skills
    end
    local count = Clean(SafeCall(GetNumSkillLines)) or 0
    local category
    for index = 1, count do
        local name, isHeader, _, rank, _, modifier, maxRank = SafeCall(GetSkillLineInfo, index)
        name = Clean(name)
        if name then
            if isHeader then
                category = name
            else
                skills[#skills + 1] = {
                    category = category,
                    name = name,
                    rank = Clean(rank),
                    modifier = Clean(modifier),
                    maxRank = Clean(maxRank),
                }
            end
        end
    end
    return skills
end

local function SpellDetails(entry, spellID)
    spellID = Clean(spellID)
    if not spellID then
        return
    end
    entry.spellID = spellID
    local description
    if C_Spell and C_Spell.GetSpellDescription then
        description = SafeCall(C_Spell.GetSpellDescription, spellID)
    else
        description = SafeCall(GetSpellDescription, spellID)
    end
    description = StripCodes(Clean(description))
    if description and description ~= "" then
        entry.description = description
    end
    local costs = (C_Spell and C_Spell.GetSpellPowerCost and SafeCall(C_Spell.GetSpellPowerCost, spellID)) or SafeCall(GetSpellPowerCost, spellID)
    if IsUsable(costs) and type(costs) == "table" then
        local parts = {}
        for _, cost in ipairs(costs) do
            if type(cost) == "table" and type(Clean(cost.cost)) == "number" and cost.cost > 0 then
                parts[#parts + 1] = cost.cost .. " " .. ToText(Clean(cost.name), "")
            end
        end
        if #parts > 0 then
            entry.cost = table.concat(parts, ", ")
        end
    end
    if C_Spell and C_Spell.GetSpellInfo then
        local info = SafeCall(C_Spell.GetSpellInfo, spellID)
        if IsUsable(info) and type(info) == "table" then
            entry.castMs = Clean(info.castTime)
            entry.maxRange = Clean(info.maxRange)
        end
    else
        local _, _, _, castTime, _, maxRange = SafeCall(GetSpellInfo, spellID)
        entry.castMs = Clean(castTime)
        entry.maxRange = Clean(maxRange)
    end
    local cooldown = Clean(SafeCall(GetSpellBaseCooldown, spellID))
    if type(cooldown) == "number" and cooldown > 0 then
        entry.cooldownMs = cooldown
    end
end

-- Keeps one entry per spell name: the spellbook lists ranks from lowest to highest.
local function AddSpell(spells, byName, tab, name, rank, spellID, passive, notLearned)
    name = Clean(name)
    if not name or name == "" then
        return
    end
    local entry = { name = name, tab = tab, passive = passive and true or nil, notLearned = notLearned and true or nil }
    rank = Clean(rank)
    if rank and rank ~= "" then
        entry.rank = rank
    end
    SpellDetails(entry, spellID)
    if byName[name] then
        spells[byName[name]] = entry
    else
        spells[#spells + 1] = entry
        byName[name] = #spells
    end
end

local function CollectSpells()
    local spells, byName = {}, {}
    if C_SpellBook and C_SpellBook.GetNumSpellBookSkillLines and C_SpellBook.GetSpellBookSkillLineInfo and C_SpellBook.GetSpellBookItemInfo and Enum and Enum.SpellBookSpellBank then
        local bank = Enum.SpellBookSpellBank.Player
        local futureType = Enum.SpellBookItemType and Enum.SpellBookItemType.FutureSpell
        local lines = Clean(SafeCall(C_SpellBook.GetNumSpellBookSkillLines)) or 0
        for line = 1, lines do
            local info = SafeCall(C_SpellBook.GetSpellBookSkillLineInfo, line)
            if IsUsable(info) and type(info) == "table" and not info.shouldHide then
                local offset = Clean(info.itemIndexOffset) or 0
                for index = offset + 1, offset + (Clean(info.numSpellBookItems) or 0) do
                    local item = SafeCall(C_SpellBook.GetSpellBookItemInfo, index, bank)
                    if IsUsable(item) and type(item) == "table" then
                        AddSpell(spells, byName, Clean(info.name), item.name, item.subName, item.spellID or item.actionID, item.isPassive, futureType and item.itemType == futureType)
                    end
                end
            end
        end
        if #spells > 0 then
            return spells
        end
    end
    if type(GetNumSpellTabs) == "function" and type(GetSpellTabInfo) == "function" and type(GetSpellBookItemName) == "function" then
        local book = BOOKTYPE_SPELL or "spell"
        local tabs = Clean(SafeCall(GetNumSpellTabs)) or 0
        for tab = 1, tabs do
            local tabName, _, offset, count = SafeCall(GetSpellTabInfo, tab)
            offset = Clean(offset) or 0
            for index = offset + 1, offset + (Clean(count) or 0) do
                local name, rank = SafeCall(GetSpellBookItemName, index, book)
                local spellType, spellID = SafeCall(GetSpellBookItemInfo, index, book)
                local passive = SafeCall(IsPassiveSpell, index, book)
                AddSpell(spells, byName, Clean(tabName), name, rank, spellID, passive, spellType == "FUTURESPELL")
            end
        end
    end
    return spells
end

local function SlotBindingKey(slot)
    local commands = {}
    if slot <= 12 then
        commands[1] = "ACTIONBUTTON" .. slot
    elseif slot >= 61 and slot <= 72 then
        commands[1] = "MULTIACTIONBAR1BUTTON" .. (slot - 60)
    elseif slot >= 49 and slot <= 60 then
        commands[1] = "MULTIACTIONBAR2BUTTON" .. (slot - 48)
    elseif slot >= 25 and slot <= 36 then
        commands[1] = "MULTIACTIONBAR3BUTTON" .. (slot - 24)
    elseif slot >= 37 and slot <= 48 then
        commands[1] = "MULTIACTIONBAR4BUTTON" .. (slot - 36)
    end
    commands[#commands + 1] = "CLICK BT4Button" .. slot .. ":Keybind"
    commands[#commands + 1] = "CLICK BT4Button" .. slot .. ":LeftButton"
    for _, command in ipairs(commands) do
        local key = Clean(SafeCall(GetBindingKey, command))
        if key then
            return key
        end
    end
    return nil
end

local function CollectActionBars()
    local bars = { page = Clean(SafeCall(GetActionBarPage)), slots = {} }
    if type(HasAction) ~= "function" or type(GetActionInfo) ~= "function" then
        return bars
    end
    for slot = 1, 180 do
        if SafeCall(HasAction, slot) then
            local actionType, id = SafeCall(GetActionInfo, slot)
            local entry = { slot = slot, type = Clean(actionType), id = Clean(id) }
            if entry.type == "spell" then
                entry.name = SpellName(entry.id)
            elseif entry.type == "item" then
                entry.name = ItemName(entry.id)
            elseif entry.type == "macro" then
                local name, _, body = SafeCall(GetMacroInfo, entry.id)
                entry.name = Clean(name)
                entry.body = Clean(body)
            end
            entry.name = entry.name or Clean(SafeCall(GetActionText, slot))
            entry.key = SlotBindingKey(slot)
            bars.slots[#bars.slots + 1] = entry
        end
    end
    return bars
end

local function CollectPet()
    if not SafeCall(UnitExists, "pet") then
        return nil
    end
    local pet = {
        name = Clean(SafeCall(UnitName, "pet")),
        family = Clean(SafeCall(UnitCreatureFamily, "pet")),
        level = Clean(SafeCall(UnitLevel, "pet")),
        maxHealth = Clean(SafeCall(UnitHealthMax, "pet")),
        happiness = Clean(SafeCall(GetPetHappiness)),
        actions = {},
        spells = {},
    }
    for index = 1, NUM_PET_ACTION_SLOTS or 10 do
        local name, _, isToken, _, autoCastAllowed, autoCastEnabled = SafeCall(GetPetActionInfo, index)
        name = Clean(name)
        if name then
            if isToken and type(_G[name]) == "string" then
                name = _G[name]
            end
            pet.actions[#pet.actions + 1] = { slot = index, name = name, autocast = autoCastAllowed and (autoCastEnabled and "on" or "off") or nil }
        end
    end
    local book = (Enum and Enum.SpellBookSpellBank and Enum.SpellBookSpellBank.Pet) or BOOKTYPE_PET or "pet"
    local count = Clean(SafeCall(C_SpellBook and C_SpellBook.HasPetSpells or HasPetSpells))
    local getName = (C_SpellBook and C_SpellBook.GetSpellBookItemName) or GetSpellBookItemName
    if type(count) == "number" and getName then
        local seen = {}
        for index = 1, count do
            local name, rank = SafeCall(getName, index, book)
            name = Clean(name)
            if name and not seen[name] then
                seen[name] = true
                pet.spells[#pet.spells + 1] = Clean(rank) and rank ~= "" and (name .. " (" .. rank .. ")") or name
            end
        end
    end
    return pet
end

local function CollectReputations()
    local list = {}
    if C_Reputation and C_Reputation.GetNumFactions and C_Reputation.GetFactionDataByIndex then
        for index = 1, Clean(SafeCall(C_Reputation.GetNumFactions)) or 0 do
            local data = SafeCall(C_Reputation.GetFactionDataByIndex, index)
            if IsUsable(data) and type(data) == "table" and Clean(data.name) and (not data.isHeader or data.isHeaderWithRep) then
                list[#list + 1] = {
                    name = data.name,
                    standing = Clean(_G["FACTION_STANDING_LABEL" .. tostring(data.reaction)]),
                    value = Clean(data.currentStanding),
                    min = Clean(data.currentReactionThreshold),
                    max = Clean(data.nextReactionThreshold),
                }
            end
        end
        return list
    end
    for index = 1, Clean(SafeCall(GetNumFactions)) or 0 do
        local name, _, standingID, barMin, barMax, barValue, _, _, isHeader, _, hasRep = SafeCall(GetFactionInfo, index)
        name = Clean(name)
        if name and (not isHeader or hasRep) then
            list[#list + 1] = {
                name = name,
                standing = Clean(_G["FACTION_STANDING_LABEL" .. tostring(standingID)]),
                value = Clean(barValue),
                min = Clean(barMin),
                max = Clean(barMax),
            }
        end
    end
    return list
end

local function CollectAuras()
    local auras = {}
    for index = 1, 40 do
        local name
        if C_UnitAuras and C_UnitAuras.GetAuraDataByIndex then
            local data = SafeCall(C_UnitAuras.GetAuraDataByIndex, "player", index, "HELPFUL")
            if not IsUsable(data) or type(data) ~= "table" then
                break
            end
            name = Clean(data.name)
        else
            name = Clean((SafeCall(UnitBuff, "player", index)))
            if not name then
                break
            end
        end
        if name then
            auras[#auras + 1] = name
        end
    end
    return auras
end

local function UnitSummary(unit)
    if not SafeCall(UnitExists, unit) then
        return nil
    end
    return {
        name = Clean(SafeCall(UnitName, unit)),
        level = Clean(SafeCall(UnitLevel, unit)),
        class = Clean((SafeCall(UnitClass, unit))),
        race = Clean((SafeCall(UnitRace, unit))),
        classification = Clean(SafeCall(UnitClassification, unit)),
        creatureType = Clean(SafeCall(UnitCreatureType, unit)),
        enemy = SafeCall(UnitCanAttack, "player", unit) and true or nil,
    }
end

local function CollectGroup()
    local group = { size = Clean(SafeCall(GetNumGroupMembers)) or 0, raid = SafeCall(IsInRaid) and true or nil, members = {} }
    if group.size > 0 and not group.raid then
        for index = 1, 4 do
            local member = UnitSummary("party" .. index)
            if member then
                group.members[#group.members + 1] = member
            end
        end
    end
    return group
end

-- Recipes are only readable while a profession window is open: kept per character.
local function StoreRecipes(name, rank, maxRank, recipes)
    name = Clean(name)
    if not name or name == "UNKNOWN" or #recipes == 0 then
        return
    end
    EnsureStorage()
    MentorCharDB.recipes = MentorCharDB.recipes or {}
    MentorCharDB.recipes[name] = {
        rank = Clean(rank),
        maxRank = Clean(maxRank),
        updated = time(),
        updatedText = date("%Y-%m-%d %H:%M"),
        recipes = recipes,
    }
end

local function CacheTradeSkill()
    if type(GetTradeSkillLine) == "function" and type(GetNumTradeSkills) == "function" then
        local name, rank, maxRank = SafeCall(GetTradeSkillLine)
        local recipes = {}
        for index = 1, Clean(SafeCall(GetNumTradeSkills)) or 0 do
            local skillName, skillType, numAvailable = SafeCall(GetTradeSkillInfo, index)
            skillName = Clean(skillName)
            if skillName and skillType ~= "header" and skillType ~= "subheader" then
                recipes[#recipes + 1] = { name = skillName, difficulty = Clean(skillType), available = Clean(numAvailable) }
            end
        end
        StoreRecipes(name, rank, maxRank, recipes)
        return
    end
    if C_TradeSkillUI and C_TradeSkillUI.GetAllRecipeIDs and C_TradeSkillUI.GetRecipeInfo then
        local line = C_TradeSkillUI.GetBaseProfessionInfo and SafeCall(C_TradeSkillUI.GetBaseProfessionInfo)
        local recipes = {}
        local ids = SafeCall(C_TradeSkillUI.GetAllRecipeIDs)
        if IsUsable(ids) and type(ids) == "table" then
            for _, recipeID in ipairs(ids) do
                local info = SafeCall(C_TradeSkillUI.GetRecipeInfo, recipeID)
                if IsUsable(info) and type(info) == "table" and info.learned and Clean(info.name) then
                    recipes[#recipes + 1] = { name = info.name, difficulty = Clean(info.relativeDifficulty), available = Clean(info.numAvailable) }
                end
            end
        end
        if IsUsable(line) and type(line) == "table" then
            StoreRecipes(line.professionName, line.skillLevel, line.maxSkillLevel, recipes)
        end
    end
end

local function CacheCraft()
    if type(GetCraftDisplaySkillLine) ~= "function" or type(GetNumCrafts) ~= "function" then
        return
    end
    local name, rank, maxRank = SafeCall(GetCraftDisplaySkillLine)
    local recipes = {}
    for index = 1, Clean(SafeCall(GetNumCrafts)) or 0 do
        local craftName, subName, craftType, numAvailable = SafeCall(GetCraftInfo, index)
        craftName = Clean(craftName)
        if craftName and craftType ~= "header" then
            local label = Clean(subName) and subName ~= "" and (craftName .. " (" .. subName .. ")") or craftName
            recipes[#recipes + 1] = { name = label, difficulty = Clean(craftType), available = Clean(numAvailable) }
        end
    end
    StoreRecipes(name, rank, maxRank, recipes)
end

local function CollectMacros()
    local macros = {}
    if type(GetNumMacros) ~= "function" or type(GetMacroInfo) ~= "function" then
        return macros
    end
    local accountCount, characterCount = SafeCall(GetNumMacros)
    local function Add(index, scope)
        local name, icon, body = SafeCall(GetMacroInfo, index)
        name = Clean(name)
        if name then
            macros[#macros + 1] = { name = name, body = Clean(body), icon = Clean(icon), scope = scope }
        end
    end
    for index = 1, Clean(accountCount) or 0 do
        Add(index, "account")
    end
    local firstCharacter = MAX_ACCOUNT_MACROS or 120
    for index = firstCharacter + 1, firstCharacter + (Clean(characterCount) or 0) do
        Add(index, "character")
    end
    return macros
end

-- Heavy sections are kept until one of their events says they changed.
local sectionCache = {}
local CACHED_SECTIONS = { talents = true, spells = true, actionBars = true, pet = true, macros = true, reputations = true, skills = true }
local SECTION_INVALIDATION = {
    CHARACTER_POINTS_CHANGED = { "talents" },
    PLAYER_TALENT_UPDATE = { "talents" },
    TRAIT_CONFIG_UPDATED = { "talents" },
    PLAYER_LEVEL_UP = { "talents", "spells" },
    LEARNED_SPELL_IN_TAB = { "spells", "actionBars" },
    SPELLS_CHANGED = { "spells" },
    ACTIONBAR_SLOT_CHANGED = { "actionBars" },
    ACTIONBAR_PAGE_CHANGED = { "actionBars" },
    UPDATE_BINDINGS = { "actionBars" },
    UNIT_PET = { "pet" },
    PET_BAR_UPDATE = { "pet" },
    UPDATE_MACROS = { "macros", "actionBars" },
    UPDATE_FACTION = { "reputations" },
    SKILL_LINES_CHANGED = { "skills" },
}

local function ClearSectionCache()
    for name in pairs(sectionCache) do
        sectionCache[name] = nil
    end
end

local function Section(name, collector, errors)
    if CACHED_SECTIONS[name] and sectionCache[name] ~= nil then
        return sectionCache[name] or nil
    end
    local ok, result = pcall(collector)
    if not ok then
        errors[#errors + 1] = name .. ": " .. tostring(result)
        return nil
    end
    -- A section still waiting for game data (talent texts) is collected again next time.
    if CACHED_SECTIONS[name] and not (type(result) == "table" and result.incomplete) then
        sectionCache[name] = result == nil and false or result
    end
    return result
end

local SNAPSHOT_SECTIONS = {
    { "character", CollectCharacter },
    { "location", CollectLocation },
    { "stats", CollectStats },
    { "equipment", CollectEquipment },
    { "bags", CollectBags },
    { "professions", CollectProfessions },
    { "quests", CollectQuests },
    { "talents", CollectTalents },
    { "skills", CollectSkills },
    { "spells", CollectSpells },
    { "macros", CollectMacros },
    { "actionBars", CollectActionBars },
    { "pet", CollectPet },
    { "reputations", CollectReputations },
    { "auras", CollectAuras },
    { "group", CollectGroup },
    { "target", function() return UnitSummary("target") end },
}

local function CollectSnapshot(reason)
    EnsureStorage()
    local errors = {}
    local snapshot = { meta = CollectMeta(reason) }
    for _, section in ipairs(SNAPSHOT_SECTIONS) do
        snapshot[section[1]] = Section(section[1], section[2], errors)
    end
    snapshot.bank = MentorCharDB.bank
    snapshot.recipes = MentorCharDB.recipes
    if #errors > 0 then
        snapshot.meta.errors = errors
    end
    return snapshot
end

local function StoreSnapshot(snapshot)
    EnsureStorage()
    local key = snapshot.character and snapshot.character.key or GetCharacterKey()
    MentorDB.snapshots[key] = snapshot
    MentorDB.lastCharacterKey = key
end

local function GetStoredSnapshot()
    EnsureStorage()
    local key = MentorDB.lastCharacterKey
    if key then
        return MentorDB.snapshots[key]
    end
    return nil
end

local function TakeSnapshot(reason)
    if InCombatLockdown() then
        return nil
    end
    local ok, snapshot = pcall(CollectSnapshot, reason)
    if not ok or type(snapshot) ~= "table" then
        return nil
    end
    StoreSnapshot(snapshot)
    return snapshot
end

local function ScheduleSnapshot(reason)
    if snapshotPending or not C_Timer or not C_Timer.After then
        return
    end
    snapshotPending = true
    C_Timer.After(SNAPSHOT_DELAY, function()
        snapshotPending = false
        TakeSnapshot(reason)
    end)
end

local function DescribeItemText(item)
    local details = {}
    if item.itemLevel then
        details[#details + 1] = "ilvl " .. item.itemLevel
    end
    if item.itemSubType then
        details[#details + 1] = item.itemSubType
    end
    if item.itemID then
        details[#details + 1] = "id " .. item.itemID
    end
    local text = ToText(item.name)
    if item.count and item.count > 1 then
        text = text .. " x" .. item.count
    end
    if #details > 0 then
        text = text .. " (" .. table.concat(details, ", ") .. ")"
    end
    return text
end

local function AddTooltipLines(lines, tooltip, indent)
    if type(tooltip) ~= "table" then
        return
    end
    for _, tooltipLine in ipairs(tooltip) do
        AddLine(lines, indent .. "> " .. tooltipLine)
    end
end

local function FormatSnapshot(snapshot)
    local lines = {}
    local meta = snapshot.meta
    local character = snapshot.character
    local location = snapshot.location
    local stats = snapshot.stats
    AddLine(lines, "# Fiche Mentor")
    AddLine(lines, "- Date: " .. ToText(meta.updatedAtText))
    AddLine(lines, "- Client: " .. ToText(meta.client) .. " (build " .. ToText(meta.build) .. ", interface " .. ToText(meta.interface) .. ")")
    AddLine(lines, "- Addon version: " .. ToText(meta.addonVersion))
    AddLine(lines, "")
    AddLine(lines, "## Character")
    AddLine(lines, "- Name: " .. character.key)
    AddLine(lines, "- Level: " .. ToText(character.level))
    AddLine(lines, "- Race / Class: " .. ToText(character.race) .. " " .. ToText(character.class))
    AddLine(lines, "- Faction: " .. ToText(character.faction))
    AddLine(lines, "- XP: " .. ToText(character.xp) .. " / " .. ToText(character.xpMax) .. " (rested " .. ToText(character.restedXp, "0") .. ")")
    AddLine(lines, "- Money: " .. character.moneyText)
    AddLine(lines, "")
    AddLine(lines, "## Location")
    AddLine(lines, "- Zone: " .. ToText(location.zone))
    AddLine(lines, "- Subzone: " .. ToText(location.subZone, "none"))
    if location.x and location.y then
        AddLine(lines, string.format("- Coordinates: %.1f, %.1f (map %s)", location.x, location.y, ToText(location.mapID)))
    else
        AddLine(lines, "- Coordinates: unknown")
    end
    AddLine(lines, "")
    AddLine(lines, "## Stats")
    AddLine(lines, "- Max health: " .. ToText(stats.maxHealth))
    AddLine(lines, "- Max power: " .. ToText(stats.maxPower))
    for _, stat in ipairs(stats.primary) do
        AddLine(lines, "- " .. stat.name .. ": " .. ToText(stat.value))
    end
    AddLine(lines, "- Armor: " .. ToText(stats.armor))
    AddLine(lines, "")
    AddLine(lines, "## Equipment")
    for _, entry in ipairs(snapshot.equipment) do
        if entry.item then
            AddLine(lines, "- " .. entry.slot .. ": " .. DescribeItemText(entry.item))
            AddTooltipLines(lines, entry.item.tooltip, "    ")
        else
            AddLine(lines, "- " .. entry.slot .. ": empty")
        end
    end
    AddLine(lines, "")
    AddLine(lines, "## Bags")
    AddLine(lines, string.format("- Slots: %d free / %d", snapshot.bags.freeSlots, snapshot.bags.totalSlots))
    for _, item in ipairs(snapshot.bags.items) do
        AddLine(lines, "- [" .. item.bag .. ":" .. item.slot .. "] " .. DescribeItemText(item))
        AddTooltipLines(lines, item.tooltip, "    ")
    end
    AddLine(lines, "")
    AddLine(lines, "## Professions")
    if #snapshot.professions == 0 then
        AddLine(lines, "- None detected")
    end
    for _, profession in ipairs(snapshot.professions) do
        AddLine(lines, "- " .. profession.label .. ": " .. profession.name .. " " .. ToText(profession.skill) .. " / " .. ToText(profession.maxSkill))
    end
    local talents = snapshot.talents
    if type(talents) == "table" then
        AddLine(lines, "")
        AddLine(lines, "## Talents")
        if talents.spec then
            AddLine(lines, "- Specialization: " .. ToText(talents.spec) .. " (" .. ToText(talents.role, "no role") .. ")")
        end
        if talents.unspent then
            AddLine(lines, "- Unspent points: " .. ToText(talents.unspent))
        end
        if talents.loadoutString then
            AddLine(lines, "- Loadout string: " .. talents.loadoutString)
        end
        for _, tree in ipairs(talents.trees or {}) do
            local picked = {}
            for _, talent in ipairs(tree.talents) do
                picked[#picked + 1] = string.format("%s %s/%s", talent.name, ToText(talent.rank), ToText(talent.maxRank, "?"))
            end
            AddLine(lines, string.format("- %s (%s points): %s", ToText(tree.name), ToText(tree.points, "0"), #picked > 0 and table.concat(picked, ", ") or "none"))
        end
    end
    if type(snapshot.skills) == "table" and #snapshot.skills > 0 then
        AddLine(lines, "")
        AddLine(lines, "## Skills")
        for _, skill in ipairs(snapshot.skills) do
            AddLine(lines, string.format("- [%s] %s %s / %s", ToText(skill.category, "?"), skill.name, ToText(skill.rank), ToText(skill.maxRank)))
        end
    end
    if type(snapshot.spells) == "table" and #snapshot.spells > 0 then
        local names = {}
        for _, spell in ipairs(snapshot.spells) do
            names[#names + 1] = spell.name
        end
        AddLine(lines, "")
        AddLine(lines, "## Known spells")
        AddLine(lines, table.concat(names, ", "))
    end
    if type(snapshot.macros) == "table" and #snapshot.macros > 0 then
        AddLine(lines, "")
        AddLine(lines, "## Macros")
        for _, macro in ipairs(snapshot.macros) do
            AddLine(lines, "- " .. macro.name .. " (" .. macro.scope .. "): " .. ToText(macro.body, ""):gsub("\n", " / "))
        end
    end
    AddLine(lines, "")
    AddLine(lines, "## Quest log")
    if #snapshot.quests == 0 then
        AddLine(lines, "- No active quests")
    end
    for _, quest in ipairs(snapshot.quests) do
        local status = "in progress"
        if quest.complete then
            status = "complete"
        end
        AddLine(lines, string.format("- [%s] %s (id %s, level %s)", status, ToText(quest.title), ToText(quest.questID), ToText(quest.level)))
        for _, objective in ipairs(quest.objectives) do
            local objectiveText = ToText(objective.text, "")
            if objective.finished then
                objectiveText = objectiveText .. " (done)"
            end
            AddLine(lines, "  - " .. objectiveText)
        end
    end
    return table.concat(lines, "\n")
end

local function CreateTextWindow(name, title)
    local frame = CreateFrame("Frame", name, UIParent, "BasicFrameTemplateWithInset")
    frame:SetSize(600, 450)
    frame:SetPoint("CENTER")
    frame:SetFrameStrata("DIALOG")
    frame:SetMovable(true)
    frame:SetClampedToScreen(true)
    frame:EnableMouse(true)
    frame:RegisterForDrag("LeftButton")
    frame:SetScript("OnDragStart", frame.StartMoving)
    frame:SetScript("OnDragStop", frame.StopMovingOrSizing)
    frame:Hide()

    frame.titleText = frame:CreateFontString(nil, "OVERLAY", "GameFontHighlight")
    frame.titleText:SetPoint("TOP", frame, "TOP", 0, -5)
    frame.titleText:SetText(title)

    frame.hintText = frame:CreateFontString(nil, "OVERLAY", "GameFontNormalSmall")
    frame.hintText:SetPoint("TOPLEFT", frame, "TOPLEFT", 14, -32)
    frame.hintText:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -14, -32)
    frame.hintText:SetJustifyH("LEFT")

    local scrollFrame = CreateFrame("ScrollFrame", nil, frame, "UIPanelScrollFrameTemplate")
    scrollFrame:SetPoint("TOPLEFT", frame, "TOPLEFT", 14, -52)
    scrollFrame:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", -34, 44)

    local editBox = CreateFrame("EditBox", nil, scrollFrame)
    editBox:SetMultiLine(true)
    editBox:SetAutoFocus(false)
    editBox:SetFontObject(ChatFontNormal)
    editBox:SetWidth(540)
    editBox:SetMaxLetters(0)
    editBox:SetScript("OnEscapePressed", function(self)
        self:ClearFocus()
    end)
    scrollFrame:SetScrollChild(editBox)

    local actionButton = CreateFrame("Button", nil, frame, "UIPanelButtonTemplate")
    actionButton:SetSize(140, 24)
    actionButton:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", -14, 12)

    frame.scrollFrame = scrollFrame
    frame.editBox = editBox
    frame.actionButton = actionButton

    table.insert(UISpecialFrames, name)
    return frame
end

local function CreateTracker()
    tracker = CreateFrame("Frame", "MentorTracker", UIParent, "BackdropTemplate")
    tracker:SetSize(TRACKER_WIDTH, 40)
    tracker:SetPoint("RIGHT", UIParent, "RIGHT", -40, 0)
    tracker:SetBackdrop({
        bgFile = "Interface\\Tooltips\\UI-Tooltip-Background",
        edgeFile = "Interface\\Tooltips\\UI-Tooltip-Border",
        tile = true,
        tileSize = 16,
        edgeSize = 16,
        insets = { left = 4, right = 4, top = 4, bottom = 4 },
    })
    tracker:SetBackdropColor(0, 0, 0, 0.75)
    tracker:SetMovable(true)
    tracker:SetClampedToScreen(true)
    tracker:EnableMouse(true)
    tracker:RegisterForDrag("LeftButton")
    tracker:SetScript("OnDragStart", tracker.StartMoving)
    tracker:SetScript("OnDragStop", tracker.StopMovingOrSizing)

    tracker.header = tracker:CreateFontString(nil, "OVERLAY", "GameFontNormal")
    tracker.header:SetPoint("TOPLEFT", tracker, "TOPLEFT", 10, -8)
end

local function RefreshTracker()
    EnsureStorage()
    if not tracker then
        CreateTracker()
    end
    local plan = MentorCharDB.plan
    if MentorCharDB.trackerHidden or not plan or #plan == 0 then
        tracker:Hide()
        return
    end
    local doneCount = 0
    local offset = -28
    for index, step in ipairs(plan) do
        if step.done then
            doneCount = doneCount + 1
        end
        local row = trackerRows[index]
        if not row then
            row = CreateFrame("Button", nil, tracker)
            row.label = row:CreateFontString(nil, "OVERLAY", "GameFontHighlightSmall")
            row.label:SetPoint("TOPLEFT", row, "TOPLEFT", 0, 0)
            row.label:SetWidth(TRACKER_WIDTH - 20)
            row.label:SetJustifyH("LEFT")
            row:SetScript("OnClick", function(self)
                local current = MentorCharDB.plan[self.stepIndex]
                if current then
                    current.done = not current.done
                    RefreshTracker()
                end
            end)
            trackerRows[index] = row
        end
        row.stepIndex = index
        if step.done then
            row.label:SetText("|cff808080[x] " .. step.text .. "|r")
        else
            row.label:SetText("[ ] " .. step.text)
        end
        local rowHeight = math.max(row.label:GetStringHeight(), 12) + 4
        row:SetSize(TRACKER_WIDTH - 20, rowHeight)
        row:ClearAllPoints()
        row:SetPoint("TOPLEFT", tracker, "TOPLEFT", 10, offset)
        row:Show()
        offset = offset - rowHeight
    end
    for index = #plan + 1, #trackerRows do
        trackerRows[index]:Hide()
    end
    tracker.header:SetText(string.format("Plan de Mentor (%d/%d)", doneCount, #plan))
    tracker:SetHeight(-offset + 8)
    tracker:Show()
end

local function ParsePlan(text)
    local plan = {}
    for line in text:gmatch("[^\r\n]+") do
        local cleaned = line:gsub("^%s*[%-%*]%s+", "")
        cleaned = cleaned:gsub("^%s*%d+[%.%)]%s+", "")
        cleaned = cleaned:gsub("^%s*%[[ xX]?%]%s*", "")
        cleaned = cleaned:gsub("^%s+", "")
        cleaned = cleaned:gsub("%s+$", "")
        if cleaned ~= "" then
            local escaped = cleaned:gsub("|", "||")
            plan[#plan + 1] = { text = escaped, done = false }
        end
    end
    return plan
end

local MACRO_NAME_LIMIT = 16
local MACRO_BODY_LIMIT = 255
local DEFAULT_MACRO_ICON = "INV_MISC_QUESTIONMARK"

local function InstallMacro(macro)
    if type(macro) ~= "table" or type(macro.name) ~= "string" or type(macro.body) ~= "string" then
        return false, "invalid macro"
    end
    local name = strtrim(macro.name)
    if name == "" then
        return false, "empty name"
    end
    if strlenutf8 and strlenutf8(name) > MACRO_NAME_LIMIT then
        name = name:sub(1, MACRO_NAME_LIMIT)
    end
    local body = macro.body
    if #body > MACRO_BODY_LIMIT then
        return false, name .. ": longer than " .. MACRO_BODY_LIMIT .. " characters"
    end
    local icon = type(macro.icon) == "string" and macro.icon ~= "" and macro.icon or DEFAULT_MACRO_ICON
    local existing = SafeCall(GetMacroIndexByName, name)
    if existing and existing > 0 then
        local ok, result = pcall(EditMacro, existing, name, icon, body)
        return ok and result ~= nil, ok and name or tostring(result)
    end
    local ok, result = pcall(CreateMacro, name, icon, body, macro.perCharacter and true or false)
    if ok and result then
        return true, name
    end
    return false, name .. ": " .. (ok and "no free macro slot" or tostring(result))
end

local function InstallMacros(macros)
    if type(macros) ~= "table" or #macros == 0 or type(CreateMacro) ~= "function" then
        return
    end
    if InCombatLockdown() then
        Print("Les macros de Mentor seront installées après le combat : tape /reload.")
        return
    end
    for _, macro in ipairs(macros) do
        local ok, detail = InstallMacro(macro)
        if ok then
            Print("Macro installée : " .. detail .. " (ouvre /macro pour la glisser sur tes barres).")
        else
            Print("|cffff6666Macro non installée|r : " .. tostring(detail))
        end
    end
end

local function ApplyInbox()
    EnsureStorage()
    local inbox = MentorInbox
    if type(inbox) ~= "table" or type(inbox.id) ~= "string" or inbox.id == "" then
        return
    end
    if MentorCharDB.lastInboxId == inbox.id then
        return
    end
    if type(inbox.character) == "string" and inbox.character ~= "" and inbox.character ~= GetCharacterKey() then
        return
    end
    if InCombatLockdown() and type(inbox.macros) == "table" and #inbox.macros > 0 then
        Print("Des macros de Mentor attendent : tape /reload hors combat.")
        return
    end
    MentorCharDB.lastInboxId = inbox.id
    InstallMacros(inbox.macros)
    if type(inbox.plan) == "table" and #inbox.plan > 0 then
        local plan = {}
        for _, stepText in ipairs(inbox.plan) do
            if type(stepText) == "string" and stepText ~= "" then
                local escaped = stepText:gsub("|", "||")
                plan[#plan + 1] = { text = escaped, done = false }
            end
        end
        MentorCharDB.plan = plan
        MentorCharDB.trackerHidden = false
        Print(string.format("Nouveau plan de Mentor : %d étape%s.", #plan, #plan > 1 and "s" or ""))
    end
    if type(inbox.notes) == "table" then
        for _, note in ipairs(inbox.notes) do
            if type(note) == "string" and note ~= "" then
                local escaped = note:gsub("|", "||")
                Print(escaped)
            end
        end
    end
end

local function ShowExport()
    ClearSectionCache()
    local snapshot = TakeSnapshot("manual export")
    if not snapshot then
        snapshot = GetStoredSnapshot()
        if not snapshot then
            Print("Aucune fiche pour l'instant : réessaie hors combat.")
            return
        end
        Print("En combat : voici la dernière fiche prise hors combat.")
    end
    local text = FormatSnapshot(snapshot)
    if not exportWindow then
        exportWindow = CreateTextWindow("MentorExportWindow", "Mentor - Ma fiche")
        exportWindow.hintText:SetText("Ctrl+A puis Ctrl+C pour la copier. L'app Mentor la lit toute seule après /mentor sync.")
        exportWindow.actionButton:SetText("Tout sélectionner")
        exportWindow.actionButton:SetScript("OnClick", function()
            exportWindow.editBox:SetFocus()
            exportWindow.editBox:HighlightText()
        end)
    end
    exportWindow.editBox:SetText(text)
    exportWindow:Show()
    exportWindow.scrollFrame:SetVerticalScroll(0)
    exportWindow.editBox:SetFocus()
    exportWindow.editBox:HighlightText()
end

local function ShowPlanImport()
    if not planWindow then
        planWindow = CreateTextWindow("MentorPlanWindow", "Mentor - Importer un plan")
        planWindow.hintText:SetText("Colle le plan (une étape par ligne), puis clique sur Enregistrer.")
        planWindow.actionButton:SetText("Enregistrer")
        planWindow.actionButton:SetScript("OnClick", function()
            EnsureStorage()
            local plan = ParsePlan(planWindow.editBox:GetText())
            MentorCharDB.plan = plan
            MentorCharDB.trackerHidden = false
            RefreshTracker()
            planWindow:Hide()
            Print(string.format("Plan enregistré : %d étape%s.", #plan, #plan > 1 and "s" or ""))
        end)
    end
    planWindow.editBox:SetText("")
    planWindow:Show()
    planWindow.editBox:SetFocus()
end

local function SyncAndReload()
    if InCombatLockdown() then
        Print("Impossible en combat : réessaie une fois le combat terminé.")
        return
    end
    ClearSectionCache()
    TakeSnapshot("manual sync")
    ReloadUI()
end

-- The game's own commands win over an addon command with the same name (the game already
-- uses "/mt" for its main tank command), so the short form is only kept when no other
-- command or emote has that name.
local SHORT_COMMAND = "/men"

local function CommandIsFree(command)
    for name, value in pairs(_G) do
        if type(name) == "string" and type(value) == "string" and value:lower() == command
            and (name:find("^SLASH_") or name:find("^EMOTE%d+_CMD")) and not name:find("^SLASH_MENTOR") then
            return false
        end
    end
    return true
end

local shortCommand = CommandIsFree(SHORT_COMMAND) and SHORT_COMMAND or nil

local function PrintHelp()
    Print("/mentor sync" .. (shortCommand and (" (ou " .. shortCommand .. " sync)") or "") .. " - met ta fiche à jour pour l'app (recharge l'interface)")
    Print("/mentor export - affiche ta fiche en texte")
    Print("/mentor plan - colle un plan à suivre")
    Print("/mentor show - affiche le suivi du plan")
    Print("/mentor hide - masque le suivi du plan")
    Print("/mentor clear - supprime le plan")
end

local function HandleSlashCommand(message)
    EnsureStorage()
    local command = strtrim(message or ""):lower()
    if command == "" or command == "export" then
        ShowExport()
    elseif command == "sync" then
        SyncAndReload()
    elseif command == "plan" then
        ShowPlanImport()
    elseif command == "show" then
        MentorCharDB.trackerHidden = false
        RefreshTracker()
    elseif command == "hide" then
        MentorCharDB.trackerHidden = true
        RefreshTracker()
    elseif command == "clear" then
        MentorCharDB.plan = {}
        RefreshTracker()
        Print("Plan supprimé.")
    else
        PrintHelp()
    end
end

SLASH_MENTOR1 = "/mentor"
SLASH_MENTOR2 = shortCommand
SlashCmdList.MENTOR = HandleSlashCommand

local eventFrame = CreateFrame("Frame")

local function RegisterSafely(eventName)
    pcall(eventFrame.RegisterEvent, eventFrame, eventName)
end

RegisterSafely("ADDON_LOADED")
RegisterSafely("PLAYER_ENTERING_WORLD")
RegisterSafely("PLAYER_LOGOUT")
for _, eventName in ipairs(SNAPSHOT_EVENTS) do
    RegisterSafely(eventName)
end

eventFrame:SetScript("OnEvent", function(_, event, firstArgument)
    local invalidated = SECTION_INVALIDATION[event]
    if invalidated and not (event == "UNIT_PET" and firstArgument ~= "player") then
        for _, name in ipairs(invalidated) do
            sectionCache[name] = nil
        end
    end
    if event == "ADDON_LOADED" then
        if firstArgument == ADDON_NAME then
            EnsureStorage()
            eventFrame:UnregisterEvent("ADDON_LOADED")
        end
    elseif event == "PLAYER_ENTERING_WORLD" then
        ClearSectionCache()
        ApplyInbox()
        RefreshTracker()
        ScheduleSnapshot(event)
    elseif event == "PLAYER_LOGOUT" then
        TakeSnapshot(event)
    elseif event == "TRADE_SKILL_SHOW" or event == "TRADE_SKILL_UPDATE" or event == "TRADE_SKILL_LIST_UPDATE" then
        pcall(CacheTradeSkill)
        ScheduleSnapshot(event)
    elseif event == "CRAFT_SHOW" or event == "CRAFT_UPDATE" then
        pcall(CacheCraft)
        ScheduleSnapshot(event)
    elseif event == "BANKFRAME_OPENED" then
        bankOpen = true
        pcall(CacheBank)
        ScheduleSnapshot(event)
    elseif event == "BANKFRAME_CLOSED" then
        bankOpen = false
    elseif event == "PLAYERBANKSLOTS_CHANGED" or event == "BAG_UPDATE_DELAYED" then
        if bankOpen then
            pcall(CacheBank)
        end
        ScheduleSnapshot(event)
    else
        ScheduleSnapshot(event)
    end
end)
