import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ParticleOrb } from "./lib/particles.js";
import { routeFast } from "./lib/router.js";
import {
  t,
  localizeError,
  setLocale,
  getLocale,
  translateDom,
  SUPPORTED_LOCALES,
} from "./lib/i18n.js";
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

function fillUiLocales(select) {
  if (!select) return;
  const options = [{ code: "system", name: "System Default" }, ...SUPPORTED_LOCALES];
  select.innerHTML = options
    .map(function (entry) {
      return '<option value="' + entry.code + '">' + entry.name + '</option>';
    })
    .join("");
}

fillUiLocales($("uiLocale"));

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
    uiLocale: settings.ui_locale || "system",
    overlayEnabled: settings.overlay_enabled,
    plannerMode: settings.allow_online_ai ? "hybrid" : "local",
    layaEndpoint: settings.laya_endpoint,
    plannerEndpoint: settings.planner_endpoint,
    plannerModel: settings.planner_model,
  });
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
  if ($("uiLocale")) $("uiLocale").value = settings.ui_locale || "system";
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
    ? t("advanced.last_stt_benchmark", { ms: settings.voice_benchmark_ms })
    : t("advanced.diag_not_measured");
  renderProviderStatus();
}

function renderProviderStatus() {
  const hasSecret = Boolean(settings && settings.planner_secret_ref && settings.planner_secret_ref.id);
  const badge = $("providerStatusBadge");
  const testBtn = $("testProvider");
  const disconnectBtn = $("disconnectProvider");
  const msg = $("providerStatusMessage");

  if (!badge) return;

  if (hasSecret) {
    badge.textContent = t("provider.status_connected");
    badge.style.color = "#7de29f";
    badge.style.borderColor = "#21412c";
    badge.style.background = "#0d1811";
    if (testBtn) testBtn.disabled = false;
    if (disconnectBtn) disconnectBtn.disabled = false;
    if (msg && !msg.textContent) {
      msg.textContent = t("provider.connected_id", { id: settings.planner_secret_ref.id });
      msg.className = "provider-status-msg";
    }
  } else {
    badge.textContent = t("provider.status_disconnected");
    badge.style.color = "#888";
    badge.style.borderColor = "#333";
    badge.style.background = "#141414";
    if (testBtn) testBtn.disabled = true;
    if (disconnectBtn) disconnectBtn.disabled = true;
    if (msg) {
      msg.textContent = "";
      msg.className = "provider-status-msg";
    }
  }
}

function statusPresentation(state) {
  const phase = state && state.phase ? state.phase : "booting";

  if (phase === "ready") {
    return {
      cls: "ready",
      pill: t("status_pill.ready"),
      headline: t("dashboard.ready_headline"),
      subline: t("dashboard.ready_subline"),
      button: t("dashboard.ready_button"),
      orb: "ready",
    };
  }

  if ((state && state.listening) || phase === "listening") {
    return {
      cls: "listening",
      pill: t("status_pill.listening"),
      headline: t("dashboard.listening_headline"),
      subline: t("dashboard.listening_subline"),
      button: t("dashboard.listening_button"),
      orb: "listening",
    };
  }

  if (["transcribing", "routing", "executing", "confirmation_required"].includes(phase)) {
    return {
      cls: "working",
      pill: phase === "executing" ? t("status_pill.working") : t("status_pill.understanding"),
      headline: phase === "executing" ? t("dashboard.working_headline") : t("dashboard.understanding_headline"),
      subline: t("dashboard.working_subline"),
      button: state && state.listening ? t("dashboard.listening_button") : t("dashboard.working_button"),
      orb: phase === "executing" ? "executing" : "thinking",
    };
  }

  if (phase === "error") {
    return {
      cls: "error",
      pill: t("status_pill.needs_attention"),
      headline: t("dashboard.error_headline"),
      subline: state && state.last_error ? localizeError(state.last_error) : t("dashboard.error_subline_default"),
      button: t("dashboard.error_button"),
      orb: "error",
    };
  }

  if (phase === "degraded") {
    return {
      cls: "error",
      pill: t("status_pill.recovering"),
      headline: t("dashboard.recovering_headline"),
      subline: state && state.last_error ? localizeError(state.last_error) : t("dashboard.recovering_subline_default"),
      button: t("dashboard.recovering_button"),
      orb: "error",
    };
  }

  return {
    cls: "working",
    pill: phase === "setup_required" ? t("status_pill.setup_required") : t("status_pill.preparing"),
    headline: phase === "setup_required" ? t("dashboard.setup_required_headline") : t("dashboard.preparing_headline"),
    subline: phase === "setup_required" ? t("dashboard.setup_required_subline") : t("dashboard.preparing_subline"),
    button: t("dashboard.preparing_button"),
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
      setSetupBusy(false, next.last_error ? localizeError(next.last_error) : t("onboarding.fix_issue_retry"));
      $("voiceTestStatus").textContent = t("onboarding.fix_issue_retry");
      $("testVoice").disabled = true;
      setupOrb.setState("error");
    } else if (next && next.phase === "degraded") {
      setSetupBusy(true, next.last_error ? localizeError(next.last_error) : t("onboarding.recovering_engine"));
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
    $("attentionText").textContent = next.last_error ? localizeError(next.last_error) : t("dashboard.attention_engine_not_ready");
  }
}

