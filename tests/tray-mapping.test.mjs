import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { calculateOverlayPosition } from "../src/lib/geometry.js";

test("calculateOverlayPosition centers horizontally on 100% scale single monitor", () => {
  const monitor = {
    position: { x: 0, y: 0 },
    size: { width: 1920, height: 1080 },
    workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1040 } }, // 40px taskbar at bottom
    scaleFactor: 1.0,
  };
  const windowSize = { width: 200, height: 150 };

  const pos = calculateOverlayPosition(monitor, windowSize, { baseGapDp: 56 });
  // Horizontal center: 0 + (1920 - 200) / 2 = 860
  assert.equal(pos.x, 860);
  // Vertical: 0 + 1040 - 150 - 56 = 834
  assert.equal(pos.y, 834);
});

test("calculateOverlayPosition scales gap accurately for 125%, 150%, and 200% DPI", () => {
  const windowSize = { width: 200, height: 150 };

  // 125% scale: gap = 56 * 1.25 = 70
  const m125 = {
    position: { x: 0, y: 0 },
    size: { width: 2400, height: 1350 },
    workArea: { position: { x: 0, y: 0 }, size: { width: 2400, height: 1300 } },
    scaleFactor: 1.25,
  };
  const pos125 = calculateOverlayPosition(m125, windowSize, { baseGapDp: 56 });
  assert.equal(pos125.x, Math.round((2400 - 200) / 2));
  assert.equal(pos125.y, 1300 - 150 - 70);

  // 150% scale: gap = 56 * 1.5 = 84
  const m150 = {
    position: { x: 0, y: 0 },
    size: { width: 2880, height: 1620 },
    workArea: { position: { x: 0, y: 0 }, size: { width: 2880, height: 1550 } },
    scaleFactor: 1.5,
  };
  const pos150 = calculateOverlayPosition(m150, windowSize, { baseGapDp: 56 });
  assert.equal(pos150.x, Math.round((2880 - 200) / 2));
  assert.equal(pos150.y, 1550 - 150 - 84);

  // 200% scale: gap = 56 * 2 = 112
  const m200 = {
    position: { x: 0, y: 0 },
    size: { width: 3840, height: 2160 },
    workArea: { position: { x: 0, y: 0 }, size: { width: 3840, height: 2060 } },
    scaleFactor: 2.0,
  };
  const pos200 = calculateOverlayPosition(m200, windowSize, { baseGapDp: 56 });
  assert.equal(pos200.x, Math.round((3840 - 200) / 2));
  assert.equal(pos200.y, 2060 - 150 - 112);
});

test("calculateOverlayPosition handles negative monitor coordinates (left and top placement)", () => {
  const windowSize = { width: 200, height: 160 };

  // Secondary monitor to the left: x = -1920
  const mLeft = {
    position: { x: -1920, y: 0 },
    size: { width: 1920, height: 1080 },
    workArea: { position: { x: -1920, y: 0 }, size: { width: 1920, height: 1040 } },
    scaleFactor: 1.0,
  };
  const posLeft = calculateOverlayPosition(mLeft, windowSize, { baseGapDp: 56 });
  // Centered between -1920 and 0: -1920 + (1920 - 200) / 2 = -1060
  assert.equal(posLeft.x, -1060);
  assert.equal(posLeft.y, 1040 - 160 - 56);

  // Secondary monitor above: y = -1080
  const mTop = {
    position: { x: 0, y: -1080 },
    size: { width: 1920, height: 1080 },
    workArea: { position: { x: 0, y: -1080 }, size: { width: 1920, height: 1080 } },
    scaleFactor: 1.0,
  };
  const posTop = calculateOverlayPosition(mTop, windowSize, { baseGapDp: 56 });
  assert.equal(posTop.x, 860);
  assert.equal(posTop.y, -1080 + 1080 - 160 - 56); // -216
});

