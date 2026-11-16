"use strict";

const TAURI = window.__TAURI__;
const invoke = TAURI.core.invoke;
const listen = TAURI.event.listen;

const $ = (id) => document.getElementById(id);
const state = {
  settings: null,
  setup: null,
  character: null,
  knowledge: null,
  devices: { inputs: [], outputs: [] },
  choices: { whisper: [], voices: [] },
  messages: new Map(),
  downloads: {},
  version: "",
};

const PHASES = {
  loading: "Chargement…",
  setup: "Configuration requise",
  ready: "Prêt",
  listening: "J'écoute…",
  transcribing: "Je transcris…",
  thinking: "Je réfléchis…",
  speaking: "Je parle",
};

const ACTION_LABELS = {
  push_to_talk: ["Parler", "Maintiens pour poser ta question"],
  push_to_talk_with_screenshot: ["Parler + capture", "Pareil, avec une capture d'écran jointe"],
  screenshot: ["Capture seule", "Un appui = capture jointe à ta prochaine question"],
  stop: ["Couper", "Coupe la voix et annule la réponse"],
  new_session: ["Nouvelle session", "Mentor oublie la conversation"],
};

const KEY_LABELS = {
  pause: "Pause", scroll_lock: "Arrêt défil", insert: "Inser", delete: "Suppr", home: "Début", end: "Fin",
  page_up: "Page préc.", page_down: "Page suiv.", caps_lock: "Verr. maj", print_screen: "Impr. écran",
  space: "Espace", enter: "Entrée", tab: "Tab", ctrl_l: "Ctrl gauche", ctrl_r: "Ctrl droit",
  alt_l: "Alt", alt_r: "Alt Gr", shift_l: "Maj gauche", shift_r: "Maj droite", menu: "Menu",
};

function bindingLabel(binding) {
  if (!binding) return "Aucune";
  const [device, name] = binding.split(":");
  if (device === "mouse") {
    return { x1: "Souris bouton 4", x2: "Souris bouton 5", middle: "Clic molette" }[name] || `Souris ${name}`;
  }
  if (/^f\d+$/.test(name)) return name.toUpperCase();
  if (/^vk\d+$/.test(name)) return `Touche ${name.slice(2)}`;
  if (/^num\d$/.test(name)) return `Pavé num. ${name.slice(3)}`;
  if (KEY_LABELS[name]) return KEY_LABELS[name];
  return name.length === 1 ? name.toUpperCase() : name;
}

function escapeHtml(text) {
  return String(text ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key === "html") node.innerHTML = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of [].concat(children)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function toast(text, level = "info", duration = 4200) {
  const node = el("div", { class: `toast ${level}`, text });
  $("toasts").append(node);
  setTimeout(() => node.remove(), duration);
}

async function call(command, args, { quiet = false } = {}) {
  try {
    return await invoke(command, args);
  } catch (error) {
    if (!quiet) toast(String(error), "error", 6500);
    throw error;
  }
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Copié.", "success", 1600);
  } catch {
    toast("Copie impossible.", "error");
  }
}

// ------------------------------------------------------------------ status & hints

function setStatus(phase) {
  $("status").dataset.phase = phase;
  $("status-label").textContent = PHASES[phase] || phase;
}

function updateHints() {
  if (!state.settings) return;
  const ptt = bindingLabel(state.settings.bindings.push_to_talk);
  $("empty-key").textContent = ptt;
  const parts = [`Maintiens ${ptt} pour parler`];
  if (state.settings.screenshot_auto) parts.push("capture auto si tu parles de ton écran");
  $("hint-ptt").textContent = parts.join(" · ");
  $("btn-pin").setAttribute("aria-pressed", String(!!state.settings.always_on_top));
}

// ------------------------------------------------------------------ rich text

