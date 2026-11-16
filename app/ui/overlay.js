"use strict";

const listen = window.__TAURI__.event.listen;
const pill = document.getElementById("pill");
const label = document.getElementById("label");
const bars = [...document.querySelectorAll(".bars i")];
const BAR_SHAPE = [0.55, 0.85, 1, 0.8, 0.6];

const LABELS = {
  listening: "J'écoute…",
  transcribing: "Je transcris…",
  thinking: "Je réfléchis…",
  speaking: "Mentor répond",
};

let phase = "idle";
let noticeTimer = null;
let smoothed = 0;

function show(nextPhase, text) {
  pill.dataset.phase = nextPhase;
  label.textContent = text || LABELS[nextPhase] || "";
}

function applyPhase(nextPhase) {
  phase = nextPhase;
  if (noticeTimer) return;
  show(LABELS[phase] ? phase : "idle");
  if (phase !== "listening") {
    smoothed = 0;
    document.documentElement.style.setProperty("--level", 0);
  }
}

listen("status", (event) => applyPhase(event.payload.phase));

listen("mic_level", (event) => {
  // RMS of normal speech is roughly 0.01 to 0.2: spread it over 0..1.
  const target = Math.min(1, Math.sqrt(event.payload.level || 0) * 2.4);
  smoothed = target > smoothed ? target : smoothed * 0.7 + target * 0.3;
  document.documentElement.style.setProperty("--level", smoothed.toFixed(3));
  bars.forEach((bar, index) => {
    const jitter = 0.75 + Math.random() * 0.5;
    bar.style.height = `${Math.round(6 + smoothed * 20 * BAR_SHAPE[index] * jitter)}px`;
  });
});

listen("notice", (event) => {
  const level = event.payload.level;
  if (level !== "warning" && level !== "error") return;
  clearTimeout(noticeTimer);
  show("notice", event.payload.text.length > 42 ? `${event.payload.text.slice(0, 40)}…` : event.payload.text);
  noticeTimer = setTimeout(() => {
    noticeTimer = null;
    applyPhase(phase);
  }, 2600);
});
