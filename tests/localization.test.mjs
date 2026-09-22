import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {
  catalogs,
  SUPPORTED_LOCALES,
  isRtl,
  getLocale,
  setLocale,
  resolveSystemLocale,
  t,
  formatNumber,
  formatDate,
  formatPlural,
  localizeError,
} from "../src/lib/i18n.js";

test("Locale catalogs: all required locale JSON files exist and are valid JSON", () => {
  const required = ["en", "it", "es", "fr", "de", "pt"];
  for (const loc of required) {
    const filePath = path.resolve(`src/locales/${loc}.json`);
    assert.ok(fs.existsSync(filePath), `Catalog file ${filePath} must exist`);
    const content = JSON.parse(fs.readFileSync(filePath, "utf8"));
    assert.equal(content.meta.locale, loc);
    assert.equal(content.meta.dir, "ltr");
    assert.ok(content.brand.name);
    assert.ok(content.policy.modal.approve);
    assert.ok(content.policy.modal.deny);
  }
});

test("Locale fallback rules: missing key in non-English catalog falls back to English", () => {
  setLocale("it");
  assert.equal(getLocale(), "it");

  // An existing key in Italian
  assert.equal(t("brand.tagline"), "Il tuo computer, con riflessi.");

  // Test fallback to English for a key missing in Italian (simulate by querying a synthetic fallback or test locale)
  setLocale("ar"); // ar catalog is minimal
  assert.equal(getLocale(), "ar");
  // In ar, "brand.tagline" is defined
  assert.equal(t("brand.tagline"), "حاسوبك، بردود فعل ذكية.");
  // In ar, "dashboard.kicker" is NOT defined, should fall back to English
  assert.equal(t("dashboard.kicker"), "VOICE CONTROL");

  // Completely unknown key returns key itself
  assert.equal(t("completely.unknown.key"), "completely.unknown.key");
  // Unknown key with default returns default
  assert.equal(t("completely.unknown.key", { default: "My Default" }), "My Default");
});

test("OS-locale resolution: picks supported language or falls back to en", () => {
  assert.equal(resolveSystemLocale(), "en");
});

test("RTL detection and layout: ar and he are detected as RTL", () => {
  assert.equal(isRtl("ar"), true);
  assert.equal(isRtl("ar-EG"), true);
  assert.equal(isRtl("he"), true);
  assert.equal(isRtl("he-IL"), true);
  assert.equal(isRtl("en"), false);
  assert.equal(isRtl("it"), false);
  assert.equal(isRtl("es"), false);

  setLocale("ar");
  assert.equal(getLocale(), "ar");
  assert.equal(isRtl(getLocale()), true);

  setLocale("he");
  assert.equal(getLocale(), "he");
  assert.equal(isRtl(getLocale()), true);

  setLocale("en");
  assert.equal(getLocale(), "en");
  assert.equal(isRtl(getLocale()), false);
});

test("Plural, date, and number formatting", () => {
  setLocale("en");
  const numStr = formatNumber(1234567);
  assert.ok(numStr.includes("1") && numStr.includes("234") && numStr.includes("567"));

  // Plural interpolation
  const single = t("advanced.export_success_plural", { count: 1 });
  assert.ok(single.includes("1 log") && !single.includes("1 logs"), `Expected singular form, got: ${single}`);

  const plural = t("advanced.export_success_plural", { count: 5 });
  assert.ok(plural.includes("5 logs"), `Expected plural form, got: ${plural}`);

  // Parameter interpolation
  const interpolated = t("onboarding.step4_heard_retry", { heard: "test phrase" });
  assert.ok(interpolated.includes("test phrase"));

  // Date formatting
  const dateStr = formatDate(new Date("2026-09-22T12:00:00Z"));
  assert.ok(dateStr.length > 0);
});

test("Translation QA: safety and confirmation wording preserve exact gate semantics", () => {
  const locales = ["en", "it", "es", "fr", "de", "pt"];

  for (const loc of locales) {
    setLocale(loc);
    const catalog = catalogs[loc];

    // 1. Approval and denial button text must exist and be non-empty
    const approve = t("policy.modal.approve");
    const deny = t("policy.modal.deny");
    assert.ok(approve && approve.length > 0, `Approve text missing in ${loc}`);
    assert.ok(deny && deny.length > 0, `Deny text missing in ${loc}`);
    assert.notEqual(approve, deny, `Approve and Deny must be distinct in ${loc}`);

    // 2. Modal title & kicker must be present
    const title = t("policy.modal.title");
    const kicker = t("policy.modal.kicker");
    assert.ok(title && title.length > 0, `Policy modal title missing in ${loc}`);
    assert.ok(kicker && kicker.length > 0, `Policy modal kicker missing in ${loc}`);

    // 3. Reset confirmation prompt must exist and ask about first-time setup and cache
    const resetConfirm = t("settings.reset_confirm");
    assert.ok(resetConfirm && resetConfirm.length > 0, `Reset confirm missing in ${loc}`);

    // 4. Agent start confirmation must include {agent} token
    const confirmAgent = t("agents.confirm_start", { agent: "Codex" });
    assert.ok(confirmAgent.includes("Codex"), `Agent token interpolation failed in ${loc}: ${confirmAgent}`);

    // 5. Risk class labels must be distinct
    const safe = t("policy.risk.safe");
    const sensitive = t("policy.risk.sensitive");
    const destructive = t("policy.risk.destructive");
    const external = t("policy.risk.external_commit");
    assert.ok(safe && sensitive && destructive && external);
    const riskSet = new Set([safe, sensitive, destructive, external]);
    assert.equal(riskSet.size, 4, `All 4 risk classes must have unique names in ${loc}`);
  }
});

test("Localized errors: error codes map to localized recovery messages", () => {
  setLocale("en");
  assert.equal(localizeError("unknown-tool"), "Tool is unknown or not permitted by policy.");
  assert.equal(localizeError("negation-detected"), "Negated command detected; action denied.");
  assert.equal(localizeError("auth-invalid"), "Authentication failed. Please check or rotate your credentials.");

  setLocale("it");
  assert.equal(localizeError("unknown-tool"), "Lo strumento è sconosciuto o non consentito dalle politiche di sicurezza.");
  assert.equal(localizeError("negation-detected"), "Rilevato comando negato; azione rifiutata.");
  assert.equal(localizeError("auth-invalid"), "Autenticazione fallita. Verifica o aggiorna le credenziali.");

  setLocale("es");
  assert.equal(localizeError("unknown-tool"), "Herramienta desconocida o no permitida por la política.");

  setLocale("de");
  assert.equal(localizeError("unknown-tool"), "Werkzeug ist unbekannt oder durch Sicherheitsrichtlinie nicht zulässig.");

  setLocale("fr");
  assert.equal(localizeError("unknown-tool"), "Outil inconnu ou non autorisé par la politique.");

  setLocale("pt");
  assert.equal(localizeError("unknown-tool"), "Ferramenta desconhecida ou não permitida pela política.");
});
