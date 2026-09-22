import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import { evaluateEntryCriteria } from "../scripts/soak-entry-check.mjs";
import {
  redactText,
  redactUrl,
  redactError,
  sanitizeMeta,
  sanitizeLogEvent,
  computeMetricsFromEvents,
  runFaultInjectionSimulation,
  verifySoakMatrix,
  ALLOWLISTED_FIELDS,
  DENIED_META_KEYS
} from "../scripts/soak-collect.mjs";

test("SoakThresholds: thresholds.json defines frozen quantitative release gates", () => {
  const file = path.resolve("tests/soak/thresholds.json");
  assert.ok(fs.existsSync(file), "tests/soak/thresholds.json must exist");

  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.equal(data.version, 1);
  assert.equal(data.status, "frozen");
  assert.ok(data.rationale);
  assert.ok(Array.isArray(data.rules) && data.rules.length > 0);

  const t = data.thresholds;
  assert.ok(t, "thresholds dictionary must be present");

  // Plan 19 quantitative gates
  assert.ok(t.crash_free_sessions_pct.target >= 99.0);
  assert.equal(t.crash_free_sessions_pct.direction, "min");

  assert.ok(t.crash_free_runtime_hours.target >= 72.0);
  assert.equal(t.crash_free_runtime_hours.direction, "min");

  assert.ok(t.task_completion_rate.target >= 0.95);
  assert.equal(t.task_completion_rate.direction, "min");

  assert.ok(t.false_action_rate.target <= 0.05);
  assert.equal(t.false_action_rate.direction, "max");

  assert.ok(t.cancel_latency_p95_ms.target <= 200);
  assert.equal(t.cancel_latency_p95_ms.direction, "max");

  assert.ok(t.startup_readiness_p95_ms.target <= 5000);
  assert.equal(t.startup_readiness_p95_ms.direction, "max");

  assert.ok(t.max_memory_growth_mb_per_24h.target <= 100);
  assert.equal(t.max_memory_growth_mb_per_24h.direction, "max");

  assert.equal(t.orphan_process_count.target, 0, "Strict 0 orphan processes tolerated");
  assert.equal(t.orphan_process_count.direction, "max");

  assert.ok(t.update_success_rate.target >= 0.95);
  assert.equal(t.update_rollback_rate.target, 0.0, "Zero state corruption on failed updates");
});

test("SoakMatrix: matrix.json covers all Plan 19 OS, hardware, and scenario targets", () => {
  const file = path.resolve("tests/soak/matrix.json");
  assert.ok(fs.existsSync(file), "tests/soak/matrix.json must exist");

  const matrix = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.equal(matrix.version, 1);
  assert.equal(matrix.status, "IN PROGRESS / BLOCKED ON HARDWARE");

  // 1. Operating systems coverage
  const os = matrix.operating_systems;
  assert.ok(os.windows?.length >= 2, "Must include Windows 11 and Windows 10");
  assert.ok(os.macos?.length >= 2, "Must include macOS 15 and macOS 14");
  assert.ok(os.linux?.length >= 2, "Must include Ubuntu and Fedora targets");

  // 2. Hardware profiles coverage
  const hw = matrix.hardware_profiles;
  assert.ok(hw.some(h => h.id === "low-end-cpu"), "Must include low-end CPU-only");
  assert.ok(hw.some(h => h.id === "modern-x86"), "Must include modern x86");
  assert.ok(hw.some(h => h.id === "apple-silicon"), "Must include Apple Silicon");
  assert.ok(hw.some(h => h.id === "nvidia-gpu"), "Must include NVIDIA discrete GPU");
  assert.ok(hw.some(h => h.id === "multi-dpi-monitors"), "Must include multi-DPI/monitors");

  // 3. Scenarios coverage
  const scn = matrix.scenarios;
  assert.ok(scn.some(s => s.id === "SCN-01-runtime-24h"), "24h background scenario required");
  assert.ok(scn.some(s => s.id === "SCN-02-runtime-72h"), "72h continuous soak scenario required");
  assert.ok(scn.some(s => s.id === "SCN-03-listen-cycles"), "Repeated listen cycles required");
  assert.ok(scn.some(s => s.id === "SCN-04-sleep-resume"), "Sleep/resume required");
  assert.ok(scn.some(s => s.id === "SCN-05-network-loss"), "Network loss/return required");
  assert.ok(scn.some(s => s.id === "SCN-06-device-switching"), "Device switching required");
  assert.ok(scn.some(s => s.id === "SCN-07-agent-lifecycle"), "Agent lifecycle required");
  assert.ok(scn.some(s => s.id === "SCN-08-interrupted-model-update"), "Model/update interruption required");
  assert.ok(scn.some(s => s.id === "SCN-09-crash-recovery"), "Crash recovery required");
  assert.ok(scn.some(s => s.id === "SCN-10-setup-upgrade-uninstall"), "Setup/upgrade/uninstall required");
  assert.ok(scn.some(s => s.id === "SCN-11-shortcut-conflicts"), "Shortcut conflicts required");
  assert.ok(scn.some(s => s.id === "SCN-12-disk-pressure"), "Disk pressure required");

  // Invariant: unmeasured physical fleet values must strictly be null
  for (const winTarget of os.windows) {
    if (winTarget.status === "blocked_on_hardware") {
      assert.equal(winTarget.measured_metrics.crash_free_hours, null);
      assert.equal(winTarget.measured_metrics.completion_rate, null);
    }
  }
});

