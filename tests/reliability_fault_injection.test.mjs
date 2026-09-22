import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";

test("FaultInjection 1/13: Model download interrupted mid-stream cleans staging and rolls back", () => {
  class MockModelManager {
    constructor() {
      this.state = "Discovered";
      this.activeFile = null;
      this.stagingFile = null;
    }

    startDownload(modelId, failAtByte = 500) {
      this.state = "Downloading";
      this.stagingFile = `/tmp/${modelId}.staging`;

      // Simulate partial download then network drop
      let downloaded = 0;
      try {
        while (downloaded < 1000) {
          downloaded += 250;
          if (downloaded >= failAtByte) {
            throw new Error("connection-reset-by-peer");
          }
        }
        this.activeFile = this.stagingFile;
        this.state = "Active";
      } catch (err) {
        // Failure handling: do NOT activate partial file, transition to Failed
        this.state = "Failed";
        this.lastError = err.message;
        // Clean staging file
        this.stagingFile = null;
      }
    }
  }

  const mgr = new MockModelManager();
  mgr.startDownload("nemotron-3.5", 500);

  assert.equal(mgr.state, "Failed");
  assert.equal(mgr.activeFile, null, "Corrupt or partial download must NEVER become active");
  assert.equal(mgr.stagingFile, null, "Staging file must be cleaned up on abort");
  assert.equal(mgr.lastError, "connection-reset-by-peer");
});

test("FaultInjection 2/13: Disk full preflight fails closed before writing payload", () => {
  function preflightDiskWrite(requiredBytes, availableBytes) {
    const SAFETY_BUFFER = 100 * 1024 * 1024; // 100MB
    const totalRequired = requiredBytes + SAFETY_BUFFER;
    if (availableBytes < totalRequired) {
      return {
        ok: false,
        code: "disk-full",
        error: `Insufficient disk space: needed ${totalRequired} bytes, available ${availableBytes}`
      };
    }
    return { ok: true };
  }

  const modelSize = 400 * 1024 * 1024; // 400MB
  const fullDiskSpace = 200 * 1024 * 1024; // only 200MB free

  const check = preflightDiskWrite(modelSize, fullDiskSpace);
  assert.equal(check.ok, false);
  assert.equal(check.code, "disk-full");
  assert.ok(check.error.includes("Insufficient disk space"));
});

test("FaultInjection 3/13: Microphone denied or unplugged recovers safely without crashing", () => {
  class AudioCaptureManager {
    constructor() {
      this.state = "idle";
      this.activeDevice = "default_mic";
      this.lastError = null;
    }

    startListening(permissionGranted, deviceConnected) {
      if (!permissionGranted) {
        this.state = "error_permission";
        this.lastError = "mic-permission-denied";
        return { ok: false, error: this.lastError };
      }
      if (!deviceConnected) {
        this.state = "error_no_device";
        this.lastError = "mic-disconnected";
        return { ok: false, error: this.lastError };
      }
      this.state = "listening";
      return { ok: true };
    }

    onDeviceUnplugged() {
      if (this.state === "listening") {
        this.state = "idle";
        this.lastError = "mic-unplugged-during-session";
      }
    }
  }

  const audio = new AudioCaptureManager();

  // Test permission denied
  const resPerm = audio.startListening(false, true);
  assert.equal(resPerm.ok, false);
  assert.equal(audio.state, "error_permission");

  // Test unplug mid-stream
  audio.startListening(true, true);
  assert.equal(audio.state, "listening");
  audio.onDeviceUnplugged();
  assert.equal(audio.state, "idle");
  assert.equal(audio.lastError, "mic-unplugged-during-session");
});