function showOnboarding() {
  $("dashboard").classList.add("hidden");
  $("onboarding").classList.remove("hidden");

  const initial = settings.language === "auto" ? guessedLanguage() : settings.language;
  $("setupLanguage").value = initial;
  $("setupAutostart").checked = Boolean(settings.start_at_login);
  setupOrb.setState("ready");
  translateDom();
}

function showDashboard() {
  $("onboarding").classList.add("hidden");
  $("dashboard").classList.remove("hidden");
  renderSettings();
  renderRuntime(runtime);
  refreshHarnesses();
  refreshDiagnostics();
  translateDom();
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
  $("voiceTestStatus").textContent = t("onboarding.step4_engine_ready");
  setSetupBusy(false, t("onboarding.engine_ready"));
  setupOrb.setState("ready");
}

async function prepareSetup() {
  try {
    setSetupBusy(true, t("onboarding.checking_mic"));
    setupOrb.setState("thinking");

    await requestMicrophonePermission();

    settings.language = $("setupLanguage").value;
    settings.start_at_login = $("setupAutostart").checked;
    settings.stt_provider = "nemotron";
    await persistSettings();

    setSetupBusy(true, t("onboarding.preparing_nemotron"));
    await invoke("prepare_engine");

    const current = await invoke("get_runtime_status");
    renderRuntime(current);
    if (current.ready) unlockVoiceTest();
  } catch (error) {
    setSetupBusy(false, t("onboarding.setup_stopped", { error: String(error) }));
    setupOrb.setState("error");
  }
}

async function startVoiceTest() {
  if (!engineReady) return;

  try {
    onboardingTesting = true;
    testStartedAt = performance.now();
    $("testVoice").disabled = true;
    $("voiceTestStatus").textContent = t("onboarding.step4_prompt");
    setupOrb.setState("listening");
    await invoke("set_listening", { active: true });
  } catch (error) {
    onboardingTesting = false;
    $("testVoice").disabled = false;
    $("voiceTestStatus").textContent = t("onboarding.voice_test_failed", { error: String(error) });
    setupOrb.setState("error");
  }
}

