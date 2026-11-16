//! World of Warcraft integration: install folder detection, Mentor addon install,
//! character snapshot loading and the inbox used to send macros and plans to the game.

use crate::lua;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const ADDON_NAME: &str = "Mentor";
const ADDON_FILES: [&str; 3] = ["Mentor.toc", "Mentor.lua", "Inbox.lua"];
const DB_GLOBAL: &str = "MentorDB";
const INBOX_GLOBAL: &str = "MentorInbox";
/// Former name of the addon: removed on install, its saved data is still read until the first sync.
const LEGACY_ADDON: &str = "ClaudeBridge";
const MACRO_NAME_LIMIT: usize = 16;
const MACRO_BODY_LIMIT: usize = 255;

#[derive(Clone, Debug, Serialize)]
pub struct Flavor {
    pub folder: String,
    pub path: String,
    pub product: String,
    pub version: String,
    pub interface: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct WowStatus {
    pub folder: String,
    pub found: bool,
    pub flavor: Option<Flavor>,
    pub candidates: Vec<Flavor>,
    pub addon_installed: Option<String>,
    pub addon_bundled: String,
    pub addon_outdated: bool,
    pub saved_variables: Option<String>,
    /// Characters whose addon list has the addon switched off.
    pub addon_disabled: Vec<String>,
    pub game_running: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub per_character: bool,
}

pub struct Snapshot {
    pub key: String,
    pub data: Value,
    pub updated_at: i64,
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn registry_install_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for key in [
            r"HKLM\SOFTWARE\WOW6432Node\Blizzard Entertainment\World of Warcraft",
            r"HKLM\SOFTWARE\Blizzard Entertainment\World of Warcraft",
        ] {
            let output = std::process::Command::new("reg")
                .args(["query", key, "/v", "InstallPath"])
                .creation_flags(0x0800_0000)
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = text.lines().find(|line| line.contains("InstallPath")) {
                if let Some(index) = line.find("REG_SZ") {
                    let value = line[index + 6..].trim();
                    if !value.is_empty() {
                        return Some(PathBuf::from(value));
                    }
                }
            }
        }
    }
    None
}

fn normalize_root(path: PathBuf) -> PathBuf {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
    if name.starts_with('_') && name.ends_with('_') {
        return path.parent().map(Path::to_path_buf).unwrap_or(path);
    }
    path
}

pub fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = registry_install_path() {
        roots.push(normalize_root(path));
    }
    for drive in b'C'..=b'H' {
        let drive = format!("{}:\\", drive as char);
        for relative in [
            "Program Files (x86)\\World of Warcraft",
            "Program Files\\World of Warcraft",
            "World of Warcraft",
            "Games\\World of Warcraft",
            "Jeux\\World of Warcraft",
            "Battle.net\\World of Warcraft",
            "Blizzard\\World of Warcraft",
        ] {
            roots.push(PathBuf::from(&drive).join(relative));
        }
    }
    let mut unique = Vec::new();
    for root in roots {
        if root.is_dir() && !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

fn build_versions(root: &Path) -> Vec<(String, String)> {
    let Some(text) = read_text(&root.join(".build.info")) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let columns: Vec<String> = header.split('|').map(|column| column.split('!').next().unwrap_or_default().to_string()).collect();
    let index_of = |name: &str| columns.iter().position(|column| column == name);
    let (Some(product_index), Some(version_index)) = (index_of("Product"), index_of("Version")) else {
        return Vec::new();
    };
    lines
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('|').collect();
            Some((fields.get(product_index)?.to_string(), fields.get(version_index)?.to_string()))
        })
        .collect()
}

fn interface_from_version(version: &str) -> Option<u32> {
    let mut parts = version.split('.').map(|part| part.parse::<u32>().ok());
    let major = parts.next()??;
    let minor = parts.next()??;
    let patch = parts.next()??;
    Some(major * 10000 + minor * 100 + patch)
}

pub fn flavors(root: &Path) -> Vec<Flavor> {
    let versions = build_versions(root);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let folder = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !folder.starts_with('_') || !folder.ends_with('_') {
            continue;
        }
        if !path.join("Interface").is_dir() && !path.join("WTF").is_dir() {
            continue;
        }
        let product = read_text(&path.join(".flavor.info"))
            .and_then(|text| text.lines().nth(1).map(|line| line.trim().to_string()))
            .unwrap_or_default();
        let version = versions.iter().find(|(name, _)| name == &product).map(|(_, version)| version.clone()).unwrap_or_default();
        result.push(Flavor {
            interface: interface_from_version(&version),
            folder,
            path: path.to_string_lossy().to_string(),
            product,
            version,
        });
    }
    result.sort_by_key(|flavor| (flavor.folder != "_forever_", flavor.folder.clone()));
    result
}