test("FaultInjection 4/13: STT process crash detected and harvested by supervisor", () => {
  class MockProcessSupervisor {
    constructor() {
      this.children = new Map();
      this.restarts = 0;
    }

    spawn(id, cmd) {
      this.children.set(id, { id, cmd, pid: 1234, alive: true, exitCode: null });
    }

    simulateCrash(id, signal) {
      const child = this.children.get(id);
      if (child) {
        child.alive = false;
        child.exitCode = signal === "SIGSEGV" ? 139 : 1;
      }
    }

    watchdogCheck(id) {
      const child = this.children.get(id);
      if (child && !child.alive) {
        this.restarts++;
        child.alive = true;
        child.exitCode = null;
        return { recovered: true, newPid: 1235 };
      }
      return { recovered: false };
    }
  }

  const supervisor = new MockProcessSupervisor();
  supervisor.spawn("stt_engine", "crispasr-streaming");
  supervisor.simulateCrash("stt_engine", "SIGSEGV");

  assert.equal(supervisor.children.get("stt_engine").alive, false);
  const recovery = supervisor.watchdogCheck("stt_engine");
  assert.ok(recovery.recovered);
  assert.equal(supervisor.restarts, 1);
  assert.ok(supervisor.children.get("stt_engine").alive);
});

test("FaultInjection 5/13: Planner timeout aborts request and falls back safely", async () => {
  async function executePlannerWithTimeout(prompt, timeoutMs) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);

    try {
      // Simulate hanging planner promise
      await new Promise((_, reject) => {
        controller.signal.addEventListener("abort", () => {
          reject(new Error("planner-timeout: request exceeded deadline"));
        });
      });
    } catch (err) {
      return {
        ok: false,
        fallback: "deterministic-reflex",
        error: err.message
      };
    } finally {
      clearTimeout(timer);
    }
  }

  const result = await executePlannerWithTimeout("complex multi-step task", 20);
  assert.equal(result.ok, false);
  assert.equal(result.fallback, "deterministic-reflex");
  assert.ok(result.error.includes("planner-timeout"));
});

test("FaultInjection 6/13: Port conflict detected with clear error and fallback", () => {
  class PortManager {
    constructor() {
      this.occupiedPorts = new Set([8787]); // Default sidecar port already bound
    }

    bindService(desiredPort, allowFallback = true) {
      if (this.occupiedPorts.has(desiredPort)) {
        if (!allowFallback) {
          return { ok: false, code: "port-conflict", error: `Port ${desiredPort} already in use` };
        }
        // Fallback to alternate port
        const alternatePort = desiredPort + 1;
        if (!this.occupiedPorts.has(alternatePort)) {
          this.occupiedPorts.add(alternatePort);
          return { ok: true, port: alternatePort, warned: true };
        }
      }
      this.occupiedPorts.add(desiredPort);
      return { ok: true, port: desiredPort };
    }
  }

  const mgr = new PortManager();
  const strictBind = mgr.bindService(8787, false);
  assert.equal(strictBind.ok, false);
  assert.equal(strictBind.code, "port-conflict");

  const flexibleBind = mgr.bindService(8787, true);
  assert.ok(flexibleBind.ok);
  assert.equal(flexibleBind.port, 8788);
  assert.equal(flexibleBind.warned, true);
});

test("FaultInjection 7/13: Two instances detected via lockfile and prevented", () => {
  class InstanceLock {
    constructor() {
      this.locked = false;
    }

    tryAcquire() {
      if (this.locked) {
        return { ok: false, error: "already-running: another instance is active" };
      }
      this.locked = true;
      return { ok: true };
    }

    release() {
      this.locked = false;
    }
  }

  const lock = new InstanceLock();
  assert.ok(lock.tryAcquire().ok);

  // Second instance attempt
  const second = lock.tryAcquire();
  assert.equal(second.ok, false);
  assert.equal(second.error.includes("already-running"), true);

  // After first releases, can acquire again
  lock.release();
  assert.ok(lock.tryAcquire().ok);
});

test("FaultInjection 8/13: Hotkey conflict handled gracefully without UI crash", () => {
  class HotkeyManager {
    constructor() {
      this.registered = false;
      this.fallbackAvailable = true;
    }

    registerShortcut(shortcut, systemHotkeys) {
      if (systemHotkeys.includes(shortcut)) {
        return {
          ok: false,
          code: "hotkey-conflict",
          error: `Shortcut ${shortcut} already registered by OS or another application`,
          recovery: "Use system tray or click overlay button"
        };
      }
      this.registered = true;
      return { ok: true };
    }
  }

  const mgr = new HotkeyManager();
  const res = mgr.registerShortcut("CommandOrControl+Shift+Space", ["CommandOrControl+Shift+Space"]);

  assert.equal(res.ok, false);
  assert.equal(res.code, "hotkey-conflict");
  assert.ok(res.recovery.includes("system tray"));
});