async function passVoiceTest(payload) {
  const heard = String(payload && payload.text ? payload.text : "").trim();
  const route = routeFast(heard);

  if (!(route.kind === "control" && route.action === "reflex.ping")) {
    $("voiceTestStatus").textContent = t("onboarding.step4_heard_retry", { heard: heard.slice(0, 70) });
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
  $("voiceTestStatus").textContent = t("onboarding.step4_heard_success", { heard: heard.slice(0, 80) });
  $("setupDone").classList.remove("hidden");
  $("benchmarkText").textContent = t("onboarding.done_benchmark", { ms: benchmarkMs });
  $("finishSetup").focus();
}

async function finishSetup() {
  try {
    settings = await invoke("complete_setup", { benchmarkMs: benchmarkMs || 1 });
    syncLocalVoiceSettings();
    showDashboard();
  } catch (error) {
    $("voiceTestStatus").textContent = t("onboarding.finish_error", { error: String(error) });
    setupOrb.setState("error");
  }
}

async function saveDashboardSettings() {
  if (!settings) return;

  if ($("uiLocale")) {
    const prevLocale = settings.ui_locale;
    settings.ui_locale = $("uiLocale").value;
    if (settings.ui_locale !== prevLocale) {
      setLocale(settings.ui_locale);
      translateDom();
    }
  }
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
    $("attentionText").textContent = t("settings.save_error", { error: String(error) });
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
    $("harnessList").innerHTML = '<span class="muted">' + t("agents.check_failed") + '</span>';
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
      invoke("get_benchmark_report").catch(() => null),
    ]);
    const profile = values[0];
    const speech = values[1];
    const cacheBytes = Number(values[2] || 0);
    const modelStatus = values[3];
    const benchmarkReport = values[4];
    $("systemInfo").textContent =
      profile.os + " · " + profile.arch + " · " + profile.logical_cpus
      + " threads · " + profile.acceleration_hint;

    $("engineInfo").textContent = speech.ready
      ? t("advanced.diag_engine_ready")
      : speech.running
        ? t("advanced.diag_engine_starting")
        : t("advanced.diag_engine_stopped");

    if ($("modelCacheInfo")) {
      const mb = (cacheBytes / (1024 * 1024)).toFixed(1);
      $("modelCacheInfo").textContent = t("advanced.diag_cache_mb", { mb: mb });
    }


    if ($("modelBackendInfo") && modelStatus) {
      $("modelBackendInfo").textContent = modelStatus.verified
        ? "Nemotron 3.5 Q4_K (Verified Active)"
        : "Nemotron 3.5 (" + modelStatus.state + ")";
    }
    if ($("benchmarkInfo")) {
      if (benchmarkReport && benchmarkReport.selected_candidate_id) {
        const top = benchmarkReport.candidates && benchmarkReport.candidates[0];
        const rtfStr = top && top.rtf && top.rtf < 900 ? ` (${top.rtf.toFixed(2)}x RTF)` : "";
        $("benchmarkInfo").textContent = `${benchmarkReport.selected_candidate_id}${rtfStr}`;
      } else if (settings.voice_benchmark_ms) {
        $("benchmarkInfo").textContent = settings.voice_benchmark_ms + " ms last local STT";
      } else {
        $("benchmarkInfo").textContent = t("advanced.diag_not_measured");
      }
    }
  } catch {
    $("systemInfo").textContent = t("advanced.diag_unavailable");
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
  kicker.textContent = t("policy.modal.kicker");

  const title = document.createElement("h2");
  title.style.margin = "0";
  title.style.fontSize = "18px";
  title.style.fontWeight = "600";
  title.textContent = t("policy.modal.title");

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

  addRow(t("policy.modal.tool"), req.tool || "unknown");
  const riskKey = "policy.risk." + (req.risk || "unknown").toLowerCase();
  addRow(t("policy.modal.risk"), t(riskKey, { default: (req.risk || "unknown").toUpperCase() }));
  if (req.args_summary) {
    addRow(t("policy.modal.arguments"), req.args_summary);
  }

  card.appendChild(details);

  const actions = document.createElement("div");
  actions.style.display = "flex";
  actions.style.justifyContent = "flex-end";
  actions.style.gap = "10px";
  actions.style.marginTop = "8px";

  const denyBtn = document.createElement("button");
  denyBtn.className = "secondary";
  denyBtn.textContent = t("policy.modal.deny");
  denyBtn.style.minWidth = "88px";

  const approveBtn = document.createElement("button");
  approveBtn.className = "primary";
  approveBtn.textContent = t("policy.modal.approve");
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
    const reason = result.reason ? localizeError(result.reason) : t("policy.status.denied");
    $("attentionText").textContent = reason;
    dashboardOrb.setState("idle");
  } else if (result.status === "cancelled") {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = t("policy.status.cancelled");
    dashboardOrb.setState("idle");
  } else if (result.status === "error") {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = result.reason ? localizeError(result.reason) : t("policy.status.failed");
    dashboardOrb.setState("error");
  } else if (result.status === "success") {
    $("attentionBanner").classList.add("hidden");
    dashboardOrb.setState("idle");
  }
}

