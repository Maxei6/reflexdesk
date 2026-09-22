/**
 * Calculates the physical pixel position for the overlay window.
 *
 * Handles:
 * - 100%, 125%, 150%, 200% DPI scales (scaleFactor)
 * - Multi-monitor setups with negative coordinates (e.g. monitor placed left or above primary)
 * - Work area insets (taskbar/dock at bottom, top, left, or right)
 * - Monitor disconnect / removal fallback (when monitor is null, undefined, or empty)
 * - Boundary clamping so overlay never overflows the target work area
 *
 * @param {object|null} monitor Tauri monitor object { position, size, workArea?, scaleFactor }
 * @param {object} windowSize { width: number, height: number } physical pixels
 * @param {object} [options]
 * @param {number} [options.baseGapDp=56] Gap in logical DP from bottom of work area
 * @returns {{ x: number, y: number }} Physical coordinates (x, y)
 */
export function calculateOverlayPosition(monitor, windowSize, options = {}) {
  const winWidth = Math.max(1, Math.round(Number(windowSize && windowSize.width) || 180));
  const winHeight = Math.max(1, Math.round(Number(windowSize && windowSize.height) || 160));

  if (!monitor || typeof monitor !== "object") {
    // Monitor removal or fallback: center in a standard 1920x1080 bounds
    const baseGap = typeof options.baseGapDp === "number" ? options.baseGapDp : 56;
    return {
      x: Math.round((1920 - winWidth) / 2),
      y: Math.max(0, 1080 - winHeight - Math.round(baseGap * 1)),
    };
  }

  const scale = Number(monitor.scaleFactor) > 0 ? Number(monitor.scaleFactor) : 1;
  const baseGap = typeof options.baseGapDp === "number" ? options.baseGapDp : 56;
  const gap = Math.round(baseGap * scale);

  // Use workArea if present (accounting for taskbar/dock insets), otherwise position & size
  const workPos = (monitor.workArea && monitor.workArea.position) || monitor.position || { x: 0, y: 0 };
  const workSize = (monitor.workArea && monitor.workArea.size) || monitor.size || { width: 1920, height: 1080 };

  const areaX = Number(workPos.x) || 0;
  const areaY = Number(workPos.y) || 0;
  const areaWidth = Math.max(winWidth, Number(workSize.width) || 1920);
  const areaHeight = Math.max(winHeight, Number(workSize.height) || 1080);

  // Horizontal centering within the work area
  const rawX = areaX + Math.round((areaWidth - winWidth) / 2);
  const minX = areaX;
  const maxX = areaX + Math.max(0, areaWidth - winWidth);
  const clampedX = Math.max(minX, Math.min(maxX, rawX));

  // Vertical placement: above bottom edge of work area by gap
  const rawY = areaY + areaHeight - winHeight - gap;
  const minY = areaY;
  const maxY = areaY + Math.max(0, areaHeight - winHeight);
  const clampedY = Math.max(minY, Math.min(maxY, rawY));

  return { x: clampedX, y: clampedY };
}