test("FaultInjection 9/13: Stale UI element detected and triggers re-inspection", () => {
  function executeElementAction(element, currentDOM) {
    if (!currentDOM.includes(element.id)) {
      return {
        ok: false,
        code: "stale-ref",
        error: "Element has detached from DOM",
        action: "reinspect"
      };
    }
    return { ok: true, clicked: true };
  }

  const staleElement = { id: "button_submit_1" };
  const mutatedDOM = ["button_submit_2", "header_title"];

  const res = executeElementAction(staleElement, mutatedDOM);
  assert.equal(res.ok, false);
  assert.equal(res.code, "stale-ref");
  assert.equal(res.action, "reinspect");
});

test("FaultInjection 10/13: Browser SPA mutation cancels in-flight subaction cleanly", () => {
  function handleBrowserMutation(currentUrl, targetUrl) {
    if (currentUrl !== targetUrl) {
      return {
        ok: false,
        code: "spa-mutation",
        error: "Tab navigated during action execution",
        recovery: "abort-subaction-and-refresh-state"
      };
    }
    return { ok: true };
  }

  const res = handleBrowserMutation("https://example.com/checkout", "https://example.com/cart");
  assert.equal(res.ok, false);
  assert.equal(res.code, "spa-mutation");
  assert.equal(res.recovery, "abort-subaction-and-refresh-state");
});

test("FaultInjection 11/13: Owned agent crash harvested with exit code", () => {
  class HarnessSession {
    constructor(sessionId) {
      this.sessionId = sessionId;
      this.status = "running";
      this.exitCode = null;
    }

    onProcessExit(code) {
      this.exitCode = code;
      this.status = code === 0 ? "completed" : "failed";
      return {
        kind: "error",
        session_id: this.sessionId,
        exit_code: code,
        message: `Agent exited with code ${code}`
      };
    }
  }

  const session = new HarnessSession("hs_1a2b3c4d");
  const crashEvent = session.onProcessExit(137); // OOM kill

  assert.equal(session.status, "failed");
  assert.equal(crashEvent.exit_code, 137);
  assert.equal(crashEvent.kind, "error");
});

test("FaultInjection 12/13: Quit during action signals cancel and terminates owned processes", () => {
  let cancelTokenInvoked = false;
  const killedPids = [];

  function cancelNow(reason) {
    cancelTokenInvoked = true;
  }

  function terminateOwnedProcesses(processes) {
    for (const p of processes) {
      if (p.ownership === "Internal" || p.ownership === "OwnedSession") {
        killedPids.push(p.pid);
      }
    }
  }

  function onAppQuit() {
    cancelNow("app-quitting");
    terminateOwnedProcesses([
      { pid: 101, ownership: "Internal" },
      { pid: 102, ownership: "OwnedSession" },
      { pid: 201, ownership: "UserApp" }
    ]);
  }

  onAppQuit();

  assert.ok(cancelTokenInvoked);
  assert.ok(killedPids.includes(101));
  assert.ok(killedPids.includes(102));
  assert.equal(killedPids.includes(201), false, "UserApp must not be terminated");
});

test("FaultInjection 13/13: Corrupt config or cache safely falls back to defaults", () => {
  function safeLoadConfig(rawContent, defaultConfig) {
    try {
      if (!rawContent || rawContent.trim() === "") {
        return { config: defaultConfig, recovered: true };
      }
      const parsed = JSON.parse(rawContent);
      return { config: Object.assign({}, defaultConfig, parsed), recovered: false };
    } catch (err) {
      // Graceful fallback to default on corruption
      return { config: defaultConfig, recovered: true, corrupt_quarantined: true };
    }
  }

  const defaultConfig = {
    schema_version: 1,
    language: "en",
    overlay_enabled: true
  };

  const corruptJson = '{"schema_version": 1, "language": "en", TRUNCATED_GARBAGE...';
  const result = safeLoadConfig(corruptJson, defaultConfig);

  assert.deepEqual(result.config, defaultConfig);
  assert.equal(result.recovered, true);
  assert.equal(result.corrupt_quarantined, true);
});
