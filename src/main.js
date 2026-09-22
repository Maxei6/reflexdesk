import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ParticleOrb } from "./lib/particles.js";
import { routeFast } from "./lib/router.js";

const $ = (id) => document.getElementById(id);
const LOCAL_KEY = "reflexdesk.settings.v1";

const LANGUAGES = [
  ["auto", "Auto detect"],
  ["en", "English"],
  ["it", "Italiano"],
  ["es", "Español"],
  ["fr", "Français"],
  ["de", "Deutsch"],
  ["pt", "Português"],
  ["nl", "Nederlands"],
  ["tr", "Türkçe"],
  ["ru", "Русский"],
  ["ar", "العربية"],
  ["hi", "हिन्दी"],
  ["ja", "日本語"],
  ["ko", "한국어"],
  ["vi", "Tiếng Việt"],
  ["uk", "Українська"],
  ["zh", "中文"],
];

let settings = null;
let runtime = null;
let onboardingTesting = false;
let testStartedAt = 0;
let benchmarkMs = null;
let engineReady = false;

const setupOrb = new ParticleOrb($("setupOrb"), { compact: true });
const dashboardOrb = new ParticleOrb($("dashboardOrb"), { compact: true });
setupOrb.start();
dashboardOrb.start();

function fillLanguages(select) {
  select.innerHTML = LANGUAGES
    .map(function (entry) {
      return '<option value="' + entry[0] + '">' + entry[1] + '</option>';
    })
    .join("");
}

fillLanguages($("setupLanguage"));
fillLanguages($("language"));

function guessedLanguage() {
  const code = String(navigator.language || "en").toLowerCase().split("-")[0];
  return LANGUAGES.some(function (entry) { return entry[0] === code; }) ? code : "auto";
}

function syncLocalVoiceSettings() {
  if (!settings) return;
  const previous = JSON.parse(localStorage.getItem(LOCAL_KEY) || "{}");
  const next = Object.assign({}, previous, {
    sttProvider: settings.stt_provider,
    language: settings.language,
    overlayEnabled: settings.overlay_enabled,
    plannerMode: settings.allow_online_ai ? "hybrid" : "local",
    layaEndpoint: settings.laya_endpoint,
    plannerEndpoint: settings.planner_endpoint,
    plannerModel: settings.planner_model,
  });
  localStorage.setItem(LOCAL_KEY, JSON.stringify(next));
}

async function persistSettings() {
  syncLocalVoiceSettings();
  settings = await invoke("save_app_settings", { settings: settings });
  syncLocalVoiceSettings();
  renderSettings();
  return settings;
}

function renderSettings() {
  if (!settings) return;

  $("language").value = settings.language;
  $("startAtLogin").checked = Boolean(settings.start_at_login);
  $("overlayEnabled").checked = Boolean(settings.overlay_enabled);
  $("allowOnlineAi").checked = Boolean(settings.allow_online_ai);
  $("sttProvider").value = settings.stt_provider;
  $("layaEndpoint").value = settings.laya_endpoint;
  $("plannerEndpoint").value = settings.planner_endpoint;
  $("plannerModel").value = settings.planner_model;

  $("shortcutKey").textContent = settings.shortcut
    .replace("CommandOrControl", navigator.platform && navigator.platform.toLowerCase().includes("mac") ? "Cmd" : "Ctrl")
    .replaceAll("+", " + ");

  $("benchmarkInfo").textContent = settings.voice_benchmark_ms
    ? settings.voice_benchmark_ms + " ms last local STT"
    : "Not measured";
}

