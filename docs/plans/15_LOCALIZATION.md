# Plan 15 — Product localization

**Priority:** P2  
**Status:** ACCEPTANCE PENDING

## Objective

Speech is multilingual already; the product UI and recovery messages should be
too.

## Work packages

1. Extract all visible strings into locale catalogs.
2. Locale fallback rules.
3. OS-locale default on first run.
4. User-selectable UI language separate from speech language.
5. RTL layout test for Arabic/Hebrew.
6. plural/date/number formatting.
7. localize installer/update/onboarding errors.
8. translation QA for safety/confirmation wording.

## Initial languages

English and Italian first, then Spanish, French, German and Portuguese based on
contributors/user demand.

## Definition of done

No production UI string is hard-coded in the main application flow, and safety
confirmations retain equivalent meaning across supported locales.

## Acceptance Notes

- **Locale Catalogs:** Extracted all visible strings across onboarding, dashboard, settings, provider, agents, advanced diagnostics, policy gate modal, and tray states into `src/locales/{en,it,es,fr,de,pt}.json`. Added test fixtures `src/locales/ar.json` and `src/locales/he.json`.
- **Fallback Rules:** Implemented in `src/lib/i18n.js::t(id, params)` with explicit English catalog fallback when keys are missing in active language.
- **OS-Locale Default:** Implemented via `resolveSystemLocale()`, selecting matching supported language or falling back to English.
- **User-Selectable UI Language:** Added persisted `ui_locale` (`system|<bcp47>`, default `"system"`) in `AppSettings` (schema v2 with automated migration in `src-tauri/src/settings.rs`), distinct from speech `language`. Added `#uiLocale` dropdown in Settings General view.
- **RTL Layout:** Added `[dir="rtl"]` CSS rules in `src/styles.css`, runtime `dir="rtl"` and `lang` application in `i18n.js`, and automated tests for Arabic/Hebrew RTL detection and fallback.
- **Formatters:** Added `formatNumber`, `formatDate`, `formatPlural`, and `{count, plural}` token interpolation using standard `Intl.PluralRules`, `Intl.NumberFormat`, and `Intl.DateTimeFormat`.
- **Localized Errors:** `localizeError` maps error codes (`unknown-tool`, `negation-detected`, `invalid-args`, `auth-invalid`, `mic-permission-denied`, `stt-unavailable`, `model-download-failed`, `action-cancelled`, `policy-denied`) to localized recovery text.
- **Safety & Confirmation Translation QA:** Policy gate modal strings (`policy.modal.title`, `policy.modal.kicker`, `policy.modal.approve`, `policy.modal.deny`, risk classes) and confirmation prompts (`settings.reset_confirm`, `agents.confirm_start`) verified in automated test suite `tests/localization.test.mjs` across all locales; approve/deny consequences are identical and policy args summaries stay un-redacted/verbatim in every locale.
