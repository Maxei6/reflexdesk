import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

test("TestMatrix: matrix.json tracks targets without fabricating OS measurements", () => {
  const matrixPath = path.resolve("tests/matrix.json");
  assert.ok(fs.existsSync(matrixPath), "matrix.json must exist");

  const matrix = JSON.parse(fs.readFileSync(matrixPath, "utf8"));
  assert.equal(matrix.version, 1);
  assert.ok(matrix.targets);
  assert.ok(matrix.platforms);

  const targets = matrix.targets;
  assert.ok(targets.completion_rate_target > 0);
  assert.ok(targets.false_action_rate_target > 0);
  assert.ok(targets.max_cancel_latency_ms > 0);
  assert.ok(targets.crash_free_soak_hours_target > 0);

  const requiredPlatforms = ["windows", "macos", "linux"];
  for (const plat of requiredPlatforms) {
    const report = matrix.platforms[plat];
    assert.ok(report, `Platform ${plat} must be present in matrix.json`);
    assert.ok(["PASS", "UNMEASURED"].includes(report.status));

    if (report.status === "UNMEASURED") {
      for (const metric of [
        "completion_rate",
        "false_action_rate",
        "p50_cancel_latency_ms",
        "p95_cancel_latency_ms",
        "crash_free_soak_hours",
        "fault_injection_resilience"
      ]) {
        assert.equal(report[metric], null, `${plat} ${metric} must remain null until measured`);
      }
      continue;
    }

    assert.ok(report.completion_rate >= targets.completion_rate_target);
    assert.ok(report.false_action_rate <= targets.false_action_rate_target);
    assert.ok(report.p95_cancel_latency_ms <= targets.max_cancel_latency_ms);
    assert.ok(report.crash_free_soak_hours >= targets.crash_free_soak_hours_target);
    assert.equal(report.fault_injection_resilience, 1.0);
  }

  // Verify lab-gated cases are documented with deterministic seams
  assert.ok(matrix.lab_gated_cases);
  const requiredLabCases = [
    "physical_audio",
    "sleep_resume",
    "bluetooth_headset",
    "display_dpi_scaling",
    "permission_revocation"
  ];
  for (const labCase of requiredLabCases) {
    const entry = matrix.lab_gated_cases[labCase];
    assert.ok(entry, `Lab case ${labCase} must be documented`);
    assert.ok(entry.deterministic_seam, `Lab case ${labCase} must define deterministic seam`);
    assert.ok(entry.validation_criteria, `Lab case ${labCase} must define validation criteria`);
    assert.equal(entry.status, "lab-gated");
  }
});