function statusPresentation(state) {
  const phase = state && state.phase ? state.phase : "booting";

  if (phase === "ready") {
    return {
      cls: "ready",
      pill: "Ready",
      headline: "Ready when you are.",
      subline: "Press the shortcut anywhere, or start listening here.",
      button: "Talk to ReflexDesk",
      orb: "ready",
    };
  }

  if ((state && state.listening) || phase === "listening") {
    return {
      cls: "listening",
      pill: "Listening",
      headline: "I'm listening.",
      subline: "Speak naturally. Press the shortcut again to stop.",
      button: "Stop listening",
      orb: "listening",
    };
  }

  if (["transcribing", "routing", "executing", "confirmation_required"].includes(phase)) {
    return {
      cls: "working",
      pill: phase === "executing" ? "Working" : "Understanding",
      headline: phase === "executing" ? "Doing it." : "Understanding you.",
      subline: "Everything stays local unless online fallback is enabled.",
      button: state && state.listening ? "Stop listening" : "Working…",
      orb: phase === "executing" ? "executing" : "thinking",
    };
  }

  if (phase === "error") {
    return {
      cls: "error",
      pill: "Needs attention",
      headline: "ReflexDesk needs a quick fix.",
      subline: state && state.last_error ? state.last_error : "The local engine did not start correctly.",
      button: "Unavailable",
      orb: "error",
    };
  }

  if (phase === "degraded") {
    return {
      cls: "error",
      pill: "Recovering",
      headline: "Restarting the local engine.",
      subline: state && state.last_error ? state.last_error : "ReflexDesk is recovering automatically.",
      button: "Recovering…",
      orb: "error",
    };
  }

  return {
    cls: "working",
    pill: phase === "setup_required" ? "Setup required" : "Preparing",
    headline: phase === "setup_required" ? "Finish setup first." : "ReflexDesk is preparing.",
    subline: "The local engine must be ready before voice control can start.",
    button: "Preparing…",
    orb: "thinking",
  };
}

function renderRuntime(next) {
  runtime = next;
  engineReady = Boolean(next && next.ready);

  if (!settings || !settings.setup_complete) {
    if (engineReady) {
      unlockVoiceTest();
    } else if (next && next.phase === "error") {
      onboardingTesting = false;
      setSetupBusy(false, next.last_error || "The local engine could not start.");
      $("voiceTestStatus").textContent = "Fix the issue above, then retry setup.";
      $("testVoice").disabled = true;
      setupOrb.setState("error");
    } else if (next && next.phase === "degraded") {
      setSetupBusy(true, next.last_error || "Recovering the local engine…");
      setupOrb.setState("warning");
    }
    return;
  }

  const ui = statusPresentation(next);
  const pill = $("statusPill");
  pill.className = "status-pill " + ui.cls;
  pill.querySelector("b").textContent = ui.pill;

  $("dashboardHeadline").textContent = ui.headline;
  $("dashboardSubline").textContent = ui.subline;

  const button = $("listenButton");
  button.querySelector("b").textContent = ui.button;
  button.disabled = !(next && next.ready) && !(next && next.listening);
  button.classList.toggle("listening", Boolean(next && next.listening));

  dashboardOrb.setState(ui.orb);

  const showAttention = next && ["error", "degraded"].includes(next.phase);
  $("attentionBanner").classList.toggle("hidden", !showAttention);
  if (showAttention) {
    $("attentionText").textContent = next.last_error || "The local engine is not ready.";
  }
}

function showOnboarding() {
  $("dashboard").classList.add("hidden");
  $("onboarding").classList.remove("hidden");

  const initial = settings.language === "auto" ? guessedLanguage() : settings.language;
  $("setupLanguage").value = initial;
  $("setupAutostart").checked = Boolean(settings.start_at_login);
  setupOrb.setState("ready");
}

function showDashboard() {
  $("onboarding").classList.add("hidden");
  $("dashboard").classList.remove("hidden");
  renderSettings();
  renderRuntime(runtime);
  refreshHarnesses();
  refreshDiagnostics();
}

async function requestMicrophonePermission() {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
      channelCount: 1,
    },
  });

  for (const track of stream.getTracks()) track.stop();
}

function setSetupBusy(busy, text) {
  $("prepareSetup").disabled = busy;
  $("setupProgress").classList.toggle("hidden", !busy);
  if (text) $("setupStatus").textContent = text;
}

function unlockVoiceTest() {
  engineReady = true;
  $("voiceTestStep").classList.add("unlocked");
  $("testVoice").disabled = false;
  $("voiceTestStatus").textContent =
    "Local engine ready. Say a short phrase to prove microphone → speech → ReflexDesk works.";
  setSetupBusy(false, "Local speech engine is ready.");
  setupOrb.setState("ready");
}

