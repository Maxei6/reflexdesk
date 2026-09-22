// extension/chrome/content.js
// Semantic DOM & Accessibility snapshot builder with tree compression & sensitive field masking

(function () {
  if (window.__reflexdesk_content_installed) {
    return;
  }
  window.__reflexdesk_content_installed = true;

  let lastMutationTime = 0;
  let isMutating = false;
  let mutationTimeout = null;

  // Track DOM mutations to flag active SPA re-renders
  const observer = new MutationObserver(() => {
    lastMutationTime = Date.now();
    clearTimeout(mutationTimeout);
    mutationTimeout = setTimeout(() => {
      isMutating = false;
    }, 250);
  });

  observer.observe(document.documentElement || document.body, {
    childList: true,
    subtree: true,
    attributes: true
  });

  const SENSITIVE_SELECTORS = [
    "input[type='password']",
    "input[autocomplete*='cc-']",
    "input[autocomplete*='password']",
    "input[name*='card']",
    "input[name*='cvv']",
    "input[name*='cvc']",
    "input[name*='password']",
    "input[name*='token']",
    "input[name*='secret']",
    "input[name*='ssn']"
  ];

  function isElementSensitive(el) {
    if (!el || el.nodeType !== Node.ELEMENT_NODE) return false;
    for (const sel of SENSITIVE_SELECTORS) {
      if (el.matches && el.matches(sel)) return true;
    }
    return false;
  }

  const SKIP_TAGS = new Set([
    "SCRIPT", "STYLE", "SVG", "PATH", "NOSCRIPT", "TEMPLATE", "LINK", "META",
    "HEAD", "CANVAS", "AUDIO", "VIDEO", "SOURCE", "TRACK", "IFRAME"
  ]);

  const INTERACTIVE_ROLES = new Set([
    "button", "link", "checkbox", "radio", "textbox", "searchbox", "combobox",
    "menuitem", "tab", "switch", "option", "slider"
  ]);

  let refCounter = 1;
  const refMap = new Map(); // refId -> Element

  function detectPageState() {
    const text = (document.body ? document.body.innerText : "").toLowerCase();
    const title = document.title.toLowerCase();

    const isCaptcha = text.includes("turnstile") ||
      text.includes("captcha") ||
      text.includes("verify you are human") ||
      text.includes("security check") ||
      !!document.querySelector(".cf-turnstile, #cf-challenge, iframe[src*='recaptcha'], iframe[src*='turnstile']");

    const isBlocked = title.includes("403 forbidden") ||
      title.includes("access denied") ||
      text.includes("access denied") ||
      text.includes("you have been blocked");

    const isAuthRequired = text.includes("sign in to continue") ||
      text.includes("login required") ||
      (document.querySelector("form[action*='login'], form[action*='signin']") !== null);

    return {
      isBlocked,
      isCaptcha,
      isAuthRequired,
      statusCode: null
    };
  }

  function buildSnapshot(maxDepth = 16) {
    refCounter = 1;
    refMap.clear();

    const elements = [];

    function traverse(node, depth) {
      if (!node || depth > maxDepth) return;
      if (node.nodeType !== Node.ELEMENT_NODE) return;

      const tagName = node.tagName.toUpperCase();
      if (SKIP_TAGS.has(tagName)) return;

      const rect = node.getBoundingClientRect();
      const isVisible = (rect.width > 0 && rect.height > 0 &&
        window.getComputedStyle(node).visibility !== "hidden" &&
        window.getComputedStyle(node).display !== "none");

      const role = node.getAttribute("role") || null;
      const ariaLabel = node.getAttribute("aria-label") || null;
      const isSensitive = isElementSensitive(node);

      const isInteractive = tagName === "A" || tagName === "BUTTON" || tagName === "INPUT" ||
        tagName === "TEXTAREA" || tagName === "SELECT" ||
        node.hasAttribute("onclick") || (role && INTERACTIVE_ROLES.has(role.toLowerCase()));

      let directText = null;
      if (node.childNodes.length === 1 && node.childNodes[0].nodeType === Node.TEXT_NODE) {
        const t = node.childNodes[0].nodeValue.trim();
        if (t.length > 0) directText = t.substring(0, 200);
      }

      // Semantic capture criteria: interactive, or has aria/name/text, or form control
      if (isVisible && (isInteractive || ariaLabel || role || directText || isSensitive)) {
        const ref = `b${refCounter++}`;
        refMap.set(ref, node);
        node.setAttribute("data-reflexdesk-ref", ref);

        let value = null;
        if ("value" in node) {
          value = isSensitive ? "[REDACTED]" : String(node.value || "").substring(0, 200);
        }

        elements.push({
          ref,
          tag: tagName.toLowerCase(),
          role,
          name: ariaLabel || node.getAttribute("name") || node.getAttribute("title") || null,
          text: isSensitive ? "[REDACTED]" : directText,
          value,
          placeholder: node.getAttribute("placeholder") || null,
          disabled: !!node.disabled,
          isSensitive,
          isInteractive,
          bounds: {
            x: Math.round(rect.x),
            y: Math.round(rect.y),
            width: Math.round(rect.width),
            height: Math.round(rect.height)
          }
        });
      }

      for (let i = 0; i < node.children.length; i++) {
        traverse(node.children[i], depth + 1);
      }
    }

    traverse(document.body || document.documentElement, 0);

    return {
      tabId: null, // Populated by background script
      url: window.location.href,
      title: document.title,
      elements,
      isMutating: isMutating || (Date.now() - lastMutationTime < 200),
      pageState: detectPageState()
    };
  }

  // Handle messages from background service worker
  chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
    const action = request.action;
    const args = request.args || {};

    switch (action) {
      case "browser.inspect": {
        const snapshot = buildSnapshot(args.maxDepth || 16);
        sendResponse({ success: true, data: snapshot });
        break;
      }

      case "browser.find": {
        const query = args.query;
        const by = args.by || "text";
        let matches = [];

        if (by === "selector") {
          try {
            const found = document.querySelectorAll(query);
            for (const el of found) {
              const ref = el.getAttribute("data-reflexdesk-ref");
              if (ref && refMap.has(ref)) {
                matches.push(ref);
              }
            }
          } catch (e) {
            sendResponse({ success: false, error: `Invalid selector: ${e.message}` });
            return;
          }
        } else {
          for (const [ref, el] of refMap.entries()) {
            const textContent = (el.innerText || el.textContent || "").toLowerCase();
            const aria = (el.getAttribute("aria-label") || "").toLowerCase();
            const q = query.toLowerCase();
            if (textContent.includes(q) || aria.includes(q)) {
              matches.push(ref);
            }
          }
        }
        sendResponse({ success: true, matches });
        break;
      }

      case "browser.click": {
        const ref = args.ref;
        const el = refMap.get(ref);
        if (!el || !document.contains(el)) {
          sendResponse({ success: false, error: "stale-ref", message: "Element no longer attached to DOM" });
          return;
        }

        el.scrollIntoView({ behavior: "instant", block: "center", inline: "center" });
        el.focus();

        const opts = {
          bubbles: true,
          cancelable: true,
          view: window,
          button: args.button === "right" ? 2 : (args.button === "middle" ? 1 : 0)
        };
        el.dispatchEvent(new MouseEvent("mousedown", opts));
        el.dispatchEvent(new MouseEvent("mouseup", opts));
        el.dispatchEvent(new MouseEvent("click", opts));

        sendResponse({ success: true, ref, navigated: false });
        break;
      }

      case "browser.type": {
        const ref = args.ref;
        const text = args.text || "";
        const el = refMap.get(ref);
        if (!el || !document.contains(el)) {
          sendResponse({ success: false, error: "stale-ref", message: "Element no longer attached to DOM" });
          return;
        }

        el.scrollIntoView({ behavior: "instant", block: "center" });
        el.focus();

        if (args.clear) {
          el.value = "";
          el.dispatchEvent(new Event("input", { bubbles: true }));
        }

        if ("value" in el) {
          el.value = (el.value || "") + text;
          el.dispatchEvent(new Event("input", { bubbles: true }));
          el.dispatchEvent(new Event("change", { bubbles: true }));
        }

        if (args.submit && el.form) {
          el.form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
        }

        sendResponse({ success: true, ref, valueLength: (el.value || "").length });
        break;
      }

      case "browser.select": {
        const ref = args.ref;
        const value = args.value;
        const el = refMap.get(ref);
        if (!el || !document.contains(el)) {
          sendResponse({ success: false, error: "stale-ref", message: "Element no longer attached to DOM" });
          return;
        }

        if (el.tagName.toUpperCase() === "SELECT") {
          el.value = value;
          el.dispatchEvent(new Event("change", { bubbles: true }));
          sendResponse({ success: true, ref, selectedValue: el.value });
        } else {
          sendResponse({ success: false, error: "invalid-target", message: "Target is not a SELECT element" });
        }
        break;
      }

      case "browser.scroll": {
        const direction = args.direction || "down";
        const amount = args.amount || 500;
        let dx = 0;
        let dy = 0;

        if (direction === "up") dy = -amount;
        else if (direction === "down") dy = amount;
        else if (direction === "top") window.scrollTo({ top: 0, behavior: "smooth" });
        else if (direction === "bottom") window.scrollTo({ top: document.body.scrollHeight, behavior: "smooth" });

        if (dy !== 0 || dx !== 0) {
          window.scrollBy({ left: dx, top: dy, behavior: "smooth" });
        }

        sendResponse({ success: true, scrollX: window.scrollX, scrollY: window.scrollY });
        break;
      }

      case "browser.extract": {
        const format = args.format || "text";
        let content = "";
        const el = args.ref ? refMap.get(args.ref) : (document.body || document.documentElement);

        if (!el) {
          sendResponse({ success: false, error: "element-not-found" });
          return;
        }

        if (format === "html") {
          content = el.innerHTML;
        } else {
          content = el.innerText || el.textContent || "";
        }

        sendResponse({ success: true, content, format });
        break;
      }

      case "browser.wait": {
        const selector = args.selector;
        const timeoutMs = args.timeoutMs || 5000;
        const startTime = Date.now();

        function check() {
          const el = document.querySelector(selector);
          if (el) {
            sendResponse({ success: true, matched: true, elapsedMs: Date.now() - startTime });
          } else if (Date.now() - startTime > timeoutMs) {
            sendResponse({ success: false, error: "action-timeout", matched: false, elapsedMs: timeoutMs });
          } else {
            setTimeout(check, 100);
          }
        }
        check();
        return true; // async sendResponse
      }

      case "browser.verify": {
        const kind = args.kind;
        const selector = args.selector;
        const expect = args.expect;
        let verified = false;
        let actual = null;

        if (kind === "element_present") {
          const el = selector ? document.querySelector(selector) : null;
          verified = el !== null;
          actual = verified;
        } else if (kind === "element_hidden") {
          const el = selector ? document.querySelector(selector) : null;
          verified = !el || el.offsetParent === null;
          actual = verified;
        } else if (kind === "text_contains") {
          const bodyText = (document.body ? document.body.innerText : "");
          const expectedStr = typeof expect === "string" ? expect : JSON.stringify(expect);
          verified = bodyText.toLowerCase().includes(expectedStr.toLowerCase());
          actual = verified;
        } else if (kind === "url_matches") {
          const expectedUrl = String(expect);
          verified = window.location.href.includes(expectedUrl);
          actual = window.location.href;
        } else if (kind === "title_is") {
          verified = document.title === String(expect);
          actual = document.title;
        }

        sendResponse({ success: true, verified, actual });
        break;
      }

      default:
        sendResponse({ success: false, error: `unknown-action: ${action}` });
    }
  });
})();