pub fn detect_default_folder() -> Option<String> {
    candidate_roots().iter().flat_map(|root| flavors(root)).next().map(|flavor| flavor.path)
}

fn flavor_for(folder: &str) -> Option<Flavor> {
    let path = PathBuf::from(folder);
    let root = path.parent()?;
    flavors(root).into_iter().find(|flavor| Path::new(&flavor.path) == path)
}

fn toc_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| line.strip_prefix("## Version:").map(|version| version.trim().to_string()))
}

fn version_tuple(version: &str) -> Vec<u32> {
    version.split('.').map(|part| part.trim().parse().unwrap_or(0)).collect()
}

pub fn bundled_addon_version(resource_dir: &Path) -> String {
    read_text(&resource_dir.join(format!("{ADDON_NAME}.toc"))).and_then(|text| toc_version(&text)).unwrap_or_default()
}

pub fn addon_dir(folder: &str) -> PathBuf {
    Path::new(folder).join("Interface").join("AddOns").join(ADDON_NAME)
}

pub fn status(folder: &str, resource_dir: &Path) -> WowStatus {
    let mut candidates = Vec::new();
    for root in candidate_roots() {
        candidates.extend(flavors(&root));
    }
    let bundled = bundled_addon_version(resource_dir);
    if folder.is_empty() || !Path::new(folder).is_dir() {
        return WowStatus { folder: folder.to_string(), candidates, addon_bundled: bundled, ..Default::default() };
    }
    let installed = read_text(&addon_dir(folder).join(format!("{ADDON_NAME}.toc"))).and_then(|text| toc_version(&text));
    let outdated = installed.as_deref().is_some_and(|version| version_tuple(version) < version_tuple(&bundled));
    WowStatus {
        folder: folder.to_string(),
        found: true,
        flavor: flavor_for(folder),
        candidates,
        addon_installed: installed,
        addon_bundled: bundled,
        addon_outdated: outdated,
        saved_variables: find_saved_variables(folder).map(|path| path.to_string_lossy().to_string()),
        addon_disabled: addon_list_files(folder)
            .into_iter()
            .filter(|(_, text)| addon_line_state(text) == Some(false))
            .map(|(path, _)| character_label(&path))
            .collect(),
        game_running: game_running(),
    }
}

/// Every per-character AddOns.txt: WTF/Account/<account>/<realm>/<character>/AddOns.txt.
fn addon_list_files(folder: &str) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    let Ok(accounts) = std::fs::read_dir(Path::new(folder).join("WTF").join("Account")) else {
        return files;
    };
    for account in accounts.flatten().filter(|entry| entry.path().is_dir()) {
        for realm in std::fs::read_dir(account.path()).into_iter().flatten().flatten().filter(|entry| entry.path().is_dir()) {
            for character in std::fs::read_dir(realm.path()).into_iter().flatten().flatten() {
                let path = character.path().join("AddOns.txt");
                if let Some(text) = read_text(&path) {
                    files.push((path, text));
                }
            }
        }
    }
    files
}

fn addon_line_state(text: &str) -> Option<bool> {
    text.lines().find_map(|line| {
        let (name, state) = line.split_once(':')?;
        (name.trim() == ADDON_NAME).then(|| state.trim().eq_ignore_ascii_case("enabled"))
    })
}

fn character_label(path: &Path) -> String {
    path.parent().and_then(|dir| dir.file_name()).map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn game_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq Wow.exe", "/NH", "/FO", "CSV"])
            .creation_flags(0x0800_0000)
            .output()
        {
            return String::from_utf8_lossy(&output.stdout).to_lowercase().contains("\"wow.exe\"");
        }
    }
    false
}

/// Switches the addon back on in every character's addon list. The game rewrites these
/// files when it quits, so this only works while it is closed.
pub fn enable_addon(folder: &str) -> Result<usize> {
    if game_running() {
        return Err(anyhow!(
            "ferme le jeu d'abord (il réécrit la liste des addons en quittant), ou coche Mentor dans le bouton AddOns de l'écran de sélection du personnage"
        ));
    }
    let mut changed = 0;
    for (path, text) in addon_list_files(folder) {
        if addon_line_state(&text) != Some(false) {
            continue;
        }
        let updated: Vec<String> = text
            .lines()
            .map(|line| match line.split_once(':') {
                Some((name, _)) if name.trim() == ADDON_NAME => format!("{ADDON_NAME}: enabled"),
                _ => line.to_string(),
            })
            .collect();
        std::fs::write(&path, updated.join("\n") + "\n").with_context(|| format!("cannot write {}", path.display()))?;
        changed += 1;
    }
    Ok(changed)
}