async function prepareSetup() {
  try {
    setSetupBusy(true, "Checking microphone permission…");
    setupOrb.setState("thinking");

    await requestMicrophonePermission();

    settings.language = $("setupLanguage").value;
    settings.start_at_login = $("setupAutostart").checked;
    settings.stt_provider = "nemotron";
    await persistSettings();

    setSetupBusy(true, "Preparing NVIDIA Nemotron locally. First setup may download the model…");
    await invoke("prepare_engine");

    const current = await invoke("get_runtime_status");
    renderRuntime(current);
    if (current.ready) unlockVoiceTest();
  } catch (error) {
    setSetupBusy(false, "Setup stopped: " + String(error));
    setupOrb.setState("error");
  }
}

async function startVoiceTest() {
  if (!engineReady) return;

  try {
    onboardingTesting = true;
    testStartedAt = performance.now();
    $("testVoice").disabled = true;
    $("voiceTestStatus").textContent = "Say exactly: “Hello ReflexDesk”.";
    setupOrb.setState("listening");
    await invoke("set_listening", { active: true });
  } catch (error) {
    onboardingTesting = false;
    $("testVoice").disabled = false;
    $("voiceTestStatus").textContent = "Voice test failed to start: " + String(error);
    setupOrb.setState("error");
  }
}

async function passVoiceTest(payload) {
  const heard = String(payload && payload.text ? payload.text : "").trim();
  const route = routeFast(heard);

  if (!(route.kind === "control" && route.action === "reflex.ping")) {
    $("voiceTestStatus").textContent = "I heard “" + heard.slice(0, 70) + "”. Try again and say: “Hello ReflexDesk”.";
    setupOrb.setState("warning");
    return;
  }

  onboardingTesting = false;

  try {
    await invoke("set_listening", { active: false });
  } catch {}

  benchmarkMs = Number(
    payload && (payload.sttLatencyMs || payload.stt_latency_ms)
      ? (payload.sttLatencyMs || payload.stt_latency_ms)
      : Math.round(performance.now() - testStartedAt),
  );

  setupOrb.setState("success");
  $("voiceTestStatus").textContent =
    "Heard: “" + heard.slice(0, 80) + "”";
  $("setupDone").classList.remove("hidden");
  $("benchmarkText").textContent = "Local speech passed in about " + benchmarkMs + " ms.";
  $("finishSetup").focus();
}

async function finishSetup() {
  try {
    settings = await invoke("complete_setup", { benchmarkMs: benchmarkMs || 1 });
    syncLocalVoiceSettings();
    showDashboard();
  } catch (error) {
    $("voiceTestStatus").textContent = "Could not finish setup: " + String(error);
    setupOrb.setState("error");
  }
}

async function saveDashboardSettings() {
  if (!settings) return;

  settings.language = $("language").value;
  settings.start_at_login = $("startAtLogin").checked;
  settings.overlay_enabled = $("overlayEnabled").checked;
  settings.allow_online_ai = $("allowOnlineAi").checked;
  settings.stt_provider = $("sttProvider").value;
  settings.laya_endpoint = $("layaEndpoint").value.trim();
  settings.planner_endpoint = $("plannerEndpoint").value.trim();
  settings.planner_model = $("plannerModel").value.trim() || "auto";

  try {
    await persistSettings();
  } catch (error) {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = "Could not save setting: " + String(error);
  }
}

async function toggleListening() {
  try {
    const next = await invoke("toggle_listening");
    renderRuntime(next);
  } catch (error) {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = String(error);
  }
}

async function retryEngine() {
  $("attentionBanner").classList.add("hidden");
  await invoke("prepare_engine");
  renderRuntime(await invoke("get_runtime_status"));
}

async function refreshHarnesses() {
  if (!settings || !settings.setup_complete) return;

  try {
    const list = await invoke("detect_harnesses");
    $("harnessList").innerHTML = list
      .map(function (h) {
        return '<div class="agent-pill ' + (h.installed ? "connected" : "") + '">'
          + "<span>" + h.name + "</span>"
          + '<i title="' + (h.installed ? "Detected" : "Not installed") + '"></i>'
          + "</div>";
      })
      .join("");
  } catch {
    $("harnessList").innerHTML = '<span class="muted">Could not check local agents.</span>';
  }
}

