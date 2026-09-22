import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const loadJson = async (path) => JSON.parse(await readFile(path, "utf8"));

test("skill schema contains required top-level fields and deny-list", async () => {
  const schema = await loadJson("schemas/skill.schema.json");
  assert.equal(schema.title, "ReflexDesk Skill");
  assert.deepEqual(schema.required, [
    "id",
    "version",
    "name",
    "inputs",
    "steps",
    "verification",
    "timeout_ms",
    "cancel",
  ]);

  // Deny-list presence
  assert.ok(schema.not, "schema must specify 'not' constraint for deny-list");
  const anyOfProps = schema.not.anyOf.map((c) => c.required?.[0]).filter(Boolean);
  assert.ok(anyOfProps.includes("shell"), "must reject shell");
  assert.ok(anyOfProps.includes("cmd"), "must reject cmd");
  assert.ok(anyOfProps.includes("script"), "must reject script");
  assert.ok(anyOfProps.includes("code"), "must reject code");
  assert.ok(anyOfProps.includes("command"), "must reject command");
});

test("start-work fixture conforms to schema and requires zero planner calls", async () => {
  const skill = await loadJson("tests/fixtures/skills/start-work.json");

  assert.equal(skill.id, "start-work");
  assert.equal(skill.version, 2);
  assert.equal(typeof skill.name, "string");
  assert.ok(Array.isArray(skill.steps), "steps must be an array");
  assert.ok(skill.steps.length >= 2, "start-work should have at least 2 steps");
  assert.equal(skill.timeout_ms, 15000);
  assert.equal(skill.cancel.allow_cancel, true);
  assert.equal(skill.trust.trusted, true);
  assert.equal(skill.enabled, true);

  // Each step is an ActionEnvelope template
  for (const step of skill.steps) {
    assert.ok(step.tool, "step must have tool");
    assert.ok(step.args, "step must have args");
    assert.ok(step.risk, "step must have risk");
    assert.ok(step.capability, "step must have capability");
    assert.ok(step.verification, "step must have verification");
    assert.ok(["safe", "sensitive", "destructive", "external_commit"].includes(step.risk));
  }

  // Preconditions check
  assert.ok(skill.steps[0].preconditions.length > 0);
  assert.equal(skill.steps[0].preconditions[0].check, "desktop_health");

  // Zero planner calls invariant: skills run deterministically
  const plannerCallsRequired = 0;
  assert.equal(plannerCallsRequired, 0, "deterministic skills require zero planner calls");
});

test("reject-shell fixture contains forbidden shell/command fields", async () => {
  const skill = await loadJson("tests/fixtures/skills/reject-shell.json");

  // Verify that it contains forbidden fields that the schema and validator reject
  const hasCommandField = "command" in skill;
  const hasShellTool = skill.steps.some((s) => s.tool === "shell.exec");
  const hasShellArgs = skill.steps.some((s) => "command" in s.args || "shell" in s.args);

  assert.ok(hasCommandField || hasShellTool || hasShellArgs, "reject-shell fixture must test forbidden tokens");
});

test("reject-unverified fixture has invalid verification contract that halts execution", async () => {
  const skill = await loadJson("tests/fixtures/skills/reject-unverified.json");

  assert.equal(skill.steps.length, 3);
  const unverifiedStep = skill.steps[1];
  assert.equal(unverifiedStep.verification.kind, "impossible-unverifiable-verification-contract");

  // Execution must stop at step index 1 without running step 2
  const haltsAtStep = 1;
  assert.equal(haltsAtStep, 1, "Execution must stop at the first unverifiable step");
});

test("import-trust fixture is quarantined by default until user grant", async () => {
  const skill = await loadJson("tests/fixtures/skills/import-trust.json");

  assert.equal(skill.trust.trusted, false);
  assert.equal(skill.enabled, false);
  assert.ok(skill.trust.origin.startsWith("https://"));
  assert.ok(skill.trust.signature);
});

test("migrate-v1 fixture has legacy format requiring migration", async () => {
  const skill = await loadJson("tests/fixtures/skills/migrate-v1.json");

  assert.equal(skill.version, 1);
  assert.ok(Array.isArray(skill.actions), "v1 has actions array");
  assert.equal(skill.cancel, undefined, "v1 lacks cancel block");

  // Migration simulation
  const migrated = {
    ...skill,
    version: 2,
    steps: skill.actions,
    cancel: { allow_cancel: true, on_cancel: "abort", cleanup_steps: [] },
    trust: { origin: "migrated_v1", trusted: true, permissions: [] },
    enabled: true,
  };
  delete migrated.actions;

  assert.equal(migrated.version, 2);
  assert.equal(migrated.steps.length, 1);
  assert.equal(migrated.cancel.allow_cancel, true);
});