function parseBlocks(text) {
  const blocks = [];
  const pattern = /```([^\n`]*)\n?([\s\S]*?)(```|$)/g;
  let last = 0;
  let match;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > last) blocks.push({ type: "text", text: text.slice(last, match.index) });
    blocks.push({ type: "code", info: match[1].trim(), body: match[2].replace(/\n+$/, ""), closed: match[3] === "```" });
    last = pattern.lastIndex;
    if (match[0].length === 0) break;
  }
  if (last < text.length) blocks.push({ type: "text", text: text.slice(last).replace(/`{1,2}$/, "") });
  return blocks;
}

function inline(text) {
  return escapeHtml(text)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
}

function renderText(text) {
  const fragment = document.createDocumentFragment();
  const paragraphs = text.split(/\n{2,}/).map((part) => part.trim()).filter(Boolean);
  for (const paragraph of paragraphs) {
    const lines = paragraph.split("\n");
    if (lines.every((line) => /^\s*([-*•]|\d+[.)])\s+/.test(line))) {
      const list = el("ul");
      for (const line of lines) list.append(el("li", { html: inline(line.replace(/^\s*([-*•]|\d+[.)])\s+/, "")) }));
      fragment.append(list);
    } else {
      fragment.append(el("p", { html: lines.map(inline).join("<br>") }));
    }
  }
  return fragment;
}

function utf8Length(text) {
  return new TextEncoder().encode(text).length;
}

function renderCode(block) {
  const info = block.info.toLowerCase();
  if (info.startsWith("macro")) {
    const name = block.info.slice(5).trim().slice(0, 16) || "Coach";
    const size = utf8Length(block.body);
    return el("div", { class: "card macro" }, [
      el("div", { class: "card-head" }, [
        el("span", { class: "tag", text: "Macro" }),
        el("span", { text: name }),
        el("span", { class: `count${size > 255 ? " over" : ""}`, text: `${size}/255` }),
      ]),
      el("pre", { text: block.body }),
      block.closed &&
        el("div", { class: "card-actions" }, [
          el("button", { class: "btn", text: "Copier", onclick: () => copy(block.body) }),
          el("button", {
            class: "btn primary",
            text: "Installer en jeu",
            disabled: size > 255,
            onclick: async () => {
              await call("send_macro", { name, body: block.body });
              toast(`Macro « ${name} » prête : tape /reload en jeu, puis ouvre /macro.`, "success", 7000);
            },
          }),
        ]),
    ]);
  }
  if (info.startsWith("plan")) {
    const steps = block.body
      .split("\n")
      .map((line) => line.replace(/^\s*([-*•]|\d+[.)]|\[[ xX]?\])\s*/, "").trim())
      .filter(Boolean);
    return el("div", { class: "card plan" }, [
      el("div", { class: "card-head" }, [el("span", { class: "tag", text: "Plan" }), el("span", { class: "count", text: `${steps.length} étapes` })]),
      el("ol", {}, steps.map((step) => el("li", { text: step }))),
      block.closed &&
        el("div", { class: "card-actions" }, [
          el("button", { class: "btn", text: "Copier", onclick: () => copy(steps.join("\n")) }),
          el("button", {
            class: "btn primary",
            text: "Envoyer au suivi en jeu",
            onclick: async () => {
              await call("send_plan", { steps });
              toast("Plan envoyé : tape /reload en jeu pour l'afficher.", "success", 7000);
            },
          }),
        ]),
    ]);
  }
  return el("div", { class: "card" }, [
    block.info && el("div", { class: "card-head" }, [el("span", { class: "tag", text: block.info })]),
    el("pre", { text: block.body }),
    block.closed && el("div", { class: "card-actions" }, [el("button", { class: "btn", text: "Copier", onclick: () => copy(block.body) })]),
  ]);
}

function renderRich(container, text) {
  container.replaceChildren();
  for (const block of parseBlocks(text)) {
    container.append(block.type === "text" ? renderText(block.text) : renderCode(block));
  }
}

// ------------------------------------------------------------------ chat

function scrollChat() {
  const chat = $("chat");
  requestAnimationFrame(() => (chat.scrollTop = chat.scrollHeight));
}

function micIcon() {
  return el("span", { html: '<svg viewBox="0 0 24 24"><path d="M12 15a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.9V21h2v-2.1A7 7 0 0 0 19 12h-2Z"/></svg>' });
}

function addMessage(message) {
  $("empty")?.remove();
  const chat = $("chat");
  if (message.role === "system") {
    const actions = asList(message.actions).map((action) =>
      el("button", { class: "btn", type: "button", text: action.label, onclick: () => call("ask_text", { text: action.ask, screenshot: false }) }),
    );
    chat.append(el("div", { class: "msg system" }, [el("span", { text: message.text }), actions.length > 0 && el("div", { class: "msg-actions" }, actions)]));
    scrollChat();
    return;
  }
  const bubble = el("div", { class: "bubble" });
  const meta = el("div", { class: "meta" });
  const node = el("div", { class: `msg ${message.role}` }, [message.image && el("img", { class: "shot", src: message.image, alt: "Capture d'écran envoyée" }), bubble, meta]);
  const entry = { node, bubble, meta, text: message.text || "", role: message.role };
  state.messages.set(message.id, entry);
  if (message.role === "user") {
    bubble.textContent = message.text;
    meta.append(micIcon(), message.auto_shot ? "capture auto jointe" : message.image ? "capture jointe" : "");
  } else if (message.pending) {
    bubble.append(el("span", { class: "typing" }, [el("span"), el("span"), el("span")]));
  } else {
    renderRich(bubble, entry.text);
  }
  chat.append(node);
  scrollChat();
}

function appendDelta({ id, text }) {
  const entry = state.messages.get(id);
  if (!entry) return;
  entry.text += text;
  renderRich(entry.bubble, entry.text);
  scrollChat();
}

function finishMessage(payload) {
  const entry = state.messages.get(payload.id);
  if (!entry) return;
  if (payload.error) {
    entry.node.classList.add("error");
    entry.bubble.textContent = payload.error;
  } else if (payload.interrupted) {
    if (!entry.text.trim()) entry.bubble.textContent = "(interrompu)";
    entry.meta.textContent = "interrompu";
  } else {
    if (payload.text && !entry.text.trim()) entry.text = payload.text;
    renderRich(entry.bubble, entry.text);
    const timing = payload.timing;
    if (timing) {
      const parts = [];
      if (timing.first_audio != null) parts.push(`voix ${timing.first_audio.toFixed(1)} s`);
      parts.push(`réponse ${timing.total.toFixed(1)} s`);
      entry.meta.textContent = parts.join(" · ");
    }
  }
  scrollChat();
}

// ------------------------------------------------------------------ character panel

const SLOT_LABELS = {
  Head: "Tête", Neck: "Cou", Shoulder: "Épaules", Shirt: "Chemise", Chest: "Torse", Waist: "Taille", Legs: "Jambes",
  Feet: "Pieds", Wrist: "Poignets", Hands: "Mains", Finger1: "Doigt 1", Finger2: "Doigt 2", Trinket1: "Bijou 1",
  Trinket2: "Bijou 2", Back: "Dos", MainHand: "Main droite", OffHand: "Main gauche", Ranged: "Distance", Tabard: "Tabard",
};

/** True when version a (like "1.4.1") is older than b. */
function versionBelow(a, b) {
  const left = String(a).split(".").map(Number);
  const right = String(b).split(".").map(Number);
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const difference = (left[index] || 0) - (right[index] || 0);
    if (difference) return difference < 0;
  }
  return false;
}

function asList(value) {
  return Array.isArray(value) ? value : [];
}
const list = asList;

function renderCharacter() {
  const body = $("character-body");
  body.replaceChildren();
  const reload = el("button", {
    class: "btn",
    text: "Recharger",
    onclick: async () => {
      const result = await call("load_character");
      if (result) {
        state.character = result;
        renderCharacter();
        toast("Personnage rechargé.", "success", 1800);
      }
    },
  });
  const character = state.character?.data;
  if (!character) {
    body.append(
      el("p", { text: "Aucun personnage chargé pour l'instant." }),
      el("ol", {}, [
        el("li", { text: "Installe l'addon Mentor (Réglages → Jeu)." }),
        el("li", { text: "Connecte-toi en jeu avec ton personnage." }),
        el("li", { html: "Tape <code>/mentor sync</code> dans le chat du jeu." }),
      ]),
      reload,
    );
    return;
  }
  const info = character.character || {};
  const location = character.location || {};
  body.append(
    el("div", {}, [
      el("div", { class: "char-title", text: `${info.name || "?"} · niveau ${info.level ?? "?"}` }),
      el("div", { class: "char-sub", text: `${info.race || ""} ${info.class || ""} · ${info.faction || ""} · ${info.realm || ""}` }),
      el("div", { class: "char-sub", text: `${location.zone || "?"}${location.subZone ? " / " + location.subZone : ""} · ${info.moneyText || ""}` }),
    ]),
  );
  const running = character.meta?.addonVersion;
  const bundled = state.setup?.wow?.addon_bundled;
  if (running && bundled && versionBelow(running, bundled)) {
    body.append(el("div", { class: "notice" }, [
      el("span", { class: "warn", text: `Cette fiche vient de l'ancienne version de l'addon (${running}). La nouvelle (${bundled}) n'a pas encore tourné en jeu.` }),
      el("span", { class: "help", html: "Tape <code>/mentor sync</code> en jeu. Si ce message reste, refais-le une deuxième fois : la première synchronisation après une mise à jour enregistre encore avec l'ancienne version." }),
    ]));
  }
  const talents = character.talents;
  if (talents && !asList(talents.trees).length) {
    body.append(el("div", { class: "warn", text: `Talents non exportés${talents.problem ? " : " + talents.problem : ""}${asList(talents.trace).length ? " (" + asList(talents.trace).join(", ") + ")" : ""}.` }));
  }
  if (talents && ((talents.trees && talents.trees.length) || talents.spec)) {
    const trees = list(talents.trees).map((tree) => el("span", { class: "pill", text: `${tree.name} ${tree.points ?? 0}` }));
    if (talents.spec) trees.unshift(el("span", { class: "pill", text: talents.spec }));
    if (talents.unspent) trees.push(el("span", { class: "pill q5", text: `${talents.unspent} point(s) à dépenser` }));
    body.append(el("div", {}, [el("h3", { class: "section-title", text: "Talents" }), el("div", { class: "pill-list" }, trees)]));
  }
  const gear = list(character.equipment).filter((entry) => entry.item);
  body.append(
    el("div", {}, [
      el("h3", { class: "section-title", text: "Équipement" }),
      el(
        "div",
        { class: "gear" },
        gear.map((entry) =>
          el("div", { class: "gear-row" }, [
            el("span", { class: "slot", text: SLOT_LABELS[entry.slot] || entry.slot }),
            el("span", { class: `name q${entry.item.quality ?? 1}`, text: entry.item.name || "?" }),
            el("span", { class: "ilvl", text: entry.item.itemLevel ?? "" }),
          ]),
        ),
      ),
    ]),
  );
  const skills = [...list(character.professions), ...list(character.skills).filter((skill) => /métier|profession/i.test(skill.category || ""))];
  if (skills.length) {
    body.append(
      el("div", {}, [
        el("h3", { class: "section-title", text: "Métiers" }),
        el("div", { class: "pill-list" }, skills.map((skill) => el("span", { class: "pill", text: `${skill.name} ${skill.skill ?? skill.rank ?? ""}/${skill.maxSkill ?? skill.maxRank ?? ""}` }))),
      ]),
    );
  }
  const details = (title, count, children) =>
    el("details", { class: "char-details" }, [el("summary", { text: `${title} (${count})` }), el("div", { class: "char-list" }, children)]);
  const spells = list(character.spells).filter((spell) => !spell.passive && !spell.notLearned);
  if (spells.length) {
    body.append(details("Grimoire", spells.length, spells.map((spell) => el("div", { text: `${spell.name}${spell.rank ? " (" + spell.rank + ")" : ""}${spell.cost ? " · " + spell.cost : ""}` }))));
  }
  const slots = list(character.actionBars?.slots);
  if (slots.length) {
    body.append(details("Barres d'action", slots.length, slots.map((slot) => el("div", {}, [el("kbd", { text: slot.key || "—" }), ` ${slot.name || "?"}`]))));
  }
  const pet = character.pet;
  if (pet && pet.name) {
    body.append(details(`Familier : ${pet.name}`, list(pet.spells).length, [
      el("div", { class: "char-sub", text: `${pet.family || ""} niveau ${pet.level ?? "?"}` }),
      ...list(pet.spells).map((spell) => el("div", { text: spell })),
    ]));
  }
  const recipes = character.recipes && typeof character.recipes === "object" ? Object.entries(character.recipes) : [];
  for (const [profession, entry] of recipes) {
    const list = asList(entry.recipes);
    body.append(details(`${profession} ${entry.rank ?? "?"}/${entry.maxRank ?? "?"}`, list.length, list.map((recipe) => el("div", { text: `${recipe.name}${recipe.difficulty ? " · " + recipe.difficulty : ""}` }))));
  }
  const bank = character.bank;
  if (bank && asList(bank.items).length) {
    body.append(details(`Banque (vue le ${bank.updatedText || "?"})`, asList(bank.items).length, asList(bank.items).map((item) => el("div", { class: `q${item.quality ?? 1}`, text: `${item.name || "?"}${item.count > 1 ? " x" + item.count : ""}` }))));
  }
  const reputations = list(character.reputations);
  if (reputations.length) {
    body.append(details("Réputations", reputations.length, reputations.map((rep) => el("div", { text: `${rep.name} : ${rep.standing || "?"}` }))));
  }
  if (!recipes.length) {
    body.append(el("div", { class: "char-sub", text: "Astuce : ouvre la fenêtre de chaque métier et ta banque une fois en jeu, l'addon retient les recettes et le contenu." }));
  }
  const bags = character.bags || {};
  const quests = list(character.quests);
  body.append(
    el("div", { class: "char-sub", text: `Sacs : ${bags.freeSlots ?? "?"} places libres sur ${bags.totalSlots ?? "?"} · ${quests.length} quête(s) · ${list(character.spells).length} sorts connus` }),
    el("div", {
      class: Date.now() / 1000 - (character.meta?.updatedAt || 0) > 6 * 3600 ? "warn" : "char-sub",
      text: `Exportée ${timeAgo(character.meta?.updatedAt)} (${character.meta?.updatedAtText || "?"}). Mentor reçoit cette fiche avec ta prochaine question.`,
    }),
    addonDisabledNotice(state.setup?.wow, renderCharacter),
    el("div", { class: "char-sub", html: "Pour actualiser : tape <code>/mentor sync</code> en jeu (recharge l'interface)." }),
    reload,
  );
}

