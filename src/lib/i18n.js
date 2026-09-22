import en from "../locales/en.json" with { type: "json" };
import it from "../locales/it.json" with { type: "json" };
import es from "../locales/es.json" with { type: "json" };
import fr from "../locales/fr.json" with { type: "json" };
import de from "../locales/de.json" with { type: "json" };
import pt from "../locales/pt.json" with { type: "json" };
import ar from "../locales/ar.json" with { type: "json" };
import he from "../locales/he.json" with { type: "json" };

export const catalogs = {
  en,
  it,
  es,
  fr,
  de,
  pt,
  ar,
  he,
};

export const SUPPORTED_LOCALES = [
  { code: "en", name: "English", dir: "ltr" },
  { code: "it", name: "Italiano", dir: "ltr" },
  { code: "es", name: "Español", dir: "ltr" },
  { code: "fr", name: "Français", dir: "ltr" },
  { code: "de", name: "Deutsch", dir: "ltr" },
  { code: "pt", name: "Português", dir: "ltr" },
];

export const RTL_LOCALES = new Set(["ar", "he", "fa", "ur"]);

let currentLocale = "en";

export function isRtl(locale) {
  const code = String(locale || "").toLowerCase().split("-")[0];
  return RTL_LOCALES.has(code);
}

export function getLocale() {
  return currentLocale;
}

export function resolveSystemLocale() {
  const navLang = typeof navigator !== "undefined"
    ? (navigator.languages && navigator.languages[0]) || navigator.language || ""
    : "";
  const primary = String(navLang).toLowerCase().split("-")[0];
  if (SUPPORTED_LOCALES.some((l) => l.code === primary)) {
    return primary;
  }
  return "en";
}

export function setLocale(code) {
  let resolved = code;
  if (!code || code === "system") {
    resolved = resolveSystemLocale();
  } else {
    const clean = String(code).toLowerCase().split("-")[0];
    if (catalogs[clean]) {
      resolved = clean;
    } else if (catalogs[code]) {
      resolved = code;
    } else {
      resolved = "en";
    }
  }

  currentLocale = resolved;

  if (typeof document !== "undefined" && document.documentElement) {
    document.documentElement.lang = currentLocale;
    document.documentElement.dir = isRtl(currentLocale) ? "rtl" : "ltr";
  }

  if (typeof window !== "undefined" && typeof window.dispatchEvent === "function") {
    try {
      window.dispatchEvent(
        new CustomEvent("reflexdesk://locale-changed", {
          detail: { locale: currentLocale, dir: isRtl(currentLocale) ? "rtl" : "ltr" },
        })
      );
    } catch {}
  }

  return currentLocale;
}

function getDeepValue(obj, path) {
  if (!obj || typeof obj !== "object") return undefined;
  const parts = path.split(".");
  let cur = obj;
  for (const part of parts) {
    if (cur === undefined || cur === null) return undefined;
    cur = cur[part];
  }
  return cur;
}

export function t(id, params) {
  if (!id) return "";

  const activeCatalog = catalogs[currentLocale] || catalogs.en;
  let val = getDeepValue(activeCatalog, id);

  // Explicit English fallback
  if (val === undefined && currentLocale !== "en") {
    val = getDeepValue(catalogs.en, id);
  }

  // If missing altogether
  if (val === undefined) {
    return params && params.default !== undefined ? String(params.default) : id;
  }

  // Handle plural selection if entry is an object or count is given
  if (params && typeof params.count === "number") {
    if (typeof val === "object" && val !== null) {
      try {
        const pr = new Intl.PluralRules(currentLocale);
        const rule = pr.select(params.count);
        val = val[rule] || val.other || Object.values(val)[0] || id;
      } catch {
        val = val.other || Object.values(val)[0] || id;
      }
    }
  }

  if (typeof val !== "string") {
    return id;
  }

  // Interpolate {param} tokens
  if (params && typeof params === "object") {
    return val.replace(/\{([a-zA-Z0-9_]+)\}/g, (match, key) => {
      if (key === "count" && typeof params.count === "number") {
        return formatNumber(params.count);
      }
      return params[key] !== undefined ? String(params[key]) : match;
    });
  }

  return val;
}

export function formatNumber(number, options) {
  try {
    return new Intl.NumberFormat(currentLocale, options).format(number);
  } catch {
    return String(number);
  }
}

export function formatDate(date, options) {
  try {
    const d = date instanceof Date ? date : new Date(date);
    return new Intl.DateTimeFormat(currentLocale, options).format(d);
  } catch {
    return String(date);
  }
}

export function formatPlural(count, forms) {
  if (!forms || typeof forms !== "object") return "";
  try {
    const pr = new Intl.PluralRules(currentLocale);
    const rule = pr.select(count);
    return forms[rule] || forms.other || "";
  } catch {
    return forms.other || "";
  }
}

export function localizeError(error, params) {
  if (!error) return "";
  const errStr = typeof error === "object" && error.code ? error.code : String(error);

  if (errStr.includes("unknown-tool")) {
    return t("errors.unknown_tool");
  }
  if (errStr.includes("negation-detected")) {
    return t("errors.negation_detected");
  }
  if (errStr.includes("invalid-args")) {
    return t("errors.invalid_args");
  }
  if (errStr.includes("auth-invalid") || errStr.includes("401") || errStr.includes("403")) {
    return t("errors.auth_invalid");
  }
  if (errStr.includes("mic-permission-denied") || errStr.includes("NotAllowedError") || errStr.includes("Permission denied")) {
    return t("errors.mic_permission_denied");
  }
  if (errStr.includes("stt-unavailable") || errStr.includes("not ready")) {
    return t("errors.stt_unavailable");
  }
  if (errStr.includes("model-download-failed")) {
    return t("errors.model_download_failed");
  }
  if (errStr.includes("cancelled")) {
    return t("errors.action_cancelled");
  }
  if (errStr.includes("denied")) {
    return t("errors.policy_denied");
  }

  return t(errStr, params);
}

export function translateDom(root = (typeof document !== "undefined" ? document : null)) {
  if (!root) return;

  const elements = root.querySelectorAll("[data-i18n]");
  for (const el of elements) {
    const key = el.getAttribute("data-i18n");
    if (key) {
      el.textContent = t(key);
    }
  }

  const placeholders = root.querySelectorAll("[data-i18n-placeholder]");
  for (const el of placeholders) {
    const key = el.getAttribute("data-i18n-placeholder");
    if (key) {
      el.placeholder = t(key);
    }
  }

  const titles = root.querySelectorAll("[data-i18n-title]");
  for (const el of titles) {
    const key = el.getAttribute("data-i18n-title");
    if (key) {
      el.title = t(key);
    }
  }
}