async function refreshDiagnostics() {
  if (!settings || !settings.setup_complete) return;

  try {
    const values = await Promise.all([
      invoke("get_system_profile"),
      invoke("stt_status"),
      invoke("get_model_cache_size").catch(() => 0),
      invoke("get_model_status", { modelId: "nemotron-3.5-asr-streaming-0.6b" }).catch(() => null),
    ]);
    const profile = values[0];
    const speech = values[1];
    const cacheBytes = Number(values[2] || 0);
    const modelStatus = values[3];

    $("systemInfo").textContent =
      profile.os + " · " + profile.arch + " · " + profile.logical_cpus
      + " threads · " + profile.acceleration_hint;

    $("engineInfo").textContent = speech.ready
      ? "Nemotron 3.5 ready locally"
      : speech.running
        ? "Starting local engine"
        : "Local engine stopped";

    if ($("modelCacheInfo")) {
      const mb = (cacheBytes / (1024 * 1024)).toFixed(1);
      $("modelCacheInfo").textContent = mb + " MB on disk";
    }

    if ($("modelBackendInfo") && modelStatus) {
      $("modelBackendInfo").textContent = modelStatus.verified
        ? "Nemotron 3.5 Q4_K (Verified Active)"
        : "Nemotron 3.5 (" + modelStatus.state + ")";
    }
  } catch {
    $("systemInfo").textContent = "Diagnostics unavailable";
  }
}
const TOOL_RISK_MAP = {
  "app.open": "sensitive",
  "browser.open": "sensitive",
  "browser.search": "safe",
  "harness.start": "external_commit",
};

function newSessionId() {
  try {
    return crypto.randomUUID();
  } catch {
    return "s_" + Math.random().toString(16).slice(2) + Date.now().toString(16);
  }
}

function buildActionEnvelope(tool, args, source) {
  return {
    tool: String(tool || ""),
    args: args && typeof args === "object" ? args : {},
    source: source,
    session_id: newSessionId(),
    risk: TOOL_RISK_MAP[tool] || "sensitive",
    capability: TOOL_CAPABILITY_MAP[tool] || tool,
    verification: {
      kind: "none",
      selector: null,
      expect: null,
      timeout_ms: 2000,
    },
  };
}

let activeConfirmationModal = null;

function showConfirmationModal(req, onDecision) {
  if (activeConfirmationModal) {
    activeConfirmationModal.remove();
    activeConfirmationModal = null;
  }

  const overlay = document.createElement("div");
  overlay.className = "policy-modal-backdrop";
  overlay.style.position = "fixed";
  overlay.style.inset = "0";
  overlay.style.background = "rgba(0, 0, 0, 0.78)";
  overlay.style.display = "flex";
  overlay.style.alignItems = "center";
  overlay.style.justifyContent = "center";
  overlay.style.zIndex = "9999";
  overlay.style.backdropFilter = "blur(6px)";

  const card = document.createElement("div");
  card.className = "policy-modal-card";
  card.style.background = "#121212";
  card.style.border = "1px solid #2a2a2a";
  card.style.borderRadius = "18px";
  card.style.padding = "24px";
  card.style.width = "min(480px, calc(100vw - 32px))";
  card.style.boxShadow = "0 24px 80px rgba(0, 0, 0, 0.6)";
  card.style.display = "grid";
  card.style.gap = "16px";
  card.style.color = "#f4f4f4";

  const header = document.createElement("div");
  header.style.display = "grid";
  header.style.gap = "4px";

  const kicker = document.createElement("span");
  kicker.className = "kicker";
  kicker.textContent = "POLICY GATE · CONFIRMATION REQUIRED";

  const title = document.createElement("h2");
  title.style.margin = "0";
  title.style.fontSize = "18px";
  title.style.fontWeight = "600";
  title.textContent = "Approve Action Execution";

  header.appendChild(kicker);
  header.appendChild(title);
  card.appendChild(header);

  const details = document.createElement("div");
  details.style.display = "grid";
  details.style.gap = "8px";
  details.style.background = "#181818";
  details.style.border = "1px solid #242424";
  details.style.borderRadius = "10px";
  details.style.padding = "12px 14px";
  details.style.fontSize = "13px";

  function addRow(labelStr, valStr) {
    const row = document.createElement("div");
    row.style.display = "flex";
    row.style.justifyContent = "space-between";
    row.style.alignItems = "flex-start";
    row.style.gap = "12px";

    const lbl = document.createElement("span");
    lbl.style.color = "#888";
    lbl.style.flexShrink = "0";
    lbl.textContent = labelStr;

    const val = document.createElement("span");
    val.style.fontWeight = "550";
    val.style.wordBreak = "break-word";
    val.textContent = valStr;

    row.appendChild(lbl);
    row.appendChild(val);
    details.appendChild(row);
  }

  addRow("Tool", req.tool || "unknown");
  addRow("Risk", (req.risk || "unknown").toUpperCase());
  if (req.args_summary) {
    addRow("Arguments", req.args_summary);
  }

  card.appendChild(details);

  const actions = document.createElement("div");
  actions.style.display = "flex";
  actions.style.justifyContent = "flex-end";
  actions.style.gap = "10px";
  actions.style.marginTop = "8px";

  const denyBtn = document.createElement("button");
  denyBtn.className = "secondary";
  denyBtn.textContent = "Deny";
  denyBtn.style.minWidth = "88px";

  const approveBtn = document.createElement("button");
  approveBtn.className = "primary";
  approveBtn.textContent = "Approve";
  approveBtn.style.minWidth = "88px";

  function cleanup() {
    if (activeConfirmationModal) {
      activeConfirmationModal.remove();
      activeConfirmationModal = null;
    }
  }

  denyBtn.addEventListener("click", function () {
    cleanup();
    onDecision(false);
  });

  approveBtn.addEventListener("click", function () {
    cleanup();
    onDecision(true);
  });

  actions.appendChild(denyBtn);
  actions.appendChild(approveBtn);
  card.appendChild(actions);

  overlay.appendChild(card);
  document.body.appendChild(overlay);
  activeConfirmationModal = overlay;

  approveBtn.focus();
}