// ------------------------------------------------------------------ knowledge panel

const DATE_FORMAT = new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short" });

function shortDate(iso) {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "" : DATE_FORMAT.format(date);
}

/** Date of the most recent official update in the knowledge base. */
function newestKnowledge() {
  return asList(state.knowledge?.documents).reduce((latest, document) => (document.updated > latest ? document.updated : latest), "");
}

function knowledgeSeen(value) {
  try {
    if (value !== undefined) localStorage.setItem("knowledge-seen", value);
    return localStorage.getItem("knowledge-seen") || "";
  } catch {
    return "";
  }
}

/** The dot on the button tells that Blizzard published something since the panel was last opened. */
function updateKnowledgeDot() {
  if (!$("knowledge-panel").hidden) knowledgeSeen(newestKnowledge());
  $("knowledge-dot").hidden = !state.knowledge?.enabled || !$("knowledge-panel").hidden || newestKnowledge() <= knowledgeSeen();
}

function knowledgeStatus(knowledge) {
  if (knowledge.syncing) return el("div", { class: "char-sub" }, [el("span", { class: "spin" }), "Synchronisation avec les sites officiels…"]);
  if (!knowledge.last_sync) return el("div", { class: "warn", text: knowledge.error || "Pas encore synchronisée." });
  const count = asList(knowledge.documents).length;
  const age = Date.now() / 1000 - knowledge.last_sync;
  return el("div", { class: age > 26 * 3600 ? "warn" : "ok", text: `${count} documents officiels · synchronisée ${timeAgo(knowledge.last_sync)}` });
}