test("calculateOverlayPosition handles various taskbar / dock insets", () => {
  const windowSize = { width: 200, height: 150 };

  // Taskbar on top (work area starts at y = 50)
  const mTopDock = {
    position: { x: 0, y: 0 },
    size: { width: 1920, height: 1080 },
    workArea: { position: { x: 0, y: 50 }, size: { width: 1920, height: 1030 } },
    scaleFactor: 1.0,
  };
  const posTopDock = calculateOverlayPosition(mTopDock, windowSize, { baseGapDp: 50 });
  assert.equal(posTopDock.x, 860);
  assert.equal(posTopDock.y, 50 + 1030 - 150 - 50);

  // Taskbar on left (work area starts at x = 70)
  const mLeftDock = {
    position: { x: 0, y: 0 },
    size: { width: 1920, height: 1080 },
    workArea: { position: { x: 70, y: 0 }, size: { width: 1850, height: 1080 } },
    scaleFactor: 1.0,
  };
  const posLeftDock = calculateOverlayPosition(mLeftDock, windowSize, { baseGapDp: 50 });
  assert.equal(posLeftDock.x, 70 + Math.round((1850 - 200) / 2));
  assert.equal(posLeftDock.y, 1080 - 150 - 50);
});

test("calculateOverlayPosition gracefully handles monitor removal or null monitor", () => {
  const windowSize = { width: 200, height: 150 };

  const posNull = calculateOverlayPosition(null, windowSize);
  assert.equal(typeof posNull.x, "number");
  assert.equal(typeof posNull.y, "number");
  assert.equal(posNull.x, Math.round((1920 - 200) / 2));
  assert.equal(posNull.y, 1080 - 150 - 56);

  const posUndef = calculateOverlayPosition(undefined, windowSize);
  assert.equal(posUndef.x, Math.round((1920 - 200) / 2));
});

test("calculateOverlayPosition clamps overlay within bounds when work area is small", () => {
  const windowSize = { width: 500, height: 400 };
  const mSmall = {
    position: { x: 100, y: 100 },
    size: { width: 600, height: 450 },
    workArea: { position: { x: 100, y: 100 }, size: { width: 600, height: 450 } },
    scaleFactor: 1.0,
  };

  const pos = calculateOverlayPosition(mSmall, windowSize, { baseGapDp: 100 });
  // rawY would be 100 + 450 - 400 - 100 = 50, which is less than minY (100)
  // Clamp ensures it stays >= minY (100)
  assert.ok(pos.y >= 100);
  assert.ok(pos.x >= 100);
  assert.ok(pos.x + windowSize.width <= 100 + 600);
});

test("all required tray assets exist and are valid PNGs", () => {
  const states = ["idle", "ready", "listening", "working", "attention"];
  const sizes = [16, 20, 24];

  for (const st of states) {
    for (const sz of sizes) {
      // Standard colored
      const coloredPath = path.resolve(process.cwd(), `assets/tray/${st}-${sz}.png`);
      assert.ok(fs.existsSync(coloredPath), `Missing asset: ${coloredPath}`);
      const buf = fs.readFileSync(coloredPath);
      // Valid PNG magic bytes: 0x89 0x50 0x4E 0x47 0x0D 0x0A 0x1A 0x0A
      assert.equal(buf[0], 0x89);
      assert.equal(buf[1], 0x50);
      assert.equal(buf[2], 0x4e);
      assert.equal(buf[3], 0x47);
      // IHDR width and height
      const width = buf.readUInt32BE(16);
      const height = buf.readUInt32BE(20);
      assert.equal(width, sz);
      assert.equal(height, sz);

      // macOS monochrome template
      const tmplPath = path.resolve(process.cwd(), `assets/tray/${st}-template-${sz}.png`);
      assert.ok(fs.existsSync(tmplPath), `Missing template asset: ${tmplPath}`);
      const tmplBuf = fs.readFileSync(tmplPath);
      assert.equal(tmplBuf[0], 0x89);
      assert.equal(tmplBuf.readUInt32BE(16), sz);
      assert.equal(tmplBuf.readUInt32BE(20), sz);
    }

    // Default aliases
    const aliasPath = path.resolve(process.cwd(), `assets/tray/${st}.png`);
    assert.ok(fs.existsSync(aliasPath), `Missing alias: ${aliasPath}`);
    const tmplAliasPath = path.resolve(process.cwd(), `assets/tray/${st}-template.png`);
    assert.ok(fs.existsSync(tmplAliasPath), `Missing template alias: ${tmplAliasPath}`);
  }
});