pub fn install_addon(folder: &str, resource_dir: &Path) -> Result<String> {
    let legacy = Path::new(folder).join("Interface").join("AddOns").join(LEGACY_ADDON);
    if legacy.join(format!("{LEGACY_ADDON}.toc")).is_file() {
        std::fs::remove_dir_all(&legacy).with_context(|| format!("cannot remove {}", legacy.display()))?;
    }
    let target = addon_dir(folder);
    std::fs::create_dir_all(&target).with_context(|| format!("cannot create {}", target.display()))?;
    let interface = flavor_for(folder).and_then(|flavor| flavor.interface);
    for file in ADDON_FILES {
        let source = resource_dir.join(file);
        let destination = target.join(file);
        if file == "Inbox.lua" && destination.exists() {
            continue;
        }
        let mut text = read_text(&source).ok_or_else(|| anyhow!("missing bundled file {}", source.display()))?;
        if file.ends_with(".toc") {
            if let Some(interface) = interface {
                text = text
                    .lines()
                    .map(|line| if line.starts_with("## Interface:") { format!("## Interface: {interface}") } else { line.to_string() })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
            }
        }
        std::fs::write(&destination, text).with_context(|| format!("cannot write {}", destination.display()))?;
    }
    Ok(bundled_addon_version(resource_dir))
}

pub fn find_saved_variables(folder: &str) -> Option<PathBuf> {
    let accounts: Vec<PathBuf> = std::fs::read_dir(Path::new(folder).join("WTF").join("Account")).ok()?.flatten().map(|entry| entry.path()).collect();
    for name in [ADDON_NAME, LEGACY_ADDON] {
        let newest = accounts
            .iter()
            .map(|account| account.join("SavedVariables").join(format!("{name}.lua")))
            .filter(|path| path.is_file())
            .max_by_key(|path| path.metadata().and_then(|metadata| metadata.modified()).unwrap_or(SystemTime::UNIX_EPOCH));
        if newest.is_some() {
            return newest;
        }
    }
    None
}

pub fn saved_variables_modified(folder: &str) -> Option<SystemTime> {
    find_saved_variables(folder)?.metadata().ok()?.modified().ok()
}

pub fn load_snapshot(folder: &str) -> Result<Snapshot> {
    let path = find_saved_variables(folder).ok_or_else(|| anyhow!("pas encore de fiche : connecte-toi avec un personnage, puis tape /mentor sync"))?;
    let source = read_text(&path).ok_or_else(|| anyhow!("cannot read {}", path.display()))?;
    let globals = lua::parse_assignments(&source).with_context(|| format!("cannot parse {}", path.display()))?;
    let database = globals
        .get(DB_GLOBAL)
        .or_else(|| globals.get(&format!("{LEGACY_ADDON}DB")))
        .ok_or_else(|| anyhow!("{DB_GLOBAL} missing in {}", path.display()))?;
    let snapshots = database.get("snapshots").and_then(Value::as_object).ok_or_else(|| anyhow!("no snapshot saved yet"))?;
    let key = database
        .get("lastCharacterKey")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| snapshots.keys().next().cloned())
        .ok_or_else(|| anyhow!("no snapshot saved yet"))?;
    let data = snapshots.get(&key).cloned().ok_or_else(|| anyhow!("no snapshot for {key}"))?;
    let updated_at = data.pointer("/meta/updatedAt").and_then(Value::as_i64).unwrap_or_default();
    Ok(Snapshot { key, data, updated_at })
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => "?".to_string(),
    }
}

fn opt_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    }
}