function renderKnowledge() {
  const body = $("knowledge-body");
  const scroll = body.scrollTop;
  body.replaceChildren();
  const knowledge = state.knowledge || {};
  body.append(
    el("p", {
      class: "char-sub",
      text: "Mentor garde sur ton PC les notes officielles de Blizzard sur WoW Forever et les consulte avant de chercher sur le web. Elles se mettent à jour toutes les deux heures.",
    }),
  );
  if (!knowledge.enabled) {
    body.append(el("div", { class: "warn", text: "La base de connaissances est désactivée (Réglages → Jeu)." }));
    return;
  }
  const refresh = el("button", {
    class: "btn",
    type: "button",
    text: "Mettre à jour maintenant",
    disabled: !!knowledge.syncing,
    onclick: async () => {
      state.knowledge = { ...knowledge, syncing: true };
      renderKnowledge();
      try {
        const result = await call("sync_knowledge");
        state.knowledge = result.overview;
        const report = result.report;
        if (!report) toast("Une synchronisation est déjà en cours.", "info");
        else if (report.errors.length && !report.changes.length) toast(`Synchronisation incomplète : ${report.errors[0]}`, "warning", 7000);
        else toast(report.changes.length ? `${report.changes.length} document(s) mis à jour.` : "Tout est déjà à jour.", "success", 2500);
      } finally {
        if (state.knowledge) state.knowledge.syncing = false;
        renderKnowledge();
      }
    },
  });
  body.append(el("div", { class: "actions" }, [knowledgeStatus(knowledge), refresh]));
  if (knowledge.error && knowledge.last_sync) body.append(el("div", { class: "help", text: `Dernier souci : ${knowledge.error}` }));
  const documents = asList(knowledge.documents);
  const groups = [
    ["Correctifs et notes de mise à jour", documents.filter((document) => document.kind === "news" && document.rolling)],
    ["Annonces et réponses des développeurs (forum officiel)", documents.filter((document) => document.kind === "forum")],
    ["Guides et articles officiels", documents.filter((document) => document.kind === "news" && !document.rolling)],
  ];
  for (const [title, list] of groups) {
    if (!list.length) continue;
    const rows = list.map((document) =>
      el("button", { class: "doc", type: "button", title: "Ouvrir dans le navigateur", onclick: () => call("open_url", { url: document.url }) }, [
        el("span", { class: "doc-title" }, [document.title, document.fresh && el("span", { class: "badge", text: "nouveau" })]),
        el("span", { class: "doc-date", text: shortDate(document.updated) }),
      ]),
    );
    const visible = 8;
    const content = [el("h3", { class: "section-title", text: `${title} (${list.length})` }), el("div", { class: "doc-list" }, rows.slice(0, visible))];
    if (rows.length > visible) {
      content.push(el("details", { class: "char-details" }, [el("summary", { text: `Voir les ${rows.length - visible} autres` }), el("div", { class: "doc-list" }, rows.slice(visible))]));
    }
    body.append(el("div", {}, content));
  }
  if (documents.length) {
    body.append(
      el("button", {
        class: "btn primary",
        type: "button",
        text: "Qu'est-ce qui change pour moi ?",
        onclick: () =>
          call("ask_text", {
            text: "Qu'est-ce qui change pour mon personnage dans les dernières notes officielles de Blizzard (correctifs et annonces des développeurs) ? Résume en quelques phrases.",
            screenshot: false,
          }),
      }),
    );
  }
  requestAnimationFrame(() => (body.scrollTop = scroll));
}

function toggleDrawer(id, render) {
  const panel = $(id);
  const open = panel.hidden;
  for (const other of ["character-panel", "knowledge-panel"]) $(other).hidden = true;
  panel.hidden = !open;
  if (open) render();
  updateKnowledgeDot();
}

// ------------------------------------------------------------------ settings

let saveTimer = null;
function saveSettings(immediate = false) {
  clearTimeout(saveTimer);
  const run = async () => {
    await call("save_settings", { settings: state.settings });
    updateHints();
  };
  if (immediate) return run();
  saveTimer = setTimeout(run, 350);
}

function setSetting(key, value, immediate) {
  state.settings[key] = value;
  saveSettings(immediate);
}

function switchControl(key, onChange) {
  const input = el("input", { type: "checkbox" });
  input.checked = !!state.settings[key];
  input.addEventListener("change", () => {
    setSetting(key, input.checked, true);
    onChange?.(input.checked);
  });
  return el("label", { class: "switch" }, [input, el("span")]);
}

function selectControl(key, options, onChange) {
  const select = el(
    "select",
    { onchange: (event) => { setSetting(key, event.target.value, true); onChange?.(event.target.value); } },
    options.map(([value, label]) => el("option", { value, text: label })),
  );
  select.value = state.settings[key];
  return select;
}

function row(label, help, control) {
  const wide = control?.tagName === "TEXTAREA" || control?.tagName === "INPUT" || control?.classList?.contains("wide-control");
  return el("div", { class: `row${wide ? " wide" : ""}` }, [el("div", { class: "label" }, [label, help && el("span", { class: "help", text: help })]), control]);
}

let capturing = false;

function bindButton(action) {
  const button = el("button", { class: "btn bind-btn", text: bindingLabel(state.settings.bindings[action]) });
  button.addEventListener("click", async () => {
    if (capturing) return;
    capturing = true;
    button.classList.add("waiting");
    button.textContent = "Appuie sur une touche…";
    try {
      const name = await call("capture_binding");
      if (name !== null && name !== undefined) {
        for (const other of Object.keys(state.settings.bindings)) {
          if (other !== action && name && state.settings.bindings[other] === name) state.settings.bindings[other] = "";
        }
        state.settings.bindings[action] = name;
        await saveSettings(true);
      }
    } finally {
      capturing = false;
      button.classList.remove("waiting");
      button.textContent = bindingLabel(state.settings.bindings[action]);
      if (!$("settings").hidden) renderSettings();
      if (!$("wizard").hidden) renderWizard();
    }
  });
  return button;
}

async function refreshSetup() {
  state.setup = await call("get_setup", undefined, { quiet: true }).catch(() => state.setup);
  return state.setup;
}

function usesApiKey() {
  return state.settings?.auth_mode === "api_key";
}

function claudeReady(setup) {
  if (!setup?.claude?.installed) return false;
  return usesApiKey() ? !!setup.api_key_set : !!setup.claude.logged_in;
}