async function requestActionWithConfirmation(envelope, rawText) {
  const res = await invoke("request_action", {
    envelope: envelope,
    rawText: rawText,
    raw_text: rawText,
  });

  if (res && (res.status === "confirm" || res.status === "need_confirm")) {
    const confirmationId = res.confirmation_id || res.id;
    return new Promise(function (resolve) {
      showConfirmationModal(res, async function (approve) {
        try {
          const confirmRes = await invoke("confirm_action", {
            confirmationId: confirmationId,
            confirmation_id: confirmationId,
            approve: approve,
          });
          resolve(confirmRes);
        } catch (err) {
          resolve({ status: "deny", reason: String(err) });
        }
      });
    });
  }

  return res;
}

function handleActionResult(result) {
  if (!result) return;
  if (result.status === "deny" || result.status === "denied") {
    $("attentionBanner").classList.remove("hidden");
    const reason = result.reason || "Action denied by policy gate.";
    $("attentionText").textContent = reason;
    dashboardOrb.setState("idle");
  } else if (result.status === "cancelled") {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = "Action cancelled.";
    dashboardOrb.setState("idle");
  } else if (result.status === "error") {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = result.reason || "Action failed.";
    dashboardOrb.setState("error");
  } else if (result.status === "success") {
    $("attentionBanner").classList.add("hidden");
    dashboardOrb.setState("idle");
  }
}

async function dispatchText(text) {
  const cleanText = String(text || "").trim();
  if (!cleanText) return;

  const route = routeFast(cleanText);

  if (route.kind === "control") {
    if (route.action === "voice.stop") {
      await invoke("set_listening", { active: false });
    }
    return;
  }

  if (route.kind === "tool") {
    const envelope = buildActionEnvelope(route.action, route.args || {}, "reflex");
    const result = await requestActionWithConfirmation(envelope, cleanText);
    handleActionResult(result);
    return;
  }

  if (settings && settings.stt_provider === "manual") return;

  if (settings && settings.laya_endpoint) {
    try {
      await invoke("laya_route", {
        endpoint: settings.laya_endpoint,
        text: cleanText,
      });
    } catch {}
  }

  const planned = await invoke("planner_route", {
    endpoint: settings.planner_endpoint,
    model: settings.planner_model,
    text: cleanText,
    allowRemote: Boolean(settings.allow_online_ai),
  });

  if (planned && planned.action && planned.action !== "unknown") {
    const envelope = buildActionEnvelope(planned.action, planned.args || {}, "planner");
    const result = await requestActionWithConfirmation(envelope, cleanText);
    handleActionResult(result);
  }
}
async function bootstrap() {
  settings = await invoke("get_app_settings");
  runtime = await invoke("get_runtime_status");
  syncLocalVoiceSettings();

  if (settings.setup_complete) {
    showDashboard();
  } else {
    showOnboarding();
  }

  renderRuntime(runtime);
}