fn items(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn clip(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= limit {
        return text;
    }
    let mut clipped: String = text.chars().take(limit).collect();
    clipped.push('…');
    clipped
}

fn describe_item(item: &Value) -> String {
    let mut details = Vec::new();
    if let Some(level) = item.get("itemLevel") {
        details.push(format!("ilvl {}", text(Some(level))));
    }
    if let Some(sub_type) = item.get("itemSubType").and_then(Value::as_str) {
        details.push(sub_type.to_string());
    }
    if let Some(required) = item.get("requiredLevel").and_then(Value::as_i64).filter(|level| *level > 1) {
        details.push(format!("req {required}"));
    }
    let mut line = text(item.get("name"));
    if let Some(count) = item.get("count").and_then(Value::as_i64).filter(|count| *count > 1) {
        let _ = write!(line, " x{count}");
    }
    if !details.is_empty() {
        let _ = write!(line, " ({})", details.join(", "));
    }
    line
}

fn is_equippable(item: &Value) -> bool {
    item.get("equipLoc")
        .and_then(Value::as_str)
        .is_some_and(|location| !location.is_empty() && !location.starts_with("INVTYPE_NON_EQUIP"))
}

fn tooltip_lines(output: &mut String, item: &Value) {
    for line in items(item.get("tooltip")) {
        if let Some(line) = line.as_str() {
            let _ = writeln!(output, "    > {line}");
        }
    }
}

fn item_list(output: &mut String, list: &[Value]) {
    let mut others = Vec::new();
    for item in list {
        if is_equippable(item) {
            let _ = writeln!(output, "- {} [equippable]", describe_item(item));
            tooltip_lines(output, item);
        } else {
            others.push(describe_item(item));
        }
    }
    if !others.is_empty() {
        let _ = writeln!(output, "Other items: {}", others.join("; "));
    }
}

fn seconds(milliseconds: Option<&Value>) -> Option<String> {
    let value = milliseconds?.as_f64()?;
    if value <= 0.0 {
        return None;
    }
    let seconds = value / 1000.0;
    Some(if seconds.fract() == 0.0 { format!("{seconds:.0} s") } else { format!("{seconds:.1} s") })
}

fn format_stats(out: &mut String, data: &Value) {
    let Some(stats) = data.get("stats") else {
        return;
    };
    let primary: Vec<String> = items(stats.get("primary"))
        .iter()
        .map(|stat| format!("{} {}", text(stat.get("name")), text(stat.get("value"))))
        .collect();
    let _ = writeln!(
        out,
        "\n## Stats\nHealth {}, power {}, armor {}. {}",
        text(stats.get("maxHealth")),
        text(stats.get("maxPower")),
        text(stats.get("armor")),
        primary.join(", ")
    );
    let labels: [(&str, &str, &str); 16] = [
        ("attackPower", "attack power", ""),
        ("rangedAttackPower", "ranged attack power", ""),
        ("meleeCrit", "melee crit", "%"),
        ("rangedCrit", "ranged crit", "%"),
        ("spellCrit", "spell crit", "%"),
        ("spellPower", "spell damage bonus", ""),
        ("healingBonus", "healing bonus", ""),
        ("meleeHit", "melee hit bonus", "%"),
        ("spellHit", "spell hit bonus", "%"),
        ("dodge", "dodge", "%"),
        ("parry", "parry", "%"),
        ("block", "block", "%"),
        ("defense", "defense", ""),
        ("manaRegen", "mana regen", "/s"),
        ("manaRegenCasting", "mana regen while casting", "/s"),
        ("mainHandSpeed", "main hand speed", " s"),
    ];
    let mut details: Vec<String> = labels
        .iter()
        .filter_map(|(key, label, unit)| opt_text(stats.get(*key)).map(|value| format!("{label} {value}{unit}")))
        .collect();
    if let Some(damage) = opt_text(stats.get("mainHandDamage")) {
        details.push(format!("main hand damage {damage}"));
    }
    let resistances: Vec<String> = items(stats.get("resistances"))
        .iter()
        .map(|resistance| format!("{} {}", text(resistance.get("name")), text(resistance.get("value"))))
        .collect();
    if !resistances.is_empty() {
        details.push(format!("resistances: {}", resistances.join(", ")));
    }
    if !details.is_empty() {
        let _ = writeln!(out, "{}.", details.join(", "));
    }
}

fn format_talents(out: &mut String, talents: &Value) {
    let _ = writeln!(out, "\n## Talents (rank/max; row and column in the tree)");
    if let Some(spec) = opt_text(talents.get("spec")) {
        let _ = writeln!(out, "Specialization: {spec}");
    }
    if let Some(unspent) = opt_text(talents.get("unspent")) {
        let _ = writeln!(out, "Unspent talent points: {unspent}");
    }
    if let Some(loadout) = opt_text(talents.get("loadoutString")) {
        let _ = writeln!(out, "Loadout string: {loadout}");
    }
    let trees = items(talents.get("trees"));
    if trees.is_empty() {
        let trace: Vec<String> = items(talents.get("trace")).iter().map(|step| text(Some(step))).collect();
        let reason = match (opt_text(talents.get("problem")), trace.is_empty()) {
            (Some(problem), true) => format!(" ({problem})"),
            (Some(problem), false) => format!(" ({problem}; {})", trace.join(", ")),
            (None, false) => format!(" ({})", trace.join(", ")),
            (None, true) => String::new(),
        };
        let _ = writeln!(out, "The talent trees could not be exported{reason}: do not assume any talent, ask the player.");
    }
    for tree in trees {
        let _ = writeln!(out, "### {} ({} points)", text(tree.get("name")), text(tree.get("points")));
        for talent in items(tree.get("talents")) {
            let requires = talent
                .get("requires")
                .map(|requirement| match opt_text(requirement.get("name")) {
                    Some(name) => format!(", requires {name}"),
                    None => format!(", requires row {} col {}", text(requirement.get("tier")), text(requirement.get("column"))),
                })
                .unwrap_or_default();
            let options: Vec<String> = items(talent.get("choices")).iter().map(|option| text(Some(option))).collect();
            let choice = if options.len() > 1 { format!(" [one to pick among: {}]", options.join(", ")) } else { String::new() };
            let description = opt_text(talent.get("description")).map(|description| format!(": {}", clip(&description, 170))).unwrap_or_default();
            let _ = writeln!(
                out,
                "- {} {}/{} (row {}, col {}{requires}){choice}{description}",
                text(talent.get("name")),
                text(talent.get("rank")),
                text(talent.get("maxRank")),
                text(talent.get("tier")),
                text(talent.get("column")),
            );
        }
    }
}

fn format_spells(out: &mut String, spells: &[Value]) {
    let _ = writeln!(out, "\n## Spellbook (exact in-game names, highest rank)");
    let mut current_tab = String::new();
    for spell in spells {
        let tab = text(spell.get("tab"));
        if tab != current_tab {
            let _ = writeln!(out, "### {tab}");
            current_tab = tab;
        }
        let mut line = format!("- {}", text(spell.get("name")));
        if let Some(rank) = opt_text(spell.get("rank")) {
            let _ = write!(line, " ({rank})");
        }
        if spell.get("passive").and_then(Value::as_bool).unwrap_or(false) {
            line.push_str(" [passive]");
        }
        if spell.get("notLearned").and_then(Value::as_bool).unwrap_or(false) {
            line.push_str(" [not learned yet]");
        }
        let mut facts = Vec::new();
        if let Some(cost) = opt_text(spell.get("cost")) {
            facts.push(cost);
        }
        if spell.get("castMs").is_some() {
            facts.push(seconds(spell.get("castMs")).map(|cast| format!("{cast} cast")).unwrap_or_else(|| "instant".into()));
        }
        if let Some(range) = spell.get("maxRange").and_then(Value::as_f64).filter(|range| *range > 0.0) {
            facts.push(format!("{range:.0} yd"));
        }
        if let Some(cooldown) = seconds(spell.get("cooldownMs")) {
            facts.push(format!("{cooldown} cooldown"));
        }
        if !facts.is_empty() {
            let _ = write!(line, " — {}", facts.join(", "));
        }
        if let Some(description) = opt_text(spell.get("description")) {
            let _ = write!(line, ": {}", clip(&description, 190));
        }
        let _ = writeln!(out, "{line}");
    }
}

fn bar_name(slot: i64) -> &'static str {
    match slot {
        1..=12 => "main bar",
        13..=24 => "main bar page 2",
        25..=36 => "right bar",
        37..=48 => "right bar 2",
        49..=60 => "bottom right bar",
        61..=72 => "bottom left bar",
        73..=120 => "stance/form bar",
        _ => "extra bar",
    }
}