function claudeStatusLine(setup) {
  const claude = setup?.claude || {};
  if (!claude.installed) return el("span", { class: "bad", text: "Claude Code n'est pas installé" });
  if (usesApiKey()) {
    return setup.api_key_set
      ? el("span", { class: "ok", text: "Clé API enregistrée · paiement à l'usage" })
      : el("span", { class: "warn", text: "Aucune clé API enregistrée" });
  }
  if (!claude.logged_in) return el("span", { class: "warn", text: "Pas connecté" });
  const plan = claude.plan ? ` · ${claude.plan}` : "";
  return el("span", { class: "ok", text: `Connecté${claude.email ? " : " + claude.email : ""}${plan}` });
}

function authModeControl(onChange) {
  const option = (mode, label) =>
    el("button", {
      class: `seg${state.settings.auth_mode === mode ? " active" : ""}`,
      type: "button",
      "aria-pressed": String(state.settings.auth_mode === mode),
      text: label,
      onclick: async () => {
        if (state.settings.auth_mode === mode) return;
        state.settings.auth_mode = mode;
        await saveSettings(true);
        await refreshSetup();
        onChange();
      },
    });
  return el("div", { class: "segmented", role: "group", "aria-label": "Mode de connexion" }, [
    option("subscription", "Abonnement Claude"),
    option("api_key", "Clé API"),
  ]);
}

function apiKeyForm(onChange) {
  const input = el("input", { type: "password", placeholder: "sk-ant-…", autocomplete: "off", spellcheck: "false" });
  const save = el("button", {
    class: "btn primary",
    type: "button",
    text: state.setup?.api_key_set ? "Remplacer" : "Valider",
    onclick: async () => {
      if (!input.value.trim()) return input.focus();
      save.disabled = true;
      save.textContent = "Vérification…";
      try {
        await call("set_api_key", { key: input.value });
        input.value = "";
        toast("Clé API vérifiée et enregistrée.", "success");
        await refreshSetup();
        onChange();
      } catch {
        save.disabled = false;
        save.textContent = "Valider";
      }
    },
  });
  input.addEventListener("keydown", (event) => event.key === "Enter" && save.click());
  return el("div", { class: "api-key" }, [
    el("div", { class: "actions" }, [input, save]),
    el("div", { class: "actions" }, [
      el("button", { class: "link-btn", type: "button", text: "Créer une clé sur la console Anthropic", onclick: () => call("open_url", { url: "https://console.anthropic.com/settings/keys" }) }),
      state.setup?.api_key_set &&
        el("button", {
          class: "link-btn",
          type: "button",
          text: "Supprimer la clé",
          onclick: async () => {
            await call("clear_api_key");
            state.settings.auth_mode = "subscription";
            await refreshSetup();
            onChange();
          },
        }),
    ]),
    el("p", {
      class: "help",
      text: "Facturé à l'usage sur ton compte Anthropic : quelques centimes par question, recherches web en plus. Fixe une limite de dépenses sur la console. La clé est chiffrée avec ton compte Windows.",
    }),
  ]);
}

function addonDisabledNotice(wow, onChange) {
  if (!wow?.addon_disabled?.length) return null;
  return el("div", { class: "notice" }, [
    el("span", { class: "warn", text: `L'addon Mentor est désactivé dans le jeu pour : ${wow.addon_disabled.join(", ")}. Ta fiche ne se met plus à jour.` }),
    wow.game_running
      ? el("span", { class: "help", text: "Ferme le jeu pour l'activer d'ici, ou coche Mentor dans le bouton AddOns de l'écran de sélection du personnage." })
      : el("button", {
          class: "btn primary",
          type: "button",
          text: "Activer l'addon",
          onclick: async () => {
            const count = await call("enable_addon");
            toast(`Addon Mentor activé pour ${count} personnage(s). Lance le jeu, puis tape /mentor sync.`, "success", 7000);
            await refreshSetup();
            onChange?.();
          },
        }),
  ]);
}