test("SoakBlockers: blockers.json tracks release blockers and exit criteria", () => {
  const file = path.resolve("tests/soak/blockers.json");
  assert.ok(fs.existsSync(file), "tests/soak/blockers.json must exist");

  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.equal(data.version, 1);
  assert.equal(data.exit_criteria_met, false, "Exit criteria cannot be met while hardware/credentials blockers remain");
  assert.equal(data.summary.ready_for_release, false);

  const blockers = data.blockers;
  assert.ok(blockers.some(b => b.id === "BLK-001" && b.status === "BLOCKED_ON_HARDWARE"));
  assert.ok(blockers.some(b => b.id === "BLK-002" && b.status === "BLOCKED_ON_CREDENTIALS"));

  for (const b of blockers) {
    assert.ok(b.title);
    assert.ok(b.plan_id);
    assert.ok(b.severity);
    assert.ok(b.resolution_criteria);
    assert.ok(b.verification_step);
  }
});

test("SoakEntryCheck: evaluateEntryCriteria validates all 5 entry conditions", () => {
  const result = evaluateEntryCriteria();
  assert.ok(result);
  assert.ok(result.criteria.core_p1_plans);
  assert.ok(result.criteria.security_findings);
  assert.ok(result.criteria.release_workflow);
  assert.ok(result.criteria.updater_staging);
  assert.ok(result.criteria.e2e_suite);

  // Core plans are all in place
  assert.equal(result.criteria.core_p1_plans.status, "PASS");
  assert.equal(result.criteria.security_findings.status, "PASS");
  assert.equal(result.criteria.e2e_suite.status, "PASS");
});

