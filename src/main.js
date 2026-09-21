import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ParticleOrb } from "./lib/particles.js";
import { routeFast } from "./lib/router.js";

const $ = (id) => document.getElementById(id);
const settingsKey = "reflexdesk.settings.v1";
const defaults = {
  sttProvider: "moonshine",
  language: "en",
  reflexProvider: "deterministic",
  layaEndpoint: "http://127.0.0.1:8787",
  plannerMode: "local",
  plannerEndpoint: "http://127.0.0.1:11434/v1/chat/completions",
  plannerModel: "auto",
  workspace: "",
};
let settings = { ...defaults, ...JSON.parse(localStorage.getItem(settingsKey) || "{}") };
let active = false;
const orb = new ParticleOrb($("miniOrb"), { compact: true });
orb.start();

function saveSettings() {
  for (const key of Object.keys(defaults)) settings[key] = $(key).value;
  localStorage.setItem(settingsKey, JSON.stringify(settings));
  $("modeBadge").textContent = settings.plannerMode === "local" ? "📴 Offline" : "☁️ Hybrid";
}

for (const key of Object.keys(defaults)) {
  const el = $(key);
  el.value = settings[key];
  el.addEventListener("change", saveSettings);
}
saveSettings();

function setActive(value) {
  active = Boolean(value);
  $("masterToggle").setAttribute("aria-pressed", String(active));
  $("masterLabel").textContent = active ? "Listening" : "Off";
  $("statusLine").textContent = active ? "Listening locally…" : "Ready. Offline by default.";
  orb.setLevel(active ? 0.42 : 0.08);
}

$("masterToggle").addEventListener("click", async () => {
  const status = await invoke("toggle_listening");
  setActive(status.active);
});

async function dispatchText(text) {
  const route = routeFast(text);
  $("transcript").textContent = text;

  if (route.kind === "control" && route.action === "voice.stop") {
    const status = await invoke("set_listening", { active: false });
    setActive(status.active);
    return;
  }

  if (route.kind === "tool") {
    $("statusLine").textContent = `Running ${route.action}…`;
    orb.setLevel(0.82);
    try {
      if (route.action === "harness.start") {
        route.args = { ...(route.args || {}), cwd: settings.workspace || undefined };
        if (!window.confirm(`Start ${route.args?.harness || "AI harness"}?`)) return;
      }
      const result = await invoke("execute_tool", { name: route.action, args: route.args });
      $("statusLine").textContent = result.message || "Done.";
    } catch (error) {
      $("statusLine").textContent = `Blocked / failed: ${error}`;
    } finally {
      setTimeout(() => orb.setLevel(active ? 0.42 : 0.08), 320);
    }
    return;
  }

  if (settings.reflexProvider === "laya") {
    try {
      const laya = await invoke("laya_route", { endpoint: settings.layaEndpoint, text });
      const answer = laya?.answers?.intent;
      const action = answer?.choice;
      const confidence = Number(answer?.confidence || 0);
      if (action && action !== "unknown" && confidence >= 0.8) {
        $("statusLine").textContent = `Laya → ${action} (${Math.round(confidence * 100)}%). Extracting arguments…`;
      }
    } catch (_) {
      // Laya is optional. Planner remains the safe fallback for argument extraction.
    }
  }

  $("statusLine").textContent = "Planning locally…";
  try {
    const planned = await invoke("planner_route", {
      endpoint: settings.plannerEndpoint,
      model: settings.plannerModel,
      text,
      allowRemote: settings.plannerMode === "hybrid",
    });
    if (planned.action && planned.action !== "unknown") {
      if (planned.action === "harness.start") {
        planned.args = { ...(planned.args || {}), cwd: planned.args?.cwd || settings.workspace || undefined };
        if (!window.confirm(`Start ${planned.args?.harness || "AI harness"}?`)) return;
      }
      const result = await invoke("execute_tool", { name: planned.action, args: planned.args || {} });
      $("statusLine").textContent = result.message || "Done.";
      return;
    }
    $("statusLine").textContent = "No safe local action matched.";
  } catch (error) {
    $("statusLine").textContent = `Planner unavailable: ${error}`;
  }
}

$("runCommand").addEventListener("click", () => dispatchText($("commandInput").value));
$("commandInput").addEventListener("keydown", (e) => {
  if (e.key === "Enter") dispatchText(e.currentTarget.value);
});

async function refreshHarnesses() {
  const list = await invoke("detect_harnesses");
  $("harnessList").innerHTML = list.length
    ? list.map((h) => `<span class="harness-pill ${h.installed ? "online" : ""}">${h.name}${h.installed ? "" : " · missing"}</span>`).join("")
    : '<span class="muted">No supported harness found.</span>';
}
$("refreshHarnesses").addEventListener("click", refreshHarnesses);
refreshHarnesses();

listen("reflexdesk://active", (event) => setActive(event.payload));
listen("reflexdesk://transcript", (event) => dispatchText(event.payload.text));
invoke("get_status").then((status) => setActive(status.active));