function timeAgo(epochSeconds) {
  if (!epochSeconds) return "date inconnue";
  const minutes = Math.max(0, Math.round((Date.now() / 1000 - epochSeconds) / 60));
  if (minutes < 1) return "à l'instant";
  if (minutes < 60) return `il y a ${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `il y a ${hours} h`;
  const days = Math.round(hours / 24);
  return `il y a ${days} jour${days > 1 ? "s" : ""}`;
}

function renderSettings() {
  const s = state.settings;
  const setup = state.setup || {};
  const wow = setup.wow || {};
  const body = $("settings-body");
  body.replaceChildren();

  const claudeActions = el("div", { class: "actions" });
  if (!setup.claude?.installed) claudeActions.append(el("button", { class: "btn primary", text: "Installer", onclick: () => call("install_claude") }));
  else if (!usesApiKey() && !setup.claude?.logged_in) claudeActions.append(el("button", { class: "btn primary", text: "Se connecter", onclick: () => call("login_claude") }));
  else if (!usesApiKey()) claudeActions.append(el("button", { class: "btn danger", text: "Se déconnecter", onclick: async () => { await call("logout_claude"); await refreshSetup(); renderSettings(); } }));
  claudeActions.append(el("button", { class: "btn", text: "Vérifier", onclick: async () => { await refreshSetup(); renderSettings(); } }));

  body.append(
    el("section", { class: "group" }, [
      el("h3", { text: "Compte Claude" }),
      row("Connexion", "Abonnement Pro ou Max, ou clé API payée à l'usage.", authModeControl(renderSettings)),
      row("Statut", usesApiKey() ? null : "Chaque joueur utilise son propre abonnement Claude.", el("div", { class: "control-right" }, [claudeStatusLine(setup), claudeActions])),
      usesApiKey() && el("div", { class: "row wide" }, [el("div", { class: "label" }, ["Clé API"]), apiKeyForm(renderSettings)]),
      row("Modèle", "Sonnet : rapide. Opus : plus fort, plus lent.", selectControl("claude_model", [["sonnet", "Sonnet (conseillé)"], ["opus", "Opus"], ["haiku", "Haiku (le plus rapide)"]])),
      row("Effort de réflexion", "Bas = réponses plus rapides.", selectControl("claude_effort", [["low", "Bas"], ["medium", "Moyen"], ["high", "Élevé"], ["", "Par défaut"]])),
    ]),
    el("section", { class: "group" }, [
      el("h3", { text: "Touches" }),
      ...Object.keys(ACTION_LABELS).map((action) => row(ACTION_LABELS[action][0], ACTION_LABELS[action][1], bindButton(action))),
      row("Couper la voix quand je reparle", null, switchControl("interrupt_on_push_to_talk")),
      el("p", { class: "help", text: "Échap annule, Retour arrière efface. Les boutons latéraux de la souris marchent directement. Choisis une touche que le jeu n'utilise pas." }),
    ]),
    el("section", { class: "group" }, [
      el("h3", { text: "Audio" }),
      row("Micro", null, selectControl("input_device", [["", "Par défaut de Windows"], ...state.devices.inputs.map((name) => [name, name])])),
      row("Sortie", null, selectControl("output_device", [["", "Par défaut de Windows"], ...state.devices.outputs.map((name) => [name, name])])),
      row("Volume général", null, rangeControl("volume", 0, 2, 0.05)),
      row("Lire les réponses à voix haute", null, switchControl("speak_answers")),
      row("Petits bips", "Début et fin d'écoute, erreurs.", switchControl("beeps")),
      row("Volume des bips", "Un bip de test est joué quand tu bouges le curseur.", rangeControl("beep_volume", 0, 2, 0.05, testBeepSoon)),
      row("Voix", null, selectControl("voice", state.choices.voices.map((voice) => [voice.id, voice.label]), () => maybeDownload())),
      row("Vitesse de la voix", null, rangeControl("voice_speed", 0.7, 1.6, 0.05)),
    ]),
    el("section", { class: "group" }, [
      el("h3", { text: "Reconnaissance vocale" }),
      row("Précision", null, selectControl("whisper_model", state.choices.whisper.map((model) => [model.id, `${model.label} · ${model.size_mb} Mo`]), () => maybeDownload())),
      row("Utiliser la carte graphique", "Désactive si Mentor plante au démarrage.", switchControl("use_gpu")),
      row("Vocabulaire du jeu", "Aide à reconnaître les noms propres.", textControl("initial_prompt", true)),
      setup.engines?.stt && el("p", { class: "help", text: `Actif : ${setup.engines.stt}` }),
    ]),
    el("section", { class: "group" }, [
      el("h3", { text: "Captures d'écran" }),
      row("Capture automatique", "Envoyée seulement si ta question parle de l'écran.", switchControl("screenshot_auto", updateHints)),
      row("Mots déclencheurs", "Séparés par des virgules.", keywordsControl()),
    ]),
    el("section", { class: "group" }, [
      el("h3", { text: "Jeu" }),
      row("Dossier du jeu", wow.flavor ? `Version ${wow.flavor.version || "?"} (interface ${wow.flavor.interface ?? "?"})` : "Le dossier _forever_ de World of Warcraft", wowFolderControl()),
      row(
        "Addon Mentor",
        wow.addon_installed ? `Installé : v${wow.addon_installed}${wow.addon_outdated ? ` (nouvelle version ${wow.addon_bundled})` : ""}` : "Pas installé",
        el("button", {
          class: `btn${!wow.addon_installed || wow.addon_outdated ? " primary" : ""}`,
          text: wow.addon_installed ? (wow.addon_outdated ? "Mettre à jour" : "Réinstaller") : "Installer",
          disabled: !wow.found,
          onclick: installAddon,
        }),
      ),
      addonDisabledNotice(wow, renderSettings),
      row("Envoyer ma fiche au coach", "Classe, talents, équipement, sacs, sorts et macros.", switchControl("share_character")),
      row(
        "Base de connaissances officielle",
        "Correctifs, annonces des développeurs et guides de Blizzard, gardés sur ton PC et mis à jour toutes les deux heures.",
        switchControl("knowledge_enabled", async () => {
          state.knowledge = await call("get_knowledge", undefined, { quiet: true }).catch(() => state.knowledge);
          updateKnowledgeDot();
        }),
      ),
    ]),
    el("section", { class: "group" }, [
      el("h3", { text: "Divers" }),
      row("Langue du coach", null, selectControl("language", [["fr", "Français"], ["en", "English"]])),
      row("Jeu", null, textControl("game")),
      row("Indicateur par-dessus le jeu", "Micro animé quand tu parles, chargement, puis réponse. Les clics le traversent.", switchControl("overlay_enabled")),
      row("Position de l'indicateur", null, selectControl("overlay_position", [["top", "En haut au centre"], ["top-left", "En haut à gauche"], ["top-right", "En haut à droite"], ["bottom", "En bas au centre"]])),
      row("Toujours au premier plan", "La fenêtre de Mentor elle-même.", switchControl("always_on_top", (value) => TAURI.window.getCurrentWindow().setAlwaysOnTop(value))),
      row("Dossier des données", `Mentor ${state.version}`, el("button", { class: "btn", text: "Ouvrir", onclick: () => call("open_data_folder") })),
      row("Assistant de démarrage", null, el("button", { class: "btn", text: "Relancer", onclick: () => { closeOverlay("settings"); openWizard(); } })),
    ]),
  );
}

function rangeControl(key, min, max, step, onChange) {
  const format = (value) => `${Math.round(Number(value) * 100)} %`;
  const output = el("span", { class: "help", text: format(state.settings[key]) });
  const input = el("input", { type: "range", min, max, step, value: state.settings[key] });
  input.addEventListener("input", () => {
    output.textContent = format(input.value);
    setSetting(key, Number(input.value));
    onChange?.(Number(input.value));
  });
  return el("div", { class: "range" }, [input, output]);
}

let beepTimer = null;
function testBeepSoon() {
  clearTimeout(beepTimer);
  beepTimer = setTimeout(async () => {
    await saveSettings(true);
    call("test_beep", undefined, { quiet: true }).catch(() => {});
  }, 250);
}

function textControl(key, multiline = false) {
  const input = multiline ? el("textarea") : el("input", { type: "text" });
  input.value = state.settings[key] || "";
  input.addEventListener("change", () => setSetting(key, input.value.trim(), true));
  return input;
}

function keywordsControl() {
  const input = el("textarea");
  input.value = state.settings.screenshot_keywords.join(", ");
  input.addEventListener("change", () =>
    setSetting("screenshot_keywords", input.value.split(",").map((part) => part.trim()).filter(Boolean), true),
  );
  return input;
}

function wowFolderControl() {
  const input = el("input", { type: "text", value: state.settings.wow_folder || "" });
  input.addEventListener("change", async () => {
    setSetting("wow_folder", input.value.trim(), true);
    await refreshSetup();
    renderSettings();
  });
  const detect = el("button", {
    class: "btn",
    text: "Détecter",
    onclick: async () => {
      const folder = await call("detect_wow");
      if (folder) {
        await setSetting("wow_folder", folder, true);
        await refreshSetup();
        renderSettings();
      } else toast("World of Warcraft introuvable : indique le dossier _forever_.", "warning");
    },
  });
  const browse = el("button", {
    class: "btn",
    text: "Parcourir",
    onclick: async () => {
      const folder = await invoke("plugin:dialog|open", { options: { directory: true, title: "Dossier _forever_ de WoW" } }).catch(() => null);
      if (folder) {
        await setSetting("wow_folder", folder, true);
        await refreshSetup();
        renderSettings();
      }
    },
  });
  return el("div", { class: "actions wide-control" }, [input, detect, browse]);
}

async function installAddon() {
  const version = await call("install_addon");
  toast(`Addon Mentor v${version} installé. Relance le jeu (ou /reload) et tape /mentor sync.`, "success", 8000);
  await refreshSetup();
  if (!$("settings").hidden) renderSettings();
  if (!$("wizard").hidden) renderWizard();
}

async function maybeDownload() {
  await refreshSetup();
  if (!state.setup.models.whisper || !state.setup.models.voice) {
    toast("Téléchargement du nouveau modèle…", "info");
    call("download_models");
  }
}

// ------------------------------------------------------------------ wizard

let wizardPoll = null;

function downloadPercent(info) {
  return info.total ? Math.round((info.done / info.total) * 100) : info.state === "done" ? 100 : 0;
}

function progressLabel(kind, info) {
  return `${kind === "whisper" ? "Reconnaissance vocale" : "Voix"} : ${info.state === "done" ? "terminé" : downloadPercent(info) + " %"}`;
}

function progressBar(kind) {
  const info = state.downloads[kind];
  if (!info) return null;
  if (info.state === "error") return el("div", { class: "bad", text: info.error });
  return el("div", { id: `progress-${kind}` }, [
    el("div", { class: "help", text: progressLabel(kind, info) }),
    el("div", { class: "progress" }, el("div", { style: `width:${downloadPercent(info)}%` })),
  ]);
}

/** Updates a progress bar in place; returns false when the wizard needs a full render. */
function updateProgress(kind) {
  const info = state.downloads[kind];
  const node = $(`progress-${kind}`);
  if (!info || !node || info.state !== "progress") return false;
  node.firstChild.textContent = progressLabel(kind, info);
  node.querySelector(".progress > div").style.width = `${downloadPercent(info)}%`;
  return true;
}

let wizardSignature = "";
function wizardStateSignature() {
  return JSON.stringify([state.setup, state.settings?.bindings?.push_to_talk, Object.values(state.downloads).map((info) => info.state)]);
}

/** Re-renders the wizard only when something changed and the player is not interacting with it. */
function renderWizardIfChanged() {
  if ($("wizard").hidden || capturing) return;
  const active = document.activeElement;
  if (active && $("wizard").contains(active) && ["SELECT", "INPUT", "TEXTAREA"].includes(active.tagName)) return;
  if (wizardStateSignature() === wizardSignature) return;
  renderWizard();
}

function step(number, done, title, description, content) {
  return el("div", { class: `step${done ? " done" : ""}` }, [
    el("div", { class: "step-num", text: done ? "✓" : number }),
    el("div", {}, [el("h4", { text: title }), description && el("p", { text: description }), content]),
  ]);
}

function renderWizard() {
  const setup = state.setup || {};
  const claude = setup.claude || {};
  const engines = setup.engines || {};
  const wow = setup.wow || {};
  const modelsDone = setup.models?.whisper && setup.models?.voice;
  const body = $("wizard-body");
  const scroll = body.scrollTop;
  wizardSignature = wizardStateSignature();
  body.replaceChildren();
  requestAnimationFrame(() => (body.scrollTop = scroll));

  const claudeContent = el("div", { class: "actions" }, [authModeControl(renderWizard)]);
  if (!claude.installed) {
    claudeContent.append(el("div", { class: "actions full" }, [
      el("button", { class: "btn primary", text: "Installer Claude Code", onclick: () => call("install_claude") }),
      el("span", { class: "help", text: "Une fenêtre s'ouvre, laisse-la finir." }),
    ]));
  } else if (usesApiKey()) {
    claudeContent.append(el("div", { class: "full" }, [claudeStatusLine(setup), apiKeyForm(renderWizard)]));
  } else if (!claude.logged_in) {
    claudeContent.append(el("div", { class: "actions full" }, [
      el("button", { class: "btn primary", text: "Se connecter à Claude", onclick: () => call("login_claude") }),
      el("span", { class: "help", text: "Avec ton compte claude.ai (Pro ou Max), dans le navigateur." }),
    ]));
  } else {
    claudeContent.append(el("div", { class: "full" }, [claudeStatusLine(setup)]));
  }
  if (!setup.git) {
    claudeContent.append(el("div", { class: "warn", style: "flex-basis:100%" }, ["Git pour Windows est recommandé par Claude Code. ", el("button", { class: "link-btn", text: "L'installer", onclick: () => call("install_git") })]));
  }

  const modelContent = el("div", {}, [
    el("div", { class: "actions" }, [
      selectControl("whisper_model", state.choices.whisper.map((model) => [model.id, `${model.label} · ${model.size_mb} Mo`])),
      selectControl("voice", state.choices.voices.map((voice) => [voice.id, voice.label])),
    ]),
    el("div", { class: "actions", style: "margin-top:8px" }, [
      modelsDone
        ? el("span", { class: engines.ready ? "ok" : "warn", text: engines.ready ? `Prêt : ${engines.stt || ""}` : engines.loading ? "Chargement…" : engines.error ? `Erreur : ${engines.error}` : "Téléchargé" })
        : el("button", { class: "btn primary", text: "Télécharger", onclick: () => { state.downloads = {}; call("download_models"); } }),
      modelsDone && !engines.ready && !engines.loading && el("button", { class: "btn", text: "Réessayer", onclick: () => call("load_engines") }),
    ]),
    progressBar("whisper"),
    progressBar("voice"),
  ]);

  const wowContent = el("div", { class: "actions" });
  if (!wow.found) {
    wowContent.append(wowFolderControl());
  } else if (!wow.addon_installed || wow.addon_outdated) {
    wowContent.append(el("button", { class: "btn primary", text: wow.addon_installed ? "Mettre à jour l'addon" : "Installer l'addon", onclick: installAddon }), el("span", { class: "help", text: wow.folder }));
  } else {
    wowContent.append(el("span", { class: "ok", text: `Addon v${wow.addon_installed} installé · tape /mentor sync en jeu` }));
  }
  const disabledNotice = addonDisabledNotice(wow, renderWizard);
  if (disabledNotice) wowContent.append(disabledNotice);

  const claudeDone = claudeReady(setup);
  const ready = claudeDone && modelsDone;
  body.append(
    el("p", { class: "help", text: "Quatre étapes, une seule fois. Tout tourne sur ton PC, sauf les questions envoyées à Claude avec ton propre compte." }),
    step(1, claudeDone, "Ton compte Claude", "Avec un abonnement Pro ou Max, rien à payer en plus. Sans abonnement, une clé API facturée à l'usage. Le plan gratuit ne suffit pas.", claudeContent),
    step(2, modelsDone && engines.ready, "Voix et reconnaissance vocale", "Téléchargées une fois (environ 600 Mo en mode précis), puis tout fonctionne en local.", modelContent),
    step(3, !!state.settings.bindings.push_to_talk, "Ta touche pour parler", "Un bouton latéral de souris est idéal. Maintiens-le pendant que tu parles.", el("div", { class: "actions" }, [bindButton("push_to_talk")])),
    step(4, wow.addon_installed && !wow.addon_outdated, "Addon WoW (conseillé)", "Permet au coach de connaître ton personnage, ton équipement et d'installer des macros en jeu.", wowContent),
    el("div", { class: "wizard-foot" }, [
      el("button", {
        class: "btn primary",
        text: ready ? "C'est parti" : "Terminer plus tard",
        onclick: async () => {
          state.settings.onboarded = true;
          await saveSettings(true);
          closeOverlay("wizard");
        },
      }),
    ]),
  );
}

function openWizard() {
  $("wizard").hidden = false;
  renderWizard();
  clearInterval(wizardPoll);
  wizardPoll = setInterval(async () => {
    if ($("wizard").hidden) return clearInterval(wizardPoll);
    if (capturing) return;
    await refreshSetup();
    renderWizardIfChanged();
  }, 3000);
}

function closeOverlay(id) {
  $(id).hidden = true;
  if (id === "wizard") clearInterval(wizardPoll);
}

// ------------------------------------------------------------------ wiring

function wireUi() {
  $("composer").addEventListener("submit", async (event) => {
    event.preventDefault();
    const input = $("input");
    const text = input.value.trim();
    if (!text) return;
    input.value = "";
    input.style.height = "";
    await call("ask_text", { text, screenshot: false });
    $("armed").hidden = true;
  });
  $("input").addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      $("composer").requestSubmit();
    }
  });
  $("input").addEventListener("input", (event) => {
    event.target.style.height = "auto";
    event.target.style.height = `${Math.min(event.target.scrollHeight, 140)}px`;
  });
  $("btn-shot").addEventListener("click", () => call("take_screenshot"));
  $("armed-clear").addEventListener("click", () => {
    call("clear_screenshot");
    $("armed").hidden = true;
  });
  $("btn-stop").addEventListener("click", () => call("stop"));
  $("btn-new-session").addEventListener("click", () => call("new_session"));
  $("btn-settings").addEventListener("click", async () => {
    $("settings").hidden = false;
    renderSettings();
    const before = JSON.stringify(state.setup);
    await refreshSetup();
    const active = document.activeElement;
    const busy = capturing || (active && $("settings").contains(active) && ["SELECT", "INPUT", "TEXTAREA"].includes(active.tagName));
    if (!$("settings").hidden && !busy && JSON.stringify(state.setup) !== before) renderSettings();
  });
  $("btn-character").addEventListener("click", () => toggleDrawer("character-panel", renderCharacter));
  $("btn-knowledge").addEventListener("click", () => toggleDrawer("knowledge-panel", renderKnowledge));
  $("btn-pin").addEventListener("click", async () => {
    const value = !state.settings.always_on_top;
    await TAURI.window.getCurrentWindow().setAlwaysOnTop(value);
    setSetting("always_on_top", value, true);
    updateHints();
  });
  document.querySelectorAll("[data-close]").forEach((button) =>
    button.addEventListener("click", () => {
      closeOverlay(button.dataset.close);
      updateKnowledgeDot();
    }),
  );
  $("settings").addEventListener("click", (event) => {
    if (event.target.id === "settings") closeOverlay("settings");
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !$("settings").hidden) closeOverlay("settings");
  });
}

async function wireEvents() {
  await listen("status", (event) => setStatus(event.payload.phase));
  await listen("chat", (event) => addMessage(event.payload));
  await listen("chat_delta", (event) => appendDelta(event.payload));
  await listen("chat_done", (event) => finishMessage(event.payload));
  await listen("notice", (event) => toast(event.payload.text, event.payload.level));
  await listen("armed", (event) => {
    $("armed-img").src = event.payload.thumbnail;
    $("armed").hidden = false;
  });
  await listen("download", (event) => {
    state.downloads[event.payload.kind] = event.payload;
    if (event.payload.state === "error") toast(`Téléchargement : ${event.payload.error}`, "error", 8000);
    if (!updateProgress(event.payload.kind)) renderWizardIfChanged();
  });
  await listen("engines", async (event) => {
    if (state.setup) state.setup.engines = event.payload;
    if (event.payload.error && !event.payload.loading && event.payload.error !== "modèles à télécharger") toast(`Démarrage du coach : ${event.payload.error}`, "error", 8000);
    await refreshSetup();
    renderWizardIfChanged();
  });
  await listen("knowledge_syncing", () => {
    if (!state.knowledge) return;
    state.knowledge.syncing = true;
    if (!$("knowledge-panel").hidden) renderKnowledge();
  });
  await listen("knowledge", (event) => {
    state.knowledge = event.payload;
    if (!$("knowledge-panel").hidden) renderKnowledge();
    updateKnowledgeDot();
  });
  await listen("character", (event) => {
    const isNew = !state.character || state.character.data?.meta?.updatedAt !== event.payload.data?.meta?.updatedAt;
    state.character = event.payload;
    if (!$("character-panel").hidden) renderCharacter();
    if (isNew) {
      const info = event.payload.data?.character || {};
      toast(`Personnage chargé : ${info.name || "?"}, niveau ${info.level ?? "?"}.`, "success", 2500);
    }
  });
}

async function init() {
  wireUi();
  await wireEvents();
  const initial = await call("get_state");
  state.settings = initial.settings;
  state.setup = initial.setup;
  state.character = initial.character;
  state.knowledge = initial.knowledge;
  updateKnowledgeDot();
  state.devices = initial.devices;
  state.choices = initial.choices;
  state.version = initial.version;
  setStatus(initial.phase);
  updateHints();
  const needsSetup = !state.settings.onboarded || !claudeReady(state.setup) || !state.setup.models?.whisper || !state.setup.models?.voice;
  if (!needsSetup && state.setup.wow?.addon_disabled?.length) {
    toast("L'addon Mentor est désactivé dans le jeu : ta fiche ne se met plus à jour. Réglages → Jeu.", "warning", 9000);
  }
  if (needsSetup) openWizard();
}

init().catch((error) => toast(`Erreur au démarrage : ${error}`, "error", 10000));