test("SoakRedaction: observability redaction strips secrets, URLs, and denied keys", () => {
  // 1. Text redaction
  const rawText = "Bearer eyJhbGciOiJIUzI1NiJ9 and sk-1234567890abcdef1234 with /Users/alice/secrets.txt";
  const redacted = redactText(rawText);
  assert.ok(!redacted.includes("eyJhbGciOiJIUzI1NiJ9"));
  assert.ok(!redacted.includes("sk-1234567890abcdef1234"));
  assert.ok(!redacted.includes("/Users/alice"));
  assert.ok(redacted.includes("[REDACTED_TOKEN]"));
  assert.ok(redacted.includes("[REDACTED_KEY]"));
  assert.ok(redacted.includes("[REDACTED_USER_PATH]"));

  // 2. URL redaction (scheme + host only)
  const fullUrl = "https://api.openai.com/v1/chat/completions?api_key=secret#hash";
  const safeUrl = redactUrl(fullUrl);
  assert.equal(safeUrl, "https://api.openai.com");

  // 3. Metadata sanitization
  const meta = {
    model: "nemotron",
    api_key: "super-secret-key",
    user_token: "jwt-token-value",
    transcript: "Open my banking app",
    safe_latency_ms: 120
  };
  const sanitized = sanitizeMeta(meta);
  assert.equal(sanitized.model, "nemotron");
  assert.equal(sanitized.api_key, "[REDACTED_SENSITIVE_KEY]");
  assert.equal(sanitized.user_token, "[REDACTED_SENSITIVE_KEY]");
  assert.equal(sanitized.transcript, "[REDACTED_SENSITIVE_KEY]");
  assert.equal(sanitized.safe_latency_ms, 120);

  // 4. Log event allowlisting
  const event = {
    v: 1,
    ts: Date.now(),
    component: "stt",
    op: "transcribe",
    outcome: "success",
    latency_ms: 45,
    forbidden_audio_bytes: "0101010101",
    forbidden_prompt_dump: "raw system prompt"
  };
  const safeEvent = sanitizeLogEvent(event);
  assert.equal(safeEvent.component, "stt");
  assert.equal(safeEvent.latency_ms, 45);
  assert.equal(safeEvent.forbidden_audio_bytes, undefined, "Non-allowlisted fields must be dropped");
  assert.equal(safeEvent.forbidden_prompt_dump, undefined, "Non-allowlisted fields must be dropped");
});

test("SoakMetrics: computeMetricsFromEvents computes correct rates and percentiles", () => {
  const events = [
    { component: "lifecycle", op: "session_start", outcome: "success" },
    { component: "tool", op: "action_execute", outcome: "success" },
    { component: "tool", op: "action_execute", outcome: "success" },
    { component: "tool", op: "action_execute", outcome: "failure" },
    { component: "policy", op: "cancel_session", outcome: "cancelled", latency_ms: 50 },
    { component: "policy", op: "cancel_session", outcome: "cancelled", latency_ms: 100 },
    { component: "policy", op: "cancel_session", outcome: "cancelled", latency_ms: 150 },
    { component: "lifecycle", op: "session_start", outcome: "crash" }
  ];

  const m = computeMetricsFromEvents(events);
  assert.equal(m.total_sessions, 2);
  assert.equal(m.crashed_sessions, 1);
  assert.equal(m.crash_free_sessions_pct, 50.0);
  assert.equal(m.total_tasks, 3);
  assert.equal(m.successful_tasks, 2);
  assert.equal(Math.round(m.task_completion_rate * 100), 67);
  assert.equal(m.cancel_measurements_count, 3);
  assert.equal(m.p50_cancel_latency_ms, 100);
  assert.equal(m.p95_cancel_latency_ms, 150);
});

test("SoakSimulation: runFaultInjectionSimulation executes Plan 13 hooks cleanly", async () => {
  const result = await runFaultInjectionSimulation();
  assert.ok(result.fault_injection.all_passed, "All 13 fault injection hooks must pass");
  assert.equal(result.fault_injection.passed_hooks, 13);
  assert.ok(result.metrics.p95_cancel_latency_ms <= 200, "P95 cancel latency must be <= 200ms");
  assert.ok(result.events_count > 100);
});

test("SoakAudit: verifySoakMatrix reports BLOCKED ON HARDWARE for unmeasured fleet", () => {
  const audit = verifySoakMatrix("tests/soak/matrix.json");
  assert.equal(audit.matrix_status, "IN PROGRESS / BLOCKED ON HARDWARE");
  assert.equal(audit.is_blocked_on_hardware, true);
  assert.ok(audit.unmeasured_count > 0);
});
