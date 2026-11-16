//! Local knowledge base: official World of Warcraft: Forever documents (Blizzard news
//! articles, hotfix notes and the posts of Blizzard staff on the official forums), kept up to
//! date in the background. The coach reads them with its file tools before searching the web.

use crate::html;
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Folder of the documents, relative to the directory Claude runs in.
pub const FOLDER: &str = "knowledge";
/// Seconds between two automatic synchronizations.
pub const SYNC_INTERVAL: u64 = 2 * 3600;

const NEWS: &str = "https://news.blizzard.com";
const HUB: &str = "https://worldofwarcraft.blizzard.com/en-us/forever";
const FORUM: &str = "https://us.forums.blizzard.com/en/wow";
const ENGLISH: &str = "en-us";
/// Word that tells an official page is about this version of the game.
const GAME_KEYWORD: &str = "forever";
/// Official articles that are always kept, even once they have left the news feed:
/// hotfixes, what is new, starter guide, launch overview.
const SEED_ARTICLES: [u64; 4] = [24315281, 24315821, 24315280, 24315820];
/// Official forum topics that are always kept (development notes).
const SEED_TOPICS: [u64; 1] = [2360696];
const DEFAULT_CATEGORIES: [u64; 5] = [346, 347, 348, 349, 352];
const LISTING_INTERVAL: u64 = 6 * 3600;
const ARTICLE_INTERVAL: u64 = 24 * 3600;
const CATEGORY_INTERVAL: u64 = 7 * 24 * 3600;
const BACKFILL_DAYS: u64 = 75;
const BACKFILL_PAGES: usize = 15;
const UPDATE_PAGES: usize = 4;
const MAX_NEW_ARTICLES: usize = 40;
const MAX_TOPIC_REQUESTS: usize = 160;
/// Forum account that reposts the beginning of news articles: the articles are read in full instead.
const NEWS_BOT: &str = "BlizzardEntertainment";
const MIN_ARTICLE_CHARS: usize = 400;
const REQUEST_PAUSE: Duration = Duration::from_millis(200);
/// Lines of the index given to the coach; the complete index is in INDEX.md.
const ARTICLE_LINES: usize = 26;
const TOPIC_LINES: usize = 14;
/// Articles shorter than this are announcements with little to learn from.
const SMALL_ARTICLE: usize = 3000;
const EXCERPT_CHARS: usize = 2200;
/// Documents updated by Blizzard in the last three days are shown as new.
const FRESH_SECONDS: u64 = 72 * 3600;

