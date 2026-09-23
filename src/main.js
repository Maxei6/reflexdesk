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
function fillSecondaryLanguages(select) {
  select.innerHTML = '<option value="none">' + t("settings.secondary_language_none") + "</option>"
    + LANGUAGES.filter(function (entry) { return entry[0] !== "auto"; })
      .map(function (entry) {
        return '<option value="' + entry[0] + '">' + entry[1] + "</option>";
      }).join("");
}

function updateSecondaryChoice(primary, secondary) {
  const enabled = primary.value !== "auto";
  secondary.disabled = !enabled;
  secondary.querySelectorAll("option").forEach(function (option) {
    option.disabled = option.value !== "none" && option.value === primary.value;
  });
  if (!enabled || secondary.value === primary.value) secondary.value = "none";
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
fillSecondaryLanguages($("setupSecondaryLanguage"));
fillSecondaryLanguages($("secondaryLanguage"));

function guessedLanguage() {
  const code = String(navigator.language || "en").toLowerCase().split("-")[0];
  return LANGUAGES.some(function (entry) { return entry[0] === code; }) ? code : "auto";
}

function syncLocalVoiceSettings() {
  if (!settings) return;
  const previous = JSON.parse(localStorage.getItem(LOCAL_KEY) || "{}");
  const next = Object.assign({}, previous, {
    sttProvider: settings.stt_provider,
    ttsProvider: settings.tts_provider,
    language: settings.language,
    secondaryLanguage: settings.secondary_language || "none",
    uiLocale: settings.ui_locale || "system",
    overlayEnabled: settings.overlay_enabled,
    plannerMode: settings.allow_online_ai ? "hybrid" : "local",
    layaEndpoint: settings.laya_endpoint,
    plannerEndpoint: settings.planner_endpoint,
    plannerModel: settings.planner_model,
  });
  localStorage.setItem(LOCAL_KEY, JSON.stringify(next));
}

async function persistSettings() {
  settings = await invoke("save_app_settings", { settings: settings });
  syncLocalVoiceSettings();
  renderSettings();
  return settings;
}

function renderSettings() {
  if (!settings) return;
  if ($("uiLocale")) $("uiLocale").value = settings.ui_locale || "system";
  $("language").value = settings.language;
  $("secondaryLanguage").value = settings.secondary_language || "none";
  updateSecondaryChoice($("language"), $("secondaryLanguage"));
  $("spokenFeedback").checked = Boolean(settings.spoken_feedback);
  $("startAtLogin").checked = Boolean(settings.start_at_login);
  $("overlayEnabled").checked = Boolean(settings.overlay_enabled);
  $("allowOnlineAi").checked = Boolean(settings.allow_online_ai);
  $("sttProvider").value = settings.stt_provider;
  if ($("ttsProvider")) $("ttsProvider").value = settings.tts_provider || "local";
  if ($("openRouterSttModel")) $("openRouterSttModel").value = settings.openrouter_stt_model || "";
  if ($("openRouterTtsModel")) $("openRouterTtsModel").value = settings.openrouter_tts_model || "";
  if ($("openRouterTtsVoice")) $("openRouterTtsVoice").value = settings.openrouter_tts_voice || "";
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

// The provider card holds one credential: the OpenRouter API key. Its scope
// badge only reports whether cloud speech is currently armed, so a stored key
// can never be misread as belonging to another provider or endpoint.
function providerScope() {
  if (!settings) return "local";
  const remoteSelected =
    settings.stt_provider === "openrouter" || settings.tts_provider === "openrouter";
  const keyConnected = Boolean(
    settings.openrouter_secret_ref && settings.openrouter_secret_ref.id,
  );
  return remoteSelected || keyConnected ? "openrouter" : "local";
}

const PROVIDER_TARGET = "openrouter";

function providerSecretRef() {
  return settings ? settings.openrouter_secret_ref : null;
}

function renderProviderStatus() {
  if (!settings) return;

  const scope = providerScope();
  const card = $("providerCard");
  if (card) card.dataset.provider = PROVIDER_TARGET;

  const scopeBadge = $("providerScopeBadge");
  if (scopeBadge) {
    // Keep the data-i18n key in sync so a later translateDom() renders the same
    // scope instead of overwriting it with the markup's default.
    const key = "provider.badge_" + scope;
    scopeBadge.dataset.i18n = key;
    scopeBadge.textContent = t(key, { default: scope.toUpperCase() });
  }

  const badge = $("providerStatusBadge");
  if (!badge) return;

  const testBtn = $("testProvider");
  const disconnectBtn = $("disconnectProvider");
  const msg = $("providerStatusMessage");
  const secretRef = providerSecretRef();
  const hasSecret = Boolean(secretRef && secretRef.id);

  if (hasSecret) {
    badge.textContent = t("provider.status_connected");
    badge.style.color = "#7de29f";
    badge.style.borderColor = "#21412c";
    badge.style.background = "#0d1811";
    if (testBtn) testBtn.disabled = false;
    if (disconnectBtn) disconnectBtn.disabled = false;
    if (msg && !msg.textContent) {
      msg.textContent = t("provider.connected_id", { id: secretRef.id });
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
      msg.textContent = scope === "openrouter"
        ? t("provider.remote_speech_requires_key", {
            default: "Connect an OpenRouter API key to use cloud speech.",
          })
        : "";
      msg.className = "provider-status-msg";
    }
  }

  // Opt-in reminder wins over the generic state: OpenRouter is never used while
  // online AI is off, so that is the actionable message.
  if (scope === "openrouter" && !settings.allow_online_ai && msg) {
    msg.textContent = t("provider.requires_online", {
      default: "Turn on 'Allow online AI and voice' to use OpenRouter.",
    });
    msg.className = "provider-status-msg error";
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
  $("setupSecondaryLanguage").value = settings.secondary_language || "none";
  updateSecondaryChoice($("setupLanguage"), $("setupSecondaryLanguage"));
  $("setupAutostart").checked = Boolean(settings.start_at_login);
  setupOrb.setState("ready");
  translateDom();
}

function showDashboard() {
  $("onboarding").classList.add("hidden");
  $("dashboard").classList.remove("hidden");
  const pane = document.querySelector(".workspace-pane");
  if (pane) pane.scrollTop = 0;
  renderSettings();
  renderRuntime(runtime);
  refreshHarnesses();
  refreshSkills();
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
    settings.secondary_language = $("setupSecondaryLanguage").value;
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
      document.querySelectorAll("#setupSecondaryLanguage option[value='none'], #secondaryLanguage option[value='none']").forEach(function (option) {
        option.textContent = t("settings.secondary_language_none");
      });
    }
  }
  settings.language = $("language").value;
  updateSecondaryChoice($("language"), $("secondaryLanguage"));
  settings.secondary_language = $("secondaryLanguage").value;
  settings.spoken_feedback = $("spokenFeedback").checked;
  settings.start_at_login = $("startAtLogin").checked;
  settings.overlay_enabled = $("overlayEnabled").checked;
  settings.allow_online_ai = $("allowOnlineAi").checked;
  settings.stt_provider = $("sttProvider").value;
  if ($("ttsProvider")) settings.tts_provider = $("ttsProvider").value;
  if ($("openRouterSttModel")) settings.openrouter_stt_model = $("openRouterSttModel").value.trim();
  if ($("openRouterTtsModel")) settings.openrouter_tts_model = $("openRouterTtsModel").value.trim();
  if ($("openRouterTtsVoice")) settings.openrouter_tts_voice = $("openRouterTtsVoice").value.trim();
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

let isSkillRecording = false;

async function refreshSkills() {
  if (!settings || !settings.setup_complete || !$("skillsList")) return;

  try {
    const list = await invoke("list_skills");
    if (!list || list.length === 0) {
      $("skillsList").innerHTML = '<span class="muted">' + t("skills.empty") + '</span>';
      return;
    }
    $("skillsList").innerHTML = list
      .map(function (s) {
        return '<div class="agent-pill ' + (s.enabled ? "connected" : "") + '">'
          + "<span>" + s.name + " (" + s.step_count + " steps)</span>"
          + '<button class="compact-btn secondary-button run-skill-btn" data-id="' + s.id + '">' + t("skills.run_btn") + '</button>'
          + "</div>";
      })
      .join("");

    $("skillsList").querySelectorAll(".run-skill-btn").forEach(function (btn) {
      btn.addEventListener("click", function (e) {
        e.stopPropagation();
        runSkill(btn.getAttribute("data-id"));
      });
    });
  } catch {
    $("skillsList").innerHTML = '<span class="muted">' + t("skills.empty") + '</span>';
  }
}

async function runSkill(skillId) {
  try {
    const res = await invoke("execute_skill", { id: skillId, inputs: null, sessionId: null });
    if (res.status === "success") {
      $("attentionBanner").classList.remove("hidden");
      $("attentionText").textContent = t("skills.execute_success", { name: skillId, completed: res.completed_steps, total: res.total_steps });
    } else if (res.status === "cancelled") {
      $("attentionBanner").classList.remove("hidden");
      $("attentionText").textContent = t("skills.execute_cancelled", { name: skillId });
    } else if (res.status === "confirm") {
      // Step triggered policy confirmation - already issued
    } else {
      $("attentionBanner").classList.remove("hidden");
      $("attentionText").textContent = t("skills.execute_failed", { name: skillId, step: (res.halted_at_step || 0) + 1, error: res.error || "error" });
    }
  } catch (err) {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = t("skills.execute_failed", { name: skillId, step: 1, error: String(err) });
  }
}

async function toggleSkillRecording() {
  const btn = $("toggleSkillRecording");
  const notice = $("skillRecordingNotice");
  if (!btn) return;

  if (!isSkillRecording) {
    try {
      await invoke("start_skill_recording");
      isSkillRecording = true;
      btn.textContent = t("skills.record_stop");
      if (notice) {
        notice.classList.remove("hidden");
        notice.textContent = t("skills.recording_active");
      }
    } catch (err) {
      console.error("Failed to start recording:", err);
    }
  } else {
    try {
      const count = await invoke("stop_skill_recording");
      isSkillRecording = false;
      btn.textContent = t("skills.record_start");
      if (notice) {
        notice.textContent = t("skills.recording_stopped") + " (" + count + " actions)";
      }
      if (count > 0) {
        const draftId = "draft_skill_" + Date.now();
        const draft = await invoke("compile_skill_draft", { id: draftId, name: "Recorded Workflow" });
        await invoke("save_skill", { skill: draft });
        await refreshSkills();
      }
    } catch (err) {
      console.error("Failed to stop recording:", err);
    }
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

    $("engineInfo").textContent = speech.active_backend === "openrouter"
      ? (speech.ready
          ? "OpenRouter · " + speech.model
          : t("advanced.diag_engine_stopped"))
      : speech.ready
        ? t("advanced.diag_engine_ready")
        : speech.running
          ? t("advanced.diag_engine_starting")
          : t("advanced.diag_engine_stopped");

    if ($("modelCacheInfo")) {
      const mb = (cacheBytes / (1024 * 1024)).toFixed(1);
      $("modelCacheInfo").textContent = t("advanced.diag_cache_mb", { mb: mb });
    }


    if ($("modelBackendInfo")) {
      if (speech.active_backend === "openrouter") {
        $("modelBackendInfo").textContent = speech.model;
      } else if (modelStatus) {
        $("modelBackendInfo").textContent = modelStatus.verified
          ? "Nemotron 3.5 Q4_K (Verified Active)"
          : "Nemotron 3.5 (" + modelStatus.state + ")";
      }
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
const TOOL_CAPABILITY_MAP = {
  "app.open": "desktop.launch",
  "browser.search": "browser.open",
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

let activeSpeechAudio = null;
let activeSpeechUrl = null;

function stopRemoteSpeech() {
  if (activeSpeechAudio) {
    try { activeSpeechAudio.pause(); } catch {}
    activeSpeechAudio = null;
  }
  if (activeSpeechUrl) {
    try { URL.revokeObjectURL(activeSpeechUrl); } catch {}
    activeSpeechUrl = null;
  }
}

function base64ToBytes(encoded) {
  const binary = atob(String(encoded || ""));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

// Never let ReflexDesk transcribe its own reply.
async function pauseListeningForSpeech() {
  if (!runtime || !runtime.listening) return true;
  try {
    renderRuntime(await invoke("set_listening", { active: false }));
    return true;
  } catch {
    return false;
  }
}

async function speakRemote(text) {
  if (!(await pauseListeningForSpeech())) return;

  try {
    const speech = await invoke("tts_speak", { text });
    const blob = new Blob([base64ToBytes(speech.audio_base64)], {
      type: speech.content_type || "audio/mpeg",
    });
    stopRemoteSpeech();
    activeSpeechUrl = URL.createObjectURL(blob);
    activeSpeechAudio = new Audio(activeSpeechUrl);
    activeSpeechAudio.addEventListener("ended", stopRemoteSpeech);
    await activeSpeechAudio.play();
  } catch (error) {
    stopRemoteSpeech();
    const message = String(error).slice(0, 150);
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = t("tts.playback_failed", {
      error: message,
      default: "Spoken reply failed: " + message,
    });
  }
}

async function speakActionResult(result) {
  if (!settings?.spoken_feedback || !["success", "deny", "denied", "error"].includes(result.status)) return;

  const phrase = result.status === "success"
    ? t("settings.spoken_feedback_done")
    : t("settings.spoken_feedback_denied");

  if (settings.tts_provider === "openrouter") {
    await speakRemote(phrase);
    return;
  }

  const synth = window.speechSynthesis;
  if (!synth || !window.SpeechSynthesisUtterance) return;

  let voices = synth.getVoices();
  if (!voices.length) {
    voices = await new Promise(function (resolve) {
      const timer = setTimeout(function () {
        synth.removeEventListener("voiceschanged", ready);
        resolve(synth.getVoices());
      }, 1000);
      function ready() {
        clearTimeout(timer);
        synth.removeEventListener("voiceschanged", ready);
        resolve(synth.getVoices());
      }
      synth.addEventListener("voiceschanged", ready);
    });
  }
  // Never use a provider-backed browser voice in offline mode.
  const localVoices = voices.filter(function (voice) { return voice.localService === true; });
  if (!localVoices.length) {
    $("attentionBanner").classList.remove("hidden");
    $("attentionText").textContent = t("settings.spoken_feedback_unavailable");
    return;
  }

  if (!(await pauseListeningForSpeech())) return;

  const locale = getLocale();
  const preferred = locale === "system" ? settings.language : locale;
  const voice = localVoices.find(function (v) { return v.lang.toLowerCase().startsWith(preferred + "-"); })
    || localVoices.find(function (v) { return v.lang.toLowerCase().startsWith("en-"); })
    || localVoices[0];
  const utterance = new SpeechSynthesisUtterance(phrase);
  utterance.voice = voice;
  utterance.lang = voice.lang;
  synth.cancel();
  synth.speak(utterance);
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
  void speakActionResult(result);
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

  const plannedActions =
    planned && Array.isArray(planned.actions) && planned.actions.length
      ? planned.actions
      : planned && planned.action && planned.action !== "unknown"
        ? [{ action: planned.action, args: planned.args || {} }]
        : [];

  for (const step of plannedActions) {
    if (!step || !step.action || step.action === "unknown") continue;
    const envelope = buildActionEnvelope(step.action, step.args || {}, "planner");
    const result = await requestActionWithConfirmation(envelope, cleanText);
    handleActionResult(result);
    if (!result || ["deny", "denied", "cancelled", "error"].includes(result.status)) {
      break;
    }
  }
}
async function bootstrap() {
  settings = await invoke("get_app_settings");
  runtime = await invoke("get_runtime_status");
  setLocale(settings.ui_locale || "system");
  translateDom();
  document.querySelectorAll("#setupSecondaryLanguage option[value='none'], #secondaryLanguage option[value='none']").forEach(function (option) {
    option.textContent = t("settings.secondary_language_none");
  });
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
if ($("refreshSkills")) $("refreshSkills").addEventListener("click", refreshSkills);
if ($("toggleSkillRecording")) $("toggleSkillRecording").addEventListener("click", toggleSkillRecording);
$("hideWindow").addEventListener("click", function () {
  getCurrentWindow().hide();
});

for (const id of [
  "uiLocale",
  "secondaryLanguage",
  "spokenFeedback",
  "language",
  "startAtLogin",
  "overlayEnabled",
  "allowOnlineAi",
  "sttProvider",
  "ttsProvider",
  "openRouterSttModel",
  "openRouterTtsModel",
  "openRouterTtsVoice",
  "layaEndpoint",
  "plannerEndpoint",
  "plannerModel",
]) {
  if ($(id)) $(id).addEventListener("change", saveDashboardSettings);
}
$("setupLanguage").addEventListener("change", function () {
  updateSecondaryChoice($("setupLanguage"), $("setupSecondaryLanguage"));
});
$("language").addEventListener("change", function () {
  updateSecondaryChoice($("language"), $("secondaryLanguage"));
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
    const provider = PROVIDER_TARGET;
    if (!key) {
      if (msg) {
        msg.textContent = t("provider.enter_key_prompt");
        msg.className = "provider-status-msg error";
      }
      return;
    }

    try {
      $("connectProvider").disabled = true;
      $("connectProvider").textContent = t("provider.connecting_button");
      const status = await invoke("connect_provider", {
        provider: provider,
        apiKey: key,
      });

      // Immediately zeroize / clear DOM input
      if (keyInput) keyInput.value = "";

      settings = await invoke("get_app_settings");
      if (msg) msg.textContent = "";
      renderSettings();

      if (msg) {
        msg.textContent = status.message || t("provider.connected_success");
        msg.className = status.last_status === "connected"
          ? "provider-status-msg success"
          : "provider-status-msg error";
      }
    } catch (err) {
      if (msg) {
        msg.textContent = t("provider.connection_failed", { error: String(err).slice(0, 150) });
        msg.className = "provider-status-msg error";
      }
    } finally {
      const button = $("connectProvider");
      if (button) {
        button.disabled = false;
        button.textContent = t("provider.connect_button");
      }
    }
  });
}

if ($("testProvider")) {
  $("testProvider").addEventListener("click", async function () {
    const msg = $("providerStatusMessage");
    try {
      $("testProvider").disabled = true;
      $("testProvider").textContent = t("provider.testing_button");
      const status = await invoke("test_provider", { provider: PROVIDER_TARGET });
      if (msg) {
        msg.textContent = status.message || t("provider.connected_success");
        msg.className = status.last_status === "connected"
          ? "provider-status-msg success"
          : "provider-status-msg error";
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
      const button = $("testProvider");
      if (button) {
        button.disabled = false;
        button.textContent = t("provider.test_button");
      }
    }
  });
}

if ($("disconnectProvider")) {
  $("disconnectProvider").addEventListener("click", async function () {
    const msg = $("providerStatusMessage");
    try {
      $("disconnectProvider").disabled = true;
      await invoke("disconnect_provider", { provider: PROVIDER_TARGET });
      settings = await invoke("get_app_settings");
      if (msg) msg.textContent = "";
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
      const button = $("disconnectProvider");
      if (button) button.disabled = false;
    }
  });
}

bootstrap().catch(function (error) {
  document.body.innerHTML =
    '<main class="fatal"><h1>ReflexDesk could not start.</h1><p>'
    + String(error)
    + "</p></main>";
});