$("prepareSetup").addEventListener("click", prepareSetup);
$("testVoice").addEventListener("click", startVoiceTest);
$("finishSetup").addEventListener("click", finishSetup);
$("listenButton").addEventListener("click", toggleListening);
$("retryEngine").addEventListener("click", retryEngine);
$("refreshHarnesses").addEventListener("click", refreshHarnesses);
$("hideWindow").addEventListener("click", function () {
  getCurrentWindow().hide();
});

for (const id of [
  "language",
  "startAtLogin",
  "overlayEnabled",
  "allowOnlineAi",
  "sttProvider",
  "layaEndpoint",
  "plannerEndpoint",
  "plannerModel",
]) {
  $(id).addEventListener("change", saveDashboardSettings);
}

$("resetSetup").addEventListener("click", async function () {
  if (!window.confirm("Run first-time setup again? Your local model cache will be kept.")) return;
  settings = await invoke("reset_setup");
  showOnboarding();
});

if ($("repairModel")) {
  $("repairModel").addEventListener("click", async function () {
    try {
      $("repairModel").disabled = true;
      $("repairModel").textContent = "Repairing…";
      await invoke("repair_model");
      await refreshDiagnostics();
    } catch (err) {
      $("attentionBanner").classList.remove("hidden");
      $("attentionText").textContent = "Model repair failed: " + String(err).slice(0, 200);
    } finally {
      $("repairModel").disabled = false;
      $("repairModel").textContent = "Repair model";
    }
  });
}

listen("reflexdesk://model-progress", function (event) {
  const payload = event.payload;
  if (!payload) return;
  const pct = Number(payload.total_bytes) > 0
    ? Math.round((Number(payload.downloaded_bytes) / Number(payload.total_bytes)) * 100)
    : 0;
  if (!settings || !settings.setup_complete) {
    setSetupBusy(true, payload.message || ("Downloading speech model: " + pct + "%"));
  }
});

listen("reflexdesk://state", function (event) {
  renderRuntime(event.payload);
});

listen("reflexdesk://engine-ready", function () {
  if (!settings || !settings.setup_complete) unlockVoiceTest();
  refreshDiagnostics();
});

listen("reflexdesk://stt-status", function (event) {
  const message = String(event.payload && event.payload.message ? event.payload.message : "")
    .replace(/\x1b\[[0-9;]*m/g, "");
  if (settings && !settings.setup_complete && message) {
    $("setupStatus").textContent = message.slice(0, 150);
  }
});

listen("reflexdesk://attention", function (event) {
  if (!settings || !settings.setup_complete) return;
  $("attentionBanner").classList.remove("hidden");
  $("attentionText").textContent =
    event.payload && event.payload.message ? event.payload.message : "ReflexDesk needs attention.";
});

// Rust-owned transcript path: the overlay submits via `submit_transcript`
// with a Rust-issued nonce; Rust re-emits `transcript-verified`. The old
// frontend-trusted `reflexdesk://transcript` broadcast is deleted — main
// never executes from an unverified renderer event.
listen("reflexdesk://transcript-verified", async function (event) {
  if (onboardingTesting) {
    await passVoiceTest(event.payload);
    return;
  }

  if (!settings || !settings.setup_complete) return;

  try {
    await dispatchText(event.payload && event.payload.text ? event.payload.text : "");
  } catch (error) {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = String(error);
    dashboardOrb.setState("error");
  }
});

bootstrap().catch(function (error) {
  document.body.innerHTML =
    '<main class="fatal"><h1>ReflexDesk could not start.</h1><p>'
    + String(error)
    + "</p></main>";
});
