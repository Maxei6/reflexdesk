const APP_ALIASES = new Map([
  ["spotify", "spotify"],
  ["chrome", "chrome"],
  ["google chrome", "chrome"],
  ["firefox", "firefox"],
  ["vscode", "vscode"],
  ["vs code", "vscode"],
  ["visual studio code", "vscode"],
  ["terminal", "terminal"],
  ["notepad", "notepad"],
]);

export function routeFast(text) {
  const raw = String(text ?? "").trim();
  const lower = raw.toLowerCase();
  if (!lower) return { kind: "noop", confidence: 1 };

  if (/^(hello|hey)\s+reflex(?:desk)?[.!?]*$/.test(lower)) {
    return { kind: "control", action: "reflex.ping", args: {}, confidence: 0.99 };
  }

  if (/^(stop|stop listening|go to sleep|sleep)$/.test(lower)) {
    return { kind: "control", action: "voice.stop", args: {}, confidence: 0.99 };
  }

  const search = lower.match(/^(?:search(?: the web)? for|google)\s+(.+)$/i);
  if (search) {
    return { kind: "tool", action: "browser.search", args: { query: raw.slice(raw.toLowerCase().indexOf(search[1])) }, confidence: 0.98 };
  }

  const urlMatch = raw.match(/^(?:open|go to)\s+(https?:\/\/\S+)$/i);
  if (urlMatch) {
    return { kind: "tool", action: "browser.open", args: { url: urlMatch[1] }, confidence: 0.99 };
  }

  const open = lower.match(/^(?:open|launch|start)\s+(.+)$/i);
  if (open) {
    const target = open[1].replace(/[.!?]+$/, "").trim();
    const app = APP_ALIASES.get(target);
    if (app) return { kind: "tool", action: "app.open", args: { app }, confidence: 0.97 };
    return { kind: "planner", text: raw, confidence: 0.35 };
  }

  const harness = lower.match(/^(?:ask|tell|run)\s+(opencode|kilo|codex|claude(?: code)?)\s+(?:to\s+)?(.+)$/i);
  if (harness) {
    return {
      kind: "tool",
      action: "harness.start",
      args: { harness: harness[1].replace("claude code", "claude"), prompt: harness[2] },
      confidence: 0.9,
    };
  }

  return { kind: "planner", text: raw, confidence: 0.2 };
}
