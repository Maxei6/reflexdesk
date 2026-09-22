import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ParticleOrb } from "./lib/particles.js";
import { routeFast } from "./lib/router.js";

const $ = function (id) { return document.getElementById(id); };
const settingsKey = "reflexdesk.settings.v1";

const defaults = {
  sttProvider: "nemotron",
  language: "auto",
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

  if (settings.sttProvider === "nemotron") {
    $("sttBadge").textContent = "🎧 Nemotron 3.5";
  } else if (settings.sttProvider === "moonshine") {
    $("sttBadge").textContent = "🎧 Moonshine fallback";
  } else {
    $("sttBadge").textContent = "⌨️ Manual";
  }

  refreshSttStatus();
}

for (const key of Object.keys(defaults)) {
  const el = $(key);
  el.value = settings[key];
  el.addEventListener("change", saveSettings);
}

function setActive(value) {
  active = Boolean(value);
  $("masterToggle").setAttribute("aria-pressed", String(active));
  $("masterLabel").textContent = active ? "Listening" : "Off";
  $("statusLine").textContent = active ? "Listening locally…" : "Ready. Offline by default.";
  orb.setLevel(active ? 0.42 : 0.08);
}

$("masterToggle").addEventListener("click", async function () {
  const status = await invoke("toggle_listening");
  setActive(status.active);
});

async function dispatchText(text) {
  const cleanText = String(text || "").trim();
  if (!cleanText) return;

  const route = routeFast(cleanText);
  $("transcript").textContent = cleanText;

  if (route.kind === "control" && route.action === "voice.stop") {
    const status = await invoke("set_listening", { active: false });
    setActive(status.active);
    return;
  }

  if (route.kind === "tool") {
    $("statusLine").textContent = "Running " + route.action + "…";
    orb.setLevel(0.82);

    try {
      if (route.action === "harness.start") {
        route.args = { ...(route.args || {}), cwd: settings.workspace || undefined };
        if (!window.confirm("Start " + (route.args && route.args.harness ? route.args.harness : "AI harness") + "?")) {
          return;
        }
      }

      const result = await invoke("execute_tool", {
        name: route.action,
        args: route.args,
      });
      $("statusLine").textContent = result.message || "Done.";
    } catch (error) {
      $("statusLine").textContent = "Blocked / failed: " + String(error);
    } finally {
      setTimeout(function () {
        orb.setLevel(active ? 0.42 : 0.08);
      }, 320);
    }
    return;
  }

  if (settings.reflexProvider === "laya") {
    try {
      const laya = await invoke("laya_route", {
        endpoint: settings.layaEndpoint,
        text: cleanText,
      });
      const answer = laya && laya.answers ? laya.answers.intent : null;
      const action = answer ? answer.choice : null;
      const confidence = Number(answer && answer.confidence ? answer.confidence : 0);
      if (action && action !== "unknown" && confidence >= 0.8) {
        $("statusLine").textContent =
          "Laya → " + action + " (" + Math.round(confidence * 100) + "%). Extracting arguments…";
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
      text: cleanText,
      allowRemote: settings.plannerMode === "hybrid",
    });

    if (planned.action && planned.action !== "unknown") {
      if (planned.action === "harness.start") {
        planned.args = {
          ...(planned.args || {}),
          cwd: planned.args && planned.args.cwd
            ? planned.args.cwd
            : settings.workspace || undefined,
        };
        if (!window.confirm("Start " + (planned.args.harness || "AI harness") + "?")) return;
      }

      const result = await invoke("execute_tool", {
        name: planned.action,
        args: planned.args || {},
      });
      $("statusLine").textContent = result.message || "Done.";
      return;
    }

    $("statusLine").textContent = "No safe local action matched.";
  } catch (error) {
    $("statusLine").textContent = "Planner unavailable: " + String(error);
  }
}

$("runCommand").addEventListener("click", function () {
  dispatchText($("commandInput").value);
});

$("commandInput").addEventListener("keydown", function (event) {
  if (event.key === "Enter") dispatchText(event.currentTarget.value);
});

async function refreshHarnesses() {
  const list = await invoke("detect_harnesses");
  $("harnessList").innerHTML = list.length
    ? list
        .map(function (h) {
          return '<span class="harness-pill ' + (h.installed ? "online" : "") + '">'
            + h.name + (h.installed ? "" : " · missing") + "</span>";
        })
        .join("")
    : '<span class="muted">No supported harness found.</span>';
}

async function refreshSttStatus() {
  if (!$("sttRuntimeStatus")) return;

  if (settings.sttProvider !== "nemotron") {
    $("sttRuntimeStatus").textContent =
      settings.sttProvider === "moonshine"
        ? "Moonshine runs locally as the compatibility fallback."
        : "Speech recognition is disabled; the microphone visualizer can still be tested.";
    return;
  }

  try {
    const status = await invoke("stt_status");
    $("sttRuntimeStatus").textContent = status.ready
      ? "Nemotron 3.5 is warm and local."
      : status.runtime_found
        ? "Native runtime installed. The model starts on first listen."
        : "Native runtime missing — reinstall or run npm run prepare:stt.";
  } catch (error) {
    $("sttRuntimeStatus").textContent = "STT status unavailable: " + String(error);
  }
}

$("refreshHarnesses").addEventListener("click", refreshHarnesses);

listen("reflexdesk://active", function (event) {
  setActive(event.payload);
});

listen("reflexdesk://transcript", function (event) {
  dispatchText(event.payload.text);
});

listen("reflexdesk://stt-status", function (event) {
  if (!active) return;
  const message = String(event.payload && event.payload.message ? event.payload.message : "")
    .replace(/\x1b\[[0-9;]*m/g, "");
  if (message) $("statusLine").textContent = message.slice(0, 140);
});

refreshHarnesses();
saveSettings();
invoke("get_status").then(function (status) {
  setActive(status.active);
});