async function dispatchText(text) {
  const cleanText = String(text || "").trim();
  if (!cleanText) return;

  // Fast Tier-0 routing
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

  // Unified reflex layer: queries Deterministic, Supervised Laya, or Compact classifier
  try {
    const reflex = await invoke("reflex_route", { text: cleanText });
    if (reflex && reflex.action && reflex.action !== "unknown" && reflex.confidence >= 0.70) {
      if (reflex.action.startsWith("control.") || reflex.action.startsWith("voice.") || reflex.action.startsWith("reflex.")) {
        if (reflex.action === "voice.stop") {
          await invoke("set_listening", { active: false });
        }
        return;
      }
      const envelope = buildActionEnvelope(reflex.action, reflex.args || {}, "reflex");
      const result = await requestActionWithConfirmation(envelope, cleanText);
      handleActionResult(result);
      return;
    }
  } catch {}

  if (settings && settings.stt_provider === "manual") return;

  if (settings && settings.laya_endpoint) {
    try {
      const layaResult = await invoke("laya_route", {
        endpoint: settings.laya_endpoint,
        text: cleanText,
      });
      const choice = (layaResult && (layaResult.choice || (layaResult.intent && layaResult.intent.choice))) || "";
      const conf = (layaResult && (typeof layaResult.confidence === "number" ? layaResult.confidence : (layaResult.intent && layaResult.intent.confidence))) || 0;
      if (choice && choice !== "unknown" && conf >= 0.70) {
        const envelope = buildActionEnvelope(choice, {}, "reflex");
        const result = await requestActionWithConfirmation(envelope, cleanText);
        handleActionResult(result);
        return;
      }
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
  setLocale(settings.ui_locale || "system");
  translateDom();
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
  "uiLocale",
  "language",
  "startAtLogin",
  "overlayEnabled",
  "allowOnlineAi",
  "sttProvider",
  "layaEndpoint",
  "plannerEndpoint",
  "plannerModel",
]) {
  if ($(id)) $(id).addEventListener("change", saveDashboardSettings);
}

$("resetSetup").addEventListener("click", async function () {
  if (!window.confirm(t("settings.reset_confirm"))) return;
  settings = await invoke("reset_setup");
  showOnboarding();
});



if ($("repairModel")) {
  $("repairModel").addEventListener("click", async function () {
    try {
      $("repairModel").disabled = true;
      $("repairModel").textContent = t("advanced.btn_repairing");
      await invoke("repair_model");
      await refreshDiagnostics();
    } catch (err) {
      $("attentionBanner").classList.remove("hidden");
      $("attentionText").textContent = t("advanced.model_repair_failed", { error: String(err).slice(0, 200) });
    } finally {
      $("repairModel").disabled = false;
      $("repairModel").textContent = t("advanced.btn_repair_model");
    }
  });
}
if ($("runBenchmark")) {
  $("runBenchmark").addEventListener("click", async function () {
    try {
      $("runBenchmark").disabled = true;
      $("runBenchmark").textContent = t("advanced.btn_benchmarking");
      await invoke("run_hardware_benchmark", { timeoutSecs: 30 });
      await refreshDiagnostics();
    } catch (err) {
      $("attentionBanner").classList.remove("hidden");
      $("attentionText").textContent = t("advanced.benchmark_failed", { error: String(err).slice(0, 200) });
    } finally {
      $("runBenchmark").disabled = false;
      $("runBenchmark").textContent = t("advanced.btn_run_benchmark");
    }
  });
}
if ($("previewDiagnostics")) {
  $("previewDiagnostics").addEventListener("click", async function () {
    try {
      $("previewDiagnostics").disabled = true;
      $("previewDiagnostics").textContent = t("advanced.diag_loading");
      const bundle = await invoke("get_diagnostics_preview");
      if ($("diagnosticsPreviewText") && $("diagnosticsPreviewContainer")) {
        $("diagnosticsPreviewText").textContent = JSON.stringify(bundle, null, 2);
        $("diagnosticsPreviewContainer").classList.remove("hidden");
      }
    } catch (err) {
      if ($("diagnosticsStatus")) {
        $("diagnosticsStatus").textContent = t("advanced.preview_error", { error: String(err).slice(0, 100) });
      }
    } finally {
      $("previewDiagnostics").disabled = false;
      $("previewDiagnostics").textContent = t("advanced.diag_preview_btn");
    }
  });
}
if ($("closeDiagnosticsPreview")) {
  $("closeDiagnosticsPreview").addEventListener("click", function () {
    if ($("diagnosticsPreviewContainer")) {
      $("diagnosticsPreviewContainer").classList.add("hidden");
    }
  });
}
if ($("exportDiagnostics")) {
  $("exportDiagnostics").addEventListener("click", async function () {
    try {
      $("exportDiagnostics").disabled = true;
      $("exportDiagnostics").textContent = t("advanced.btn_exporting");
      const bundle = await invoke("export_diagnostics", { path: null });
      if ($("diagnosticsStatus")) {
        $("diagnosticsStatus").textContent = t("advanced.export_success_plural", { count: bundle.recent_logs.length });
      }
    } catch (err) {
      if ($("diagnosticsStatus")) {
        $("diagnosticsStatus").textContent = t("advanced.export_error", { error: String(err).slice(0, 100) });
      }
    } finally {
      $("exportDiagnostics").disabled = false;
      $("exportDiagnostics").textContent = t("advanced.diag_export_btn");
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

if ($("connectProvider")) {
  $("connectProvider").addEventListener("click", async function () {
    const keyInput = $("providerApiKey");
    const key = keyInput ? keyInput.value.trim() : "";
    const msg = $("providerStatusMessage");
    if (!key) {
      if (msg) {
        msg.textContent = t("provider.enter_key_prompt");
        msg.className = "provider-status-msg error";
      }
      return;
    }

    try {
      $("connectProvider").disabled = true;
      $("connectProvider").textContent = "Connecting…";
      const status = await invoke("connect_provider", {
        provider: "planner",
        apiKey: key,
      });

      // Immediately zeroize / clear DOM input
      keyInput.value = "";

      settings = await invoke("get_app_settings");
      renderSettings();

      if (msg) {
        msg.textContent = status.message || t("provider.connected_success");
        msg.className = status.last_status === "auth-invalid"
          ? "provider-status-msg error"
          : "provider-status-msg success";
      }
    } catch (err) {
      if (msg) {
        msg.textContent = t("provider.connection_failed", { error: String(err).slice(0, 150) });
        msg.className = "provider-status-msg error";
      }
    } finally {
      $("connectProvider").disabled = false;
      $("connectProvider").textContent = "Connect";
    }
  });
}

if ($("testProvider")) {
  $("testProvider").addEventListener("click", async function () {
    const msg = $("providerStatusMessage");
    try {
      $("testProvider").disabled = true;
      $("testProvider").textContent = "Testing…";
      const status = await invoke("test_provider", { provider: "planner" });
      if (msg) {
        msg.textContent = status.message || t("provider.connected_success");
        msg.className = status.last_status === "auth-invalid"
          ? "provider-status-msg error"
          : "provider-status-msg success";
      }
      if (status.last_status === "auth-invalid") {
        $("attentionBanner").classList.remove("hidden");
        $("attentionText").textContent = t("provider.auth_invalid");
      }
    } catch (err) {
      if (msg) {
        msg.textContent = t("provider.test_error", { error: String(err).slice(0, 150) });
        msg.className = "provider-status-msg error";
      }
    } finally {
      $("testProvider").disabled = false;
      $("testProvider").textContent = "Test";
    }
  });
}

if ($("disconnectProvider")) {
  $("disconnectProvider").addEventListener("click", async function () {
    const msg = $("providerStatusMessage");
    try {
      $("disconnectProvider").disabled = true;
      await invoke("disconnect_provider", { provider: "planner" });
      settings = await invoke("get_app_settings");
      renderSettings();
      if (msg) {
        msg.textContent = t("provider.disconnected_success");
        msg.className = "provider-status-msg";
      }
    } catch (err) {
      if (msg) {
        msg.textContent = t("provider.disconnect_error", { error: String(err).slice(0, 150) });
        msg.className = "provider-status-msg error";
      }
    } finally {
      $("disconnectProvider").disabled = false;
    }
  });
}

bootstrap().catch(function (error) {
  document.body.innerHTML =
    '<main class="fatal"><h1>ReflexDesk could not start.</h1><p>'
    + String(error)
    + "</p></main>";
});