fn format_action_bars(out: &mut String, bars: &Value) {
    let slots = items(bars.get("slots"));
    if slots.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n## Action bars (slot, key binding, content; current page {})", text(bars.get("page")));
    for slot in slots {
        let number = slot.get("slot").and_then(Value::as_i64).unwrap_or_default();
        let key = opt_text(slot.get("key")).map(|key| format!(" [{key}]")).unwrap_or_else(|| " [no key]".into());
        let mut line = format!("- {} slot {number}{key}: {} ({})", bar_name(number), text(slot.get("name")), text(slot.get("type")));
        if let Some(body) = opt_text(slot.get("body")) {
            let _ = write!(line, " = {}", body.replace('\n', " / "));
        }
        let _ = writeln!(out, "{line}");
    }
}

fn format_pet(out: &mut String, pet: &Value) {
    let _ = writeln!(
        out,
        "\n## Pet\n{} — {} level {}, health {}{}",
        text(pet.get("name")),
        text(pet.get("family")),
        text(pet.get("level")),
        text(pet.get("maxHealth")),
        opt_text(pet.get("happiness")).map(|happiness| format!(", happiness {happiness}")).unwrap_or_default()
    );
    let actions: Vec<String> = items(pet.get("actions"))
        .iter()
        .map(|action| {
            let autocast = opt_text(action.get("autocast")).map(|state| format!(" (autocast {state})")).unwrap_or_default();
            format!("{}{autocast}", text(action.get("name")))
        })
        .collect();
    if !actions.is_empty() {
        let _ = writeln!(out, "Pet bar: {}", actions.join(", "));
    }
    let spells: Vec<String> = items(pet.get("spells")).iter().map(|spell| text(Some(spell))).collect();
    if !spells.is_empty() {
        let _ = writeln!(out, "Pet spells: {}", spells.join(", "));
    }
}