static ARTICLE_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/article/(\d{7,9})/([a-z0-9-]+)").unwrap());
static OFFICIAL_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:news\.blizzard\.com|worldofwarcraft\.blizzard\.com)/(?:[a-z]{2}-[a-z]{2}/)?(?:news|article)/(\d{7,9})").unwrap()
});
static ANY_ARTICLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(?:news|article)/(\d{7,9})").unwrap());
static NEXT_REDIRECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"NEXT_REDIRECT;replace;([^;]+);").unwrap());
static CANONICAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<link rel="canonical" href="([^"]+)""#).unwrap());
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<h1[^>]*>(.*?)</h1>").unwrap());
static PUBLISHED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""datePublished":"([^"]+)""#).unwrap());
static MODIFIED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""dateModified":"([^"]+)""#).unwrap());
static TRAILING_DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+[-–—]\s+[^-–—]*\d{4}\s*$").unwrap());

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or_default()
}

/// ISO 8601 UTC date and time of a Unix timestamp.
fn iso(epoch: u64) -> String {
    let days = (epoch / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let seconds = epoch % 86_400;
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", seconds / 3600, seconds % 3600 / 60, seconds % 60)
}

/// "2026-11-12T19:10:52.697Z" → "2026-11-12".
fn day(date: &str) -> &str {
    date.get(..10).unwrap_or(date)
}

/// "2026-11-12T19:10:52.697Z" → "2026-11-12 19:10 UTC".
fn minute(date: &str) -> String {
    match (date.get(..10), date.get(11..16)) {
        (Some(day), Some(time)) => format!("{day} {time} UTC"),
        _ => date.to_string(),
    }
}

fn slug(text: &str) -> String {
    let mut output = String::new();
    for character in crate::text::normalize(text).chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character);
        } else if !output.ends_with('-') && !output.is_empty() {
            output.push('-');
        }
    }
    let output: String = output.trim_matches('-').chars().take(48).collect();
    output.trim_matches('-').to_string()
}

/// FNV-1a: enough to notice that a document changed.
fn fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Hotfix and patch notes are edited in place by Blizzard: they are checked at every sync.
fn is_rolling(title: &str) -> bool {
    let title = title.to_lowercase();
    ["hotfix", "patch notes", "update notes", "known issues"].iter().any(|word| title.contains(word))
}

/// News locales to keep for a coach language: English (always the most up to date) plus the
/// official translation, which gives the names used by the player's game client.
fn locales_for(language: &str) -> Vec<&'static str> {
    let translated = match language {
        "fr" => Some("fr-fr"),
        "de" => Some("de-de"),
        "es" => Some("es-es"),
        "it" => Some("it-it"),
        _ => None,
    };
    std::iter::once(ENGLISH).chain(translated).collect()
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Document {
    /// "news" (Blizzard news article) or "forum" (posts of Blizzard staff in one forum topic).
    pub kind: String,
    /// Article or topic number on the official site.
    pub source_id: u64,
    pub locale: String,
    pub title: String,
    pub url: String,
    /// File name inside the knowledge folder.
    pub file: String,
    pub published: String,
    pub updated: String,
    /// Edited in place by Blizzard (hotfix notes): checked at every synchronization.
    pub rolling: bool,
    pub fetched: u64,
    /// When the text last changed on this computer.
    pub changed: u64,
    pub hash: String,
    pub size: usize,
    etag: Option<String>,
    /// Other official articles this one links to.
    links: Vec<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Post {
    number: u64,
    author: String,
    role: String,
    created: String,
    updated: String,
    text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Topic {
    title: String,
    category: u64,
    fetched: u64,
    posts: BTreeMap<u64, Post>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct State {
    last_sync: u64,
    last_error: Option<String>,
    documents: BTreeMap<String, Document>,
    topics: BTreeMap<u64, Topic>,
    /// Article numbers found in the official listings.
    discovered: BTreeSet<u64>,
    /// Article numbers that are not about this game: never fetched again.
    ignored: BTreeSet<u64>,
    /// Last attempt for pages that did not give a document (missing translation...).
    attempts: BTreeMap<String, u64>,
    listed_at: u64,
    feed_etag: Option<String>,
    categories: BTreeSet<u64>,
    categories_at: u64,
    backfilled: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Change {
    pub title: String,
    pub url: String,
    pub kind: String,
    pub new: bool,
    #[serde(skip)]
    id: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SyncReport {
    /// Documents that are new or whose text changed, one entry per article or topic.
    pub changes: Vec<Change>,
    pub errors: Vec<String>,
    /// First synchronization on this computer.
    pub first: bool,
    pub total: usize,
}

struct Page {
    status: u16,
    body: String,
    etag: Option<String>,
}

struct Http {
    client: reqwest::blocking::Client,
    started: std::cell::Cell<bool>,
}

impl Http {
    fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("Mentor/", env!("CARGO_PKG_VERSION"), " (desktop app; reads the official WoW Forever news)"))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(40))
            .build()?;
        Ok(Self { client, started: std::cell::Cell::new(false) })
    }

    fn get(&self, url: &str, etag: Option<&str>) -> Result<Page> {
        if self.started.replace(true) {
            std::thread::sleep(REQUEST_PAUSE);
        }
        let mut request = self.client.get(url).header("Accept", "text/html,application/json");
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let response = request.send().map_err(|error| anyhow!("{url} injoignable : {error}"))?;
        let status = response.status().as_u16();
        if status == 429 {
            return Err(anyhow!("le site demande de ralentir (429)"));
        }
        let etag = response.headers().get("etag").and_then(|value| value.to_str().ok()).map(str::to_string);
        let body = response.text().map_err(|error| anyhow!("{url} : lecture interrompue ({error})"))?;
        Ok(Page { status, body, etag })
    }

    fn json(&self, url: &str) -> Result<Value> {
        let page = self.get(url, None)?;
        if page.status != 200 {
            return Err(anyhow!("{url} a répondu {}", page.status));
        }
        serde_json::from_str(&page.body).with_context(|| format!("réponse illisible de {url}"))
    }
}

struct Article {
    title: String,
    url: String,
    published: String,
    modified: String,
    body: String,
    links: Vec<u64>,
    etag: Option<String>,
}

enum Fetched {
    Unchanged,
    Missing,
    Article(Article),
}

fn parse_article(page: &str, url: String, etag: Option<String>) -> Result<Article> {
    let start = page.find("<section class=\"blog\"").ok_or_else(|| anyhow!("mise en page inattendue ({url})"))?;
    let content_start = start + page[start..].find('>').ok_or_else(|| anyhow!("mise en page inattendue ({url})"))? + 1;
    let content_end = page[content_start..].find("</section>").map(|index| content_start + index).unwrap_or(page.len());
    let content = &page[content_start..content_end];
    let title = TITLE.captures(page).map(|captures| html::plain_text(&captures[1])).filter(|title| !title.is_empty()).ok_or_else(|| anyhow!("titre introuvable ({url})"))?;
    let date = |pattern: &Regex| pattern.captures(page).map(|captures| captures[1].to_string()).unwrap_or_default();
    // The address everybody uses is refreshed faster by the site's cache than ours.
    let url = CANONICAL.captures(page).map(|captures| captures[1].to_string()).filter(|canonical| canonical.starts_with(NEWS)).unwrap_or(url);
    let mut links: Vec<u64> = OFFICIAL_LINK.captures_iter(content).filter_map(|captures| captures[1].parse().ok()).collect();
    links.sort_unstable();
    links.dedup();
    Ok(Article { title, url, published: date(&PUBLISHED), modified: date(&MODIFIED), body: html::to_markdown(content), links, etag })
}

/// Articles are addressed by number: the site answers with the current address when the
/// title part is wrong, which is how hotfix notes renamed every day are followed.
fn fetch_article(http: &Http, locale: &str, id: u64, known_url: Option<&str>, etag: Option<&str>) -> Result<Fetched> {
    let mut url = known_url.map(str::to_string).unwrap_or_else(|| format!("{NEWS}/{locale}/article/{id}/-"));
    let mut page = http.get(&url, etag)?;
    if page.status == 304 {
        return Ok(Fetched::Unchanged);
    }
    if let Some(path) = NEXT_REDIRECT.captures(&page.body).map(|captures| captures[1].to_string()) {
        if !path.starts_with(&format!("/{locale}/")) {
            return Ok(Fetched::Missing);
        }
        url = format!("{NEWS}{path}");
        page = http.get(&url, None)?;
    }
    match page.status {
        200 => parse_article(&page.body, url, page.etag).map(Fetched::Article),
        404 | 410 => Ok(Fetched::Missing),
        status => Err(anyhow!("{url} a répondu {status}")),
    }
}

fn news_key(id: u64, locale: &str) -> String {
    format!("news-{id}-{locale}")
}

fn forum_key(id: u64) -> String {
    format!("forum-{id}")
}

fn text_of(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn number_of(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

/// A forum post as returned by the topic endpoints (with `include_raw=true`).
fn parse_post(value: &Value) -> Option<(u64, Post)> {
    let id = value.get("id")?.as_u64()?;
    if value.get("hidden").and_then(Value::as_bool).unwrap_or(false) || value.get("deleted_at").is_some_and(|deleted| !deleted.is_null()) {
        return None;
    }
    let raw = text_of(value, "raw");
    let text = if raw.trim().is_empty() { html::to_markdown(&text_of(value, "cooked")) } else { raw.replace("\r\n", "\n").trim().to_string() };
    if text.is_empty() {
        return None;
    }
    let created = text_of(value, "created_at");
    Some((
        id,
        Post {
            number: number_of(value, "post_number"),
            author: text_of(value, "username"),
            role: text_of(value, "user_title"),
            updated: value.get("updated_at").and_then(Value::as_str).unwrap_or(&created).to_string(),
            created,
            text,
        },
    ))
}

fn is_staff(value: &Value) -> bool {
    value.get("staff").and_then(Value::as_bool).unwrap_or(false) || value.get("primary_group_name").is_some_and(|group| group.as_str().is_some_and(|name| !name.is_empty()))
}

pub struct Knowledge {
    directory: PathBuf,
    state_path: PathBuf,
    state: Mutex<State>,
    busy: AtomicBool,
}

impl Knowledge {
    /// `work_dir` is the directory Claude runs in; the state file stays outside of it.
    pub fn open(work_dir: &Path, state_path: PathBuf) -> Self {
        let state = std::fs::read_to_string(&state_path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Self { directory: work_dir.join(FOLDER), state_path, state: Mutex::new(state), busy: AtomicBool::new(false) }
    }

    pub fn last_sync(&self) -> u64 {
        self.state.lock().last_sync
    }

    pub fn is_syncing(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// Fetches what changed on the official sites. Returns `None` when a synchronization is
    /// already running.
    pub fn sync(&self, language: &str) -> Option<SyncReport> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return None;
        }
        let mut state = self.state.lock().clone();
        let mut report = SyncReport { first: state.last_sync == 0, ..Default::default() };
        let started = now();
        let mut reached = false;
        match Http::new() {
            Ok(http) => {
                if let Err(error) = std::fs::create_dir_all(&self.directory) {
                    report.errors.push(format!("dossier des connaissances : {error}"));
                } else {
                    match self.sync_news(&http, &mut state, language, started, &mut report) {
                        Ok(()) => reached = true,
                        Err(error) => report.errors.push(format!("Actualités Blizzard : {error:#}")),
                    }
                    match self.sync_forum(&http, &mut state, started, &mut report) {
                        Ok(()) => reached = true,
                        Err(error) => report.errors.push(format!("Forum officiel : {error:#}")),
                    }
                }
            }
            Err(error) => report.errors.push(format!("{error:#}")),
        }
        if reached {
            state.last_sync = started;
        }
        state.last_error = report.errors.first().cloned();
        report.total = state.documents.len();
        if let Err(error) = self.write_index(&state) {
            log::warn!("Knowledge index: {error:#}");
        }
        match serde_json::to_string(&state).map_err(anyhow::Error::from).and_then(|text| write_atomic(&self.state_path, &text)) {
            Ok(()) => {}
            Err(error) => log::warn!("Knowledge state not saved: {error:#}"),
        }
        *self.state.lock() = state;
        self.busy.store(false, Ordering::SeqCst);
        for error in &report.errors {
            log::warn!("Knowledge sync: {error}");
        }
        log::info!("Knowledge sync: {} document(s), {} change(s), {} error(s)", report.total, report.changes.len(), report.errors.len());
        Some(report)
    }

    // ------------------------------------------------------------------ Blizzard news

    fn sync_news(&self, http: &Http, state: &mut State, language: &str, now: u64, report: &mut SyncReport) -> Result<()> {
        if now.saturating_sub(state.listed_at) >= LISTING_INTERVAL {
            let feed = http.get(&format!("{NEWS}/{ENGLISH}/feed/world-of-warcraft"), state.feed_etag.as_deref())?;
            match feed.status {
                200 => {
                    for captures in ARTICLE_LINK.captures_iter(&feed.body) {
                        let slug = &captures[2];
                        if slug.contains(GAME_KEYWORD) && !slug.contains("wow-weekly") {
                            state.discovered.extend(captures[1].parse::<u64>().ok());
                        }
                    }
                    state.feed_etag = feed.etag;
                    state.listed_at = now;
                }
                304 => state.listed_at = now,
                status => report.errors.push(format!("le fil d'actualités a répondu {status}")),
            }
            match http.get(HUB, None) {
                Ok(page) if page.status == 200 => {
                    state.discovered.extend(ANY_ARTICLE.captures_iter(&page.body).filter_map(|captures| captures[1].parse::<u64>().ok()));
                }
                Ok(page) => report.errors.push(format!("la page du jeu a répondu {}", page.status)),
                Err(error) => report.errors.push(format!("{error:#}")),
            }
        }
        let mut queue: VecDeque<u64> = SEED_ARTICLES.into_iter().collect();
        queue.extend(state.documents.values().filter(|document| document.kind == "news").map(|document| document.source_id));
        queue.extend(state.discovered.iter().copied());
        queue.extend(state.documents.values().flat_map(|document| document.links.iter().copied()));
        let mut seen = BTreeSet::new();

        let locales = locales_for(language);
        let mut new_articles = 0;
        let mut failures = 0;
        while let Some(id) = queue.pop_front() {
            if state.ignored.contains(&id) || !seen.insert(id) {
                continue;
            }
            let english_key = news_key(id, ENGLISH);
            for locale in &locales {
                let key = news_key(id, locale);
                let english = state.documents.get(&english_key).cloned();
                if *locale != ENGLISH && english.is_none() {
                    continue;
                }
                let existing = state.documents.get(&key).cloned();
                let last_attempt = existing.as_ref().map(|document| document.fetched).or_else(|| state.attempts.get(&key).copied());
                let rolling = english.as_ref().is_some_and(|document| document.rolling);
                let due = last_attempt.is_none_or(|time| (rolling && existing.is_some()) || now.saturating_sub(time) >= ARTICLE_INTERVAL);
                if !due {
                    continue;
                }
                if existing.is_none() && *locale == ENGLISH {
                    if new_articles >= MAX_NEW_ARTICLES {
                        break;
                    }
                    new_articles += 1;
                }
                let fetched = fetch_article(http, locale, id, existing.as_ref().map(|document| document.url.as_str()), existing.as_ref().and_then(|document| document.etag.as_deref()));
                match fetched {
                    Ok(Fetched::Unchanged) => {
                        failures = 0;
                        if let Some(document) = state.documents.get_mut(&key) {
                            document.fetched = now;
                        }
                    }
                    Ok(Fetched::Missing) => {
                        failures = 0;
                        state.attempts.insert(key, now);
                        if *locale == ENGLISH && existing.is_none() && !SEED_ARTICLES.contains(&id) {
                            state.ignored.insert(id);
                        }
                    }
                    Ok(Fetched::Article(article)) => {
                        failures = 0;
                        let relevant = SEED_ARTICLES.contains(&id) || article.title.to_lowercase().contains(GAME_KEYWORD);
                        if *locale == ENGLISH && (!relevant || article.body.chars().count() < MIN_ARTICLE_CHARS) {
                            state.ignored.insert(id);
                            break;
                        }
                        // Official articles link to each other: follow them in this same run.
                        queue.extend(article.links.iter().copied());
                        if let Err(error) = self.store_article(state, id, locale, article, now, report) {
                            report.errors.push(format!("article {id} : {error:#}"));
                        }
                    }
                    Err(error) => {
                        failures += 1;
                        state.attempts.insert(key, now);
                        if failures >= 3 {
                            return Err(error);
                        }
                        report.errors.push(format!("{error:#}"));
                    }
                }
            }
        }
        Ok(())
    }

    fn store_article(&self, state: &mut State, id: u64, locale: &str, article: Article, now: u64, report: &mut SyncReport) -> Result<()> {
        let key = news_key(id, locale);
        let english_title = match state.documents.get(&news_key(id, ENGLISH)) {
            Some(document) if locale != ENGLISH => document.title.clone(),
            _ => article.title.clone(),
        };
        let language = locale.get(..2).unwrap_or(locale);
        let file = format!("news-{id}-{}.{language}.md", slug(&TRAILING_DATE.replace(&english_title, "")));
        let translation = if locale == ENGLISH { "" } else { ", official translation: it can be a few days behind the English page" };
        let mut text = format!("# {}\n\nSource: Blizzard News (official{translation}) - {}\n", article.title, article.url);
        if !article.published.is_empty() {
            let _ = writeln!(text, "Published: {}", day(&article.published));
        }
        let _ = write!(text, "\n{}\n", article.body);
        let hash = fingerprint(&format!("{}\n{}", article.title, article.body));
        let previous = state.documents.get(&key).cloned();
        let changed = previous.as_ref().is_none_or(|document| document.hash != hash);
        let path = self.directory.join(&file);
        if changed || !path.is_file() {
            write_atomic(&path, &text)?;
        }
        if let Some(previous) = previous.as_ref().filter(|previous| previous.file != file) {
            let _ = std::fs::remove_file(self.directory.join(&previous.file));
        }
        if changed {
            note_change(report, &article.title, &article.url, "news", id, previous.is_none(), locale != ENGLISH && !is_rolling(&english_title));
        }
        state.attempts.remove(&key);
        state.documents.insert(
            key,
            Document {
                kind: "news".into(),
                source_id: id,
                locale: locale.to_string(),
                rolling: is_rolling(&english_title),
                updated: if article.modified.is_empty() { article.published.clone() } else { article.modified },
                published: article.published,
                title: article.title,
                url: article.url,
                file,
                fetched: now,
                changed: if changed { now } else { previous.map(|document| document.changed).unwrap_or(now) },
                hash,
                size: text.len(),
                etag: article.etag,
                links: article.links,
            },
        );
        Ok(())
    }

    // ------------------------------------------------------------------ official forums

    fn sync_forum(&self, http: &Http, state: &mut State, now: u64, report: &mut SyncReport) -> Result<()> {
        if state.categories.is_empty() {
            state.categories = DEFAULT_CATEGORIES.into_iter().collect();
        }
        if now.saturating_sub(state.categories_at) >= CATEGORY_INTERVAL {
            match http.json(&format!("{FORUM}/categories.json")) {
                Ok(value) => {
                    for category in value.pointer("/category_list/categories").and_then(Value::as_array).into_iter().flatten() {
                        if text_of(category, "name").to_lowercase().contains(GAME_KEYWORD) {
                            state.categories.insert(number_of(category, "id"));
                            state.categories.extend(category.get("subcategory_ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64));
                        }
                    }
                    state.categories_at = now;
                }
                Err(error) => report.errors.push(format!("{error:#}")),
            }
        }
        let mut touched = BTreeSet::new();
        for id in SEED_TOPICS {
            if state.topics.get(&id).is_some_and(|topic| now.saturating_sub(topic.fetched) < ARTICLE_INTERVAL) {
                continue;
            }
            let value = http.json(&format!("{FORUM}/t/{id}.json?include_raw=true"))?;
            let topic = state.topics.entry(id).or_default();
            topic.title = text_of(&value, "title");
            topic.category = number_of(&value, "category_id");
            topic.fetched = now;
            topic.posts = value
                .pointer("/post_stream/posts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|post| is_staff(post))
                .filter_map(parse_post)
                .collect();
            touched.insert(id);
        }

        // Walk the tracker of Blizzard staff posts, newest first.
        let cutoff = iso(now.saturating_sub(BACKFILL_DAYS * 86_400));
        let mut wanted: BTreeMap<u64, (String, u64, Vec<u64>)> = BTreeMap::new();
        let mut before: Option<u64> = None;
        let pages = if state.backfilled { UPDATE_PAGES } else { BACKFILL_PAGES };
        for _ in 0..pages {
            let url = match before {
                Some(id) => format!("{FORUM}/groups/blizzard-tracker/posts.json?before_post_id={id}"),
                None => format!("{FORUM}/groups/blizzard-tracker/posts.json"),
            };
            let value = http.json(&url)?;
            let posts = value.get("posts").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut reached_known = false;
            let mut oldest = String::new();
            for post in &posts {
                let id = number_of(post, "id");
                before = Some(before.map_or(id, |current| current.min(id)));
                oldest = text_of(post, "created_at");
                let title = text_of(post, "topic_title");
                let category = number_of(post, "category_id");
                if text_of(post, "username") == NEWS_BOT || (!state.categories.contains(&category) && !title.to_lowercase().contains(GAME_KEYWORD)) {
                    continue;
                }
                let topic_id = number_of(post, "topic_id");
                if state.topics.get(&topic_id).is_some_and(|topic| topic.posts.contains_key(&id)) {
                    reached_known = true;
                    continue;
                }
                wanted.entry(topic_id).or_insert_with(|| (title, category, Vec::new())).2.push(id);
            }
            if posts.is_empty() || oldest.as_str() < cutoff.as_str() || (state.backfilled && reached_known) {
                break;
            }
        }
        let complete = wanted.len() <= MAX_TOPIC_REQUESTS;
        // Newest topics first: they matter most when the first synchronization is cut short.
        for (topic_id, (title, category, ids)) in wanted.into_iter().rev().take(MAX_TOPIC_REQUESTS) {
            let query: String = ids.iter().map(|id| format!("post_ids[]={id}&")).collect();
            let value = http.json(&format!("{FORUM}/t/{topic_id}/posts.json?{query}include_raw=true"))?;
            let topic = state.topics.entry(topic_id).or_default();
            topic.title = title;
            topic.category = category;
            topic.fetched = now;
            let before_count = topic.posts.len();
            topic.posts.extend(value.pointer("/post_stream/posts").and_then(Value::as_array).into_iter().flatten().filter_map(parse_post));
            // Posts that could not be read (deleted, hidden) are remembered as empty so they are not asked again.
            for id in ids {
                topic.posts.entry(id).or_default();
            }
            if topic.posts.len() != before_count {
                touched.insert(topic_id);
            }
        }
        state.backfilled = state.backfilled || complete;
        for topic_id in touched {
            if let Err(error) = self.store_topic(state, topic_id, now, report) {
                report.errors.push(format!("sujet {topic_id} : {error:#}"));
            }
        }
        Ok(())
    }

    fn store_topic(&self, state: &mut State, id: u64, now: u64, report: &mut SyncReport) -> Result<()> {
        let Some(topic) = state.topics.get(&id) else {
            return Ok(());
        };
        let posts: Vec<&Post> = topic.posts.values().filter(|post| !post.text.is_empty()).collect();
        let (Some(first), Some(last)) = (posts.first(), posts.last()) else {
            return Ok(());
        };
        let title = if topic.title.is_empty() { format!("Topic {id}") } else { topic.title.clone() };
        let url = format!("{FORUM}/t/{id}");
        let mut body = String::new();
        for post in &posts {
            let role = if post.role.is_empty() { String::new() } else { format!(" ({})", post.role) };
            let _ = write!(body, "\n## {} - {}{role}\n\n{}\n", minute(&post.created), post.author, post.text);
        }
        let text = format!("# {title}\n\nSource: official Blizzard forums, posts written by Blizzard staff in this topic - {url}\n{body}");
        let key = forum_key(id);
        let file = format!("forum-{id}-{}.md", slug(&title));
        let hash = fingerprint(&text);
        let previous = state.documents.get(&key).cloned();
        let changed = previous.as_ref().is_none_or(|document| document.hash != hash);
        let path = self.directory.join(&file);
        if changed || !path.is_file() {
            write_atomic(&path, &text)?;
        }
        if let Some(previous) = previous.as_ref().filter(|previous| previous.file != file) {
            let _ = std::fs::remove_file(self.directory.join(&previous.file));
        }
        if changed {
            note_change(report, &title, &url, "forum", id, previous.is_none(), false);
        }
        let document = Document {
            kind: "forum".into(),
            source_id: id,
            locale: ENGLISH.into(),
            rolling: is_rolling(&title),
            published: first.created.clone(),
            updated: posts.iter().map(|post| post.updated.as_str()).max().unwrap_or(last.created.as_str()).to_string(),
            title,
            url,
            file,
            fetched: now,
            changed: if changed { now } else { previous.map(|document| document.changed).unwrap_or(now) },
            hash,
            size: text.len(),
            etag: None,
            links: Vec::new(),
        };
        state.documents.insert(key, document);
        Ok(())
    }

    // ------------------------------------------------------------------ what the coach and the window see

    /// English documents, newest first, each with its translation when there is one.
    fn catalog(state: &State) -> Vec<(&Document, Option<&Document>)> {
        let mut entries: Vec<(&Document, Option<&Document>)> = state
            .documents
            .values()
            .filter(|document| document.locale == ENGLISH)
            .map(|document| {
                let translation = state.documents.values().find(|other| other.kind == document.kind && other.source_id == document.source_id && other.locale != ENGLISH);
                (document, translation)
            })
            .collect();
        entries.sort_by(|left, right| right.0.updated.cmp(&left.0.updated).then_with(|| left.0.file.cmp(&right.0.file)));
        entries
    }

    fn index_line(document: &Document, translation: Option<&Document>) -> String {
        let kind = match (document.kind.as_str(), document.rolling) {
            ("forum", _) => "Blizzard staff posts on the official forums",
            (_, true) => "official notes, edited in place",
            _ => "official article",
        };
        let mut line = format!("- {FOLDER}/{} : {} ({kind}, updated {}, {} kB)", document.file, document.title, day(&document.updated), document.size.div_ceil(1024));
        if let Some(translation) = translation {
            let _ = write!(line, " [official {} translation with the in-game names, possibly older: {FOLDER}/{}]", translation.locale.get(..2).unwrap_or_default(), translation.file);
        }
        line
    }

    fn write_index(&self, state: &State) -> Result<()> {
        if state.documents.is_empty() {
            return Ok(());
        }
        let mut text = String::from("# Knowledge base index\n\nOfficial Blizzard documents about World of Warcraft: Forever, newest first.\n\n");
        for (document, translation) in Self::catalog(state) {
            let _ = writeln!(text, "{}", Self::index_line(document, translation));
        }
        write_atomic(&self.directory.join("INDEX.md"), &text)
    }

    /// Beginning of the newest hotfix notes, so the coach knows the latest changes without a tool call.
    fn latest_changes(&self, state: &State) -> Option<(String, String)> {
        let (document, _) = Self::catalog(state).into_iter().find(|(document, _)| document.rolling && document.kind == "news")?;
        let text = std::fs::read_to_string(self.directory.join(&document.file)).ok()?;
        let notes = text.split_once("\n---\n").map(|(_, notes)| notes).unwrap_or(&text).trim();
        let mut excerpt = String::new();
        for line in notes.lines() {
            if excerpt.len() + line.len() > EXCERPT_CHARS {
                excerpt.push_str("[...]\n");
                break;
            }
            excerpt.push_str(line);
            excerpt.push('\n');
        }
        Some((document.file.clone(), excerpt))
    }

    /// Part of the system prompt that tells the coach what is in the knowledge base.
    pub fn prompt_section(&self) -> String {
        let state = self.state.lock();
        let catalog = Self::catalog(&state);
        if catalog.is_empty() {
            return String::new();
        }
        let mut text = String::from(
            "Knowledge base\n\
             - The app keeps official Blizzard documents about this game in the local folder `knowledge` and refreshes them every few hours: hotfix and patch notes, the posts of Blizzard staff on the official forums (class tuning, known issues, schedules), and official guides. This is your most reliable and most current source.\n\
             - Check it BEFORE the web for anything it may cover: what changed, class and talent tuning, bug fixes, new systems, races, dungeons and raids, events and dates. Use Grep on the folder `knowledge` with a short keyword, then Read only the lines around the matches (never read a large file whole). Most documents are in English: search with the English name of a spell, class or zone, and with the name the player used.\n\
             - Say that a fact comes from the official notes when it does. If the knowledge base does not cover the question, search the web.\n\
             - File lines below read: name | title | last official update | size. `+translation` means the official translation in the player's language is next to the file, with the language code instead of `en` in its name: it gives the names used by the player's game client and can be a few days behind.\n",
        );
        let line = |(document, translation): &(&Document, Option<&Document>)| {
            format!(
                "  - {} | {} | {} | {} kB{}\n",
                document.file,
                document.title,
                day(&document.updated),
                document.size.div_ceil(1024),
                if translation.is_some() { " | +translation" } else { "" }
            )
        };
        // Official articles stay listed however old they are; short announcements are left out.
        let articles: Vec<_> = catalog.iter().filter(|(document, _)| document.kind == "news" && (document.rolling || document.size >= SMALL_ARTICLE)).take(ARTICLE_LINES).collect();
        let topics: Vec<_> = catalog
            .iter()
            .filter(|(document, _)| document.kind == "forum")
            .enumerate()
            .filter(|(position, (document, _))| *position < TOPIC_LINES || SEED_TOPICS.contains(&document.source_id))
            .map(|(_, entry)| entry)
            .collect();
        if !articles.is_empty() {
            text.push_str("- Official articles, guides and notes (`news-` files), newest first:\n");
            text.extend(articles.iter().map(|entry| line(entry)));
        }
        if !topics.is_empty() {
            text.push_str("- Latest forum topics with posts from Blizzard staff (`forum-` files hold only what the staff wrote), newest first:\n");
            text.extend(topics.iter().map(|entry| line(entry)));
        }
        let listed = articles.len() + topics.len();
        if catalog.len() > listed {
            let _ = writeln!(text, "- {} more files are listed in {FOLDER}/INDEX.md.", catalog.len() - listed);
        }
        if let Some((file, excerpt)) = self.latest_changes(&state) {
            let _ = write!(text, "- Latest official changes (beginning of {FOLDER}/{file}; read the file for the rest):\n{excerpt}");
        }
        text.trim_end().to_string()
    }

    /// Summary for the window: status and the documents in the player's language when translated.
    pub fn overview(&self, language: &str) -> Value {
        let state = self.state.lock();
        let recent = iso(now().saturating_sub(FRESH_SECONDS));
        let documents: Vec<Value> = Self::catalog(&state)
            .into_iter()
            .map(|(document, translation)| {
                // Translations of notes edited in place lag behind: show the English page for those.
                let shown = translation.filter(|translation| !document.rolling && translation.locale.starts_with(language)).unwrap_or(document);
                json!({
                    "title": shown.title,
                    "url": shown.url,
                    "kind": document.kind,
                    "rolling": document.rolling,
                    "updated": document.updated,
                    "fresh": document.updated.as_str() >= recent.as_str(),
                    "size": document.size,
                })
            })
            .collect();
        json!({
            "syncing": self.is_syncing(),
            "last_sync": state.last_sync,
            "error": state.last_error,
            "documents": documents,
        })
    }
}

/// One line per article or topic in the report, even when several languages changed.
fn note_change(report: &mut SyncReport, title: &str, url: &str, kind: &str, id: u64, new: bool, translation: bool) {
    if let Some(existing) = report.changes.iter_mut().find(|change| change.kind == kind && change.id == id) {
        if translation {
            existing.title = title.to_string();
            existing.url = url.to_string();
        }
        return;
    }
    report.changes.push(Change { title: title.to_string(), url: url.to_string(), kind: kind.to_string(), new, id });
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, text).with_context(|| format!("écriture de {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("écriture de {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates_and_names() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_794_510_652), "2026-11-12T19:10:52Z");
        assert_eq!(iso(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(minute("2026-11-12T19:10:52.697Z"), "2026-11-12 19:10 UTC");
        assert_eq!(slug(&TRAILING_DATE.replace("WoW: Forever Hotfixes - November 11, 2026", "")), "wow-forever-hotfixes");
        assert_eq!(slug("WoW Forever Beta Development Notes – Updated November 2"), "wow-forever-beta-development-notes-updated-novem");
        assert_eq!(slug("Correctifs dans World of Warcraft: Forever"), "correctifs-dans-world-of-warcraft-forever");
        assert!(is_rolling("WoW: Forever Hotfixes - November 11, 2026") && !is_rolling("Explore WoW: Forever with a Gamepad"));
        assert_eq!(locales_for("fr"), vec!["en-us", "fr-fr"]);
        assert_eq!(locales_for("en"), vec!["en-us"]);
    }

    #[test]
    fn parses_an_article_page() {
        let page = "<html><head><script type=\"application/ld+json\">{\"datePublished\":\"2026-11-12T00:46:04Z\",\"dateModified\":\"2026-11-12T01:00:00Z\"}</script></head>\
            <link rel=\"canonical\" href=\"https://news.blizzard.com/en-us/article/1/wow-forever-hotfixes-november-11-2026\"/>\
            <body><h1 class=\"blz-title-text-md\">WoW: Forever Hotfixes - November 11, 2026</h1>\
            <section class=\"blog\"><p>Intro.</p><hr /><p><strong>November 11, 2026</strong></p><ul><li>Volley fixed. See <a href=\"https://worldofwarcraft.blizzard.com/news/24307383\">this</a>.</li></ul></section></article></body></html>";
        let article = parse_article(page, "https://news.blizzard.com/en-us/article/1/x".into(), None).unwrap();
        assert_eq!(article.title, "WoW: Forever Hotfixes - November 11, 2026");
        assert_eq!(article.published, "2026-11-12T00:46:04Z");
        assert_eq!(article.url, "https://news.blizzard.com/en-us/article/1/wow-forever-hotfixes-november-11-2026");
        assert_eq!(article.links, vec![24307383]);
        assert!(article.body.starts_with("Intro.\n\n---\n\n### November 11, 2026\n\n- Volley fixed."), "{}", article.body);
    }

    #[test]
    fn builds_the_prompt_section_from_stored_documents() {
        let directory = std::env::temp_dir().join("mentor_knowledge_test");
        let _ = std::fs::remove_dir_all(&directory);
        let knowledge = Knowledge::open(&directory, directory.join("state.json"));
        std::fs::create_dir_all(&knowledge.directory).unwrap();
        let mut state = State::default();
        let mut report = SyncReport::default();
        let article = |title: &str, body: &str| Article {
            title: title.into(),
            url: "https://news.blizzard.com/en-us/article/24315281/x".into(),
            published: "2026-11-12T00:46:04Z".into(),
            modified: String::new(),
            body: body.into(),
            links: vec![24307383],
            etag: None,
        };
        knowledge.store_article(&mut state, 24315281, "en-us", article("WoW: Forever Hotfixes - November 11, 2026", "Intro.\n\n---\n\n### November 11, 2026\n\n- Volley fixed."), 100, &mut report).unwrap();
        knowledge.store_article(&mut state, 24315281, "fr-fr", article("Correctifs dans World of Warcraft: Forever - 9 novembre 2026", "Intro.\n\n---\n\n### 9 novembre 2026\n\n- Salve corrigée."), 100, &mut report).unwrap();
        state.topics.insert(
            7,
            Topic {
                title: "Class Tuning Incoming - November 10".into(),
                category: 347,
                fetched: 100,
                posts: [(
                    1,
                    Post { number: 1, author: "Kaivax".into(), role: "Community Manager".into(), created: "2026-11-06T21:11:00.000Z".into(), updated: "2026-11-06T21:11:00.000Z".into(), text: "Stormstrike damage reduced.".into() },
                )]
                .into_iter()
                .collect(),
            },
        );
        knowledge.store_topic(&mut state, 7, 100, &mut report).unwrap();
        assert_eq!(report.changes.len(), 2, "{:?}", report.changes);
        assert_eq!(report.changes[0].title, "WoW: Forever Hotfixes - November 11, 2026");
        // Storing the same text again is not a change.
        let mut second = SyncReport::default();
        knowledge.store_topic(&mut state, 7, 200, &mut second).unwrap();
        assert!(second.changes.is_empty());
        assert_eq!(state.documents["forum-7"].changed, 100);
        *knowledge.state.lock() = state;
        let section = knowledge.prompt_section();
        assert!(section.contains("newest first:\n  - news-24315281-wow-forever-hotfixes.en.md | WoW: Forever Hotfixes - November 11, 2026 | 2026-11-12 | 1 kB | +translation\n"), "{section}");
        assert!(section.contains("  - forum-7-class-tuning-incoming-november-10.md | Class Tuning Incoming - November 10 | 2026-11-06 | 1 kB\n"), "{section}");
        assert!(section.ends_with("### November 11, 2026\n\n- Volley fixed."), "{section}");
        let forum = std::fs::read_to_string(knowledge.directory.join("forum-7-class-tuning-incoming-november-10.md")).unwrap();
        assert!(forum.contains("## 2026-11-06 21:11 UTC - Kaivax (Community Manager)\n\nStormstrike damage reduced."), "{forum}");
        let overview = knowledge.overview("fr");
        assert_eq!(overview["documents"][0]["title"], "WoW: Forever Hotfixes - November 11, 2026");
    }

    /// Reads the real official sites; skipped silently without internet access.
    #[test]
    fn synchronizes_with_the_official_sites() {
        if std::env::var_os("MENTOR_ONLINE_TESTS").is_none() {
            return;
        }
        let directory = std::env::temp_dir().join("mentor_knowledge_online");
        let _ = std::fs::remove_dir_all(&directory);
        let knowledge = Knowledge::open(&directory, directory.join("state.json"));
        let started = std::time::Instant::now();
        let report = knowledge.sync("fr").unwrap();
        println!("first sync: {} documents, {} changes, {:?} in {:.0} s", report.total, report.changes.len(), report.errors, started.elapsed().as_secs_f32());
        assert!(report.first && report.total >= 10, "{report:?}");
        let section = knowledge.prompt_section();
        println!("{section}");
        assert!(section.contains("hotfixes.en.md") && section.contains("Latest official changes"));
        let started = std::time::Instant::now();
        let report = knowledge.sync("fr").unwrap();
        println!("second sync: {} changes, {:?} in {:.0} s", report.changes.len(), report.errors, started.elapsed().as_secs_f32());
        assert!(report.changes.len() <= 2, "{report:?}");
        assert!(started.elapsed().as_secs() < 20, "a sync with nothing new must be quick");
    }
}