fn format_recipes(out: &mut String, recipes: &Value) {
    let Some(map) = recipes.as_object() else {
        return;
    };
    if map.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n## Known recipes (as seen the last time each profession window was opened)");
    for (profession, entry) in map {
        let list: Vec<String> = items(entry.get("recipes"))
            .iter()
            .map(|recipe| {
                let mut label = text(recipe.get("name"));
                let mut tags = Vec::new();
                if let Some(difficulty) = opt_text(recipe.get("difficulty")) {
                    tags.push(difficulty);
                }
                if let Some(available) = recipe.get("available").and_then(Value::as_i64).filter(|count| *count > 0) {
                    tags.push(format!("can craft {available}"));
                }
                if !tags.is_empty() {
                    let _ = write!(label, " [{}]", tags.join(", "));
                }
                label
            })
            .collect();
        let _ = writeln!(
            out,
            "### {profession} {}/{} (updated {})\n{}",
            text(entry.get("rank")),
            text(entry.get("maxRank")),
            text(entry.get("updatedText")),
            list.join("; ")
        );
    }
}

/// Formats a snapshot as compact text for the coach.
pub fn format_snapshot(snapshot: &Snapshot) -> String {
    let data = &snapshot.data;
    let mut out = String::new();
    let get = |path: &str| data.pointer(path);
    let _ = writeln!(out, "Character sheet exported by the Mentor addon on {}.", text(get("/meta/updatedAtText")));
    let _ = writeln!(out, "Client {} (interface {}).", text(get("/meta/client")), text(get("/meta/interface")));
    if data.get("character").is_some() {
        let _ = writeln!(
            out,
            "\n## Character\n{} - level {} {} {} ({}). XP {} / {}. Money: {}.",
            snapshot.key,
            text(get("/character/level")),
            text(get("/character/race")),
            text(get("/character/class")),
            text(get("/character/faction")),
            text(get("/character/xp")),
            text(get("/character/xpMax")),
            text(get("/character/moneyText")),
        );
    }
    let _ = writeln!(
        out,
        "Location: {}{} {}",
        text(get("/location/zone")),
        get("/location/subZone").and_then(Value::as_str).map(|sub| format!(" / {sub}")).unwrap_or_default(),
        match (get("/location/x"), get("/location/y")) {
            (Some(x), Some(y)) => format!("({}, {})", text(Some(x)), text(Some(y))),
            _ => String::new(),
        }
    );
    let auras: Vec<String> = items(get("/auras")).iter().map(|aura| text(Some(aura))).collect();
    if !auras.is_empty() {
        let _ = writeln!(out, "Active buffs: {}", auras.join(", "));
    }
    let members: Vec<String> = items(get("/group/members"))
        .iter()
        .map(|member| format!("{} ({} {})", text(member.get("name")), text(member.get("level")), text(member.get("class"))))
        .collect();
    if get("/group/raid").and_then(Value::as_bool).unwrap_or(false) {
        let _ = writeln!(out, "Group: raid of {}", text(get("/group/size")));
    } else if !members.is_empty() {
        let _ = writeln!(out, "Group: {}", members.join(", "));
    }
    if let Some(target) = get("/target").filter(|target| target.is_object()) {
        let _ = writeln!(
            out,
            "Target at export time: {} (level {}, {}{})",
            text(target.get("name")),
            text(target.get("level")),
            opt_text(target.get("creatureType")).or_else(|| opt_text(target.get("class"))).unwrap_or_default(),
            if target.get("enemy").and_then(Value::as_bool).unwrap_or(false) { ", hostile" } else { "" }
        );
    }
    format_stats(&mut out, data);
    if let Some(talents) = get("/talents") {
        format_talents(&mut out, talents);
    }
    let _ = writeln!(out, "\n## Equipment");
    for entry in items(get("/equipment")) {
        match entry.get("item") {
            Some(item) => {
                let _ = writeln!(out, "- {}: {}", text(entry.get("slot")), describe_item(item));
                tooltip_lines(&mut out, item);
            }
            None => {
                let _ = writeln!(out, "- {}: empty", text(entry.get("slot")));
            }
        }
    }
    let _ = writeln!(out, "\n## Bags ({} free / {} slots)", text(get("/bags/freeSlots")), text(get("/bags/totalSlots")));
    item_list(&mut out, items(get("/bags/items")));
    if let Some(bank) = get("/bank").filter(|bank| bank.is_object()) {
        let _ = writeln!(
            out,
            "\n## Bank (as seen when last opened on {}; {} free / {} slots)",
            text(bank.get("updatedText")),
            text(bank.get("freeSlots")),
            text(bank.get("totalSlots"))
        );
        item_list(&mut out, items(bank.get("items")));
    }
    let professions: Vec<String> = items(get("/professions"))
        .iter()
        .map(|profession| format!("{} {}/{}", text(profession.get("name")), text(profession.get("skill")), text(profession.get("maxSkill"))))
        .collect();
    let skills: Vec<String> = items(get("/skills"))
        .iter()
        .map(|skill| format!("{} {}/{}", text(skill.get("name")), text(skill.get("rank")), text(skill.get("maxRank"))))
        .collect();
    if !professions.is_empty() || !skills.is_empty() {
        let _ = writeln!(out, "\n## Professions and skills\n{}", professions.into_iter().chain(skills).collect::<Vec<_>>().join(", "));
    }
    if let Some(recipes) = get("/recipes") {
        format_recipes(&mut out, recipes);
    }
    let spells = items(get("/spells"));
    if !spells.is_empty() {
        format_spells(&mut out, spells);
    }
    if let Some(bars) = get("/actionBars") {
        format_action_bars(&mut out, bars);
    }
    let macros = items(get("/macros"));
    if !macros.is_empty() {
        let _ = writeln!(out, "\n## Existing macros");
        for item in macros {
            let _ = writeln!(out, "- {} ({}): {}", text(item.get("name")), text(item.get("scope")), text(item.get("body")).replace('\n', " / "));
        }
    }
    if let Some(pet) = get("/pet").filter(|pet| pet.is_object()) {
        format_pet(&mut out, pet);
    }
    let reputations: Vec<String> = items(get("/reputations"))
        .iter()
        .map(|reputation| {
            let progress = match (opt_text(reputation.get("value")), opt_text(reputation.get("max"))) {
                (Some(value), Some(max)) => format!(" ({value}/{max})"),
                _ => String::new(),
            };
            format!("{}: {}{progress}", text(reputation.get("name")), text(reputation.get("standing")))
        })
        .collect();
    if !reputations.is_empty() {
        let _ = writeln!(out, "\n## Reputations\n{}", reputations.join("; "));
    }
    let quests = items(get("/quests"));
    if !quests.is_empty() {
        let _ = writeln!(out, "\n## Quest log");
        for quest in quests {
            let complete = quest.get("complete").and_then(Value::as_bool).unwrap_or(false);
            let _ = writeln!(
                out,
                "- [{}] {} (level {})",
                if complete { "complete" } else { "in progress" },
                text(quest.get("title")),
                text(quest.get("level"))
            );
            for objective in items(quest.get("objectives")) {
                let done = objective.get("finished").and_then(Value::as_bool).unwrap_or(false);
                let _ = writeln!(out, "  - {}{}", text(objective.get("text")), if done { " (done)" } else { "" });
            }
        }
    }
    let errors: Vec<String> = items(get("/meta/errors")).iter().map(|error| text(Some(error))).collect();
    if !errors.is_empty() {
        let _ = writeln!(out, "\n(Export warnings, some sections may be missing: {})", errors.join("; "));
    }
    out
}

pub fn validate_macro(item: &Macro) -> Result<Macro> {
    let name: String = item.name.trim().chars().take(MACRO_NAME_LIMIT).collect();
    if name.is_empty() {
        return Err(anyhow!("le nom de la macro est vide"));
    }
    let body = item.body.trim_matches('\n').replace("\r\n", "\n");
    if body.len() > MACRO_BODY_LIMIT {
        return Err(anyhow!("la macro dépasse {MACRO_BODY_LIMIT} caractères ({})", body.len()));
    }
    Ok(Macro { name, body, icon: item.icon.clone(), per_character: item.per_character })
}

pub fn write_inbox(folder: &str, character: &str, macros: &[Macro], plan: &[String], notes: &[String]) -> Result<()> {
    let directory = addon_dir(folder);
    if !directory.is_dir() {
        return Err(anyhow!("l'addon Mentor n'est pas installé"));
    }
    let id = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|duration| duration.as_millis()).unwrap_or_default();
    let mut out = format!("{INBOX_GLOBAL} = {{\n");
    let _ = writeln!(out, "    id = {},", lua::escape_string(&id.to_string()));
    let _ = writeln!(out, "    character = {},", lua::escape_string(character));
    out.push_str("    plan = {\n");
    for step in plan {
        let _ = writeln!(out, "        {},", lua::escape_string(step));
    }
    out.push_str("    },\n    notes = {\n");
    for note in notes {
        let _ = writeln!(out, "        {},", lua::escape_string(note));
    }
    out.push_str("    },\n    macros = {\n");
    for item in macros {
        let _ = writeln!(
            out,
            "        {{ name = {}, body = {}, icon = {}, perCharacter = {} }},",
            lua::escape_string(&item.name),
            lua::escape_string(&item.body),
            lua::escape_string(&item.icon),
            item.per_character
        );
    }
    out.push_str("    },\n}\n");
    let target = directory.join("Inbox.lua");
    let temporary = directory.join("Inbox.lua.tmp");
    std::fs::write(&temporary, out)?;
    std::fs::rename(&temporary, &target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_inbox_for_lua() {
        let folder = std::env::temp_dir().join("coach_inbox_test");
        std::fs::create_dir_all(addon_dir(folder.to_str().unwrap())).unwrap();
        let item = validate_macro(&Macro {
            name: "Trait de l'ombre !".into(),
            body: "#showtooltip\n/cast [@mouseover,harm][] Trait de l'ombre\n/petattack \"x\"".into(),
            icon: String::new(),
            per_character: false,
        })
        .unwrap();
        assert_eq!(item.name.chars().count(), 16);
        write_inbox(folder.to_str().unwrap(), "Jeanne-Ocre", &[item], &["Aller à Fossoyeuse".into()], &[]).unwrap();
        let text = std::fs::read_to_string(addon_dir(folder.to_str().unwrap()).join("Inbox.lua")).unwrap();
        let globals = lua::parse_assignments(&text).unwrap();
        assert_eq!(globals["MentorInbox"]["macros"][0]["body"], "#showtooltip\n/cast [@mouseover,harm][] Trait de l'ombre\n/petattack \"x\"");
        println!("INBOX_PATH={}", addon_dir(folder.to_str().unwrap()).join("Inbox.lua").display());
    }

    #[test]
    fn enables_disabled_addon() {
        let folder = std::env::temp_dir().join("coach_addons_test");
        let character = folder.join("WTF").join("Account").join("1#1").join("Realm").join("Jeanne");
        std::fs::create_dir_all(&character).unwrap();
        std::fs::write(character.join("AddOns.txt"), "Questie: enabled\nMentor: disabled\nTomTom: enabled\n").unwrap();
        let folder = folder.to_str().unwrap();
        assert_eq!(status(folder, Path::new(".")).addon_disabled, vec!["Jeanne".to_string()]);
        if !game_running() {
            assert_eq!(enable_addon(folder).unwrap(), 1);
            let text = std::fs::read_to_string(character.join("AddOns.txt")).unwrap();
            assert!(text.contains("Mentor: enabled") && text.contains("TomTom: enabled"));
            assert!(status(folder, Path::new(".")).addon_disabled.is_empty());
        }
    }

    /// Uses the SavedVariables produced by `python tests/run_mock.py %TEMP%/coach_mock_wow`.
    #[test]
    fn formats_full_mock_snapshot_when_present() {
        let folder = std::env::temp_dir().join("coach_mock_wow");
        if !folder.join("WTF").is_dir() {
            return;
        }
        let snapshot = load_snapshot(folder.to_str().unwrap()).expect("mock snapshot");
        let text = format_snapshot(&snapshot);
        for section in ["## Talents", "## Spellbook", "## Action bars", "## Pet", "## Bank", "## Known recipes", "## Reputations", "attack power"] {
            assert!(text.contains(section), "missing {section}");
        }
        // Talents of WoW Forever (trait system): trees, grid, prerequisite by name, choice.
        assert!(text.contains("### Affliction (8 points)") && text.contains("Unspent talent points: 3"), "talent trees");
        assert!(text.contains("- Sort 9999 0/5 (row 2, col 2, requires Sort 17810): SetSpellByID"), "talent prerequisite");
        assert!(text.contains("- Sort 18220 1/1 (row 3, col 2) [one to pick among: Sort 18265, Sort 18220]"), "talent choice");
        assert!(text.contains("slot 61 [SHIFT-1]"), "Bartender key binding");
        assert!(text.contains("Trait de l'ombre (Rang 2)"), "highest spell rank");
        assert!(!text.contains("Export warnings"));
        println!("{text}");
    }

    #[test]
    fn computes_interface() {
        assert_eq!(interface_from_version("1.60.1.70862"), Some(16001));
        assert_eq!(interface_from_version("11.2.7.1"), Some(110207));
    }

    #[test]
    fn loads_real_saved_variables_when_present() {
        let folder = r"C:\Program Files (x86)\World of Warcraft\_forever_";
        if !Path::new(folder).is_dir() {
            return;
        }
        let snapshot = load_snapshot(folder).expect("snapshot");
        let text = format_snapshot(&snapshot);
        assert!(text.contains("## Equipment"));
        println!("{}", &text[..text.len().min(1500)]);
    }
}
