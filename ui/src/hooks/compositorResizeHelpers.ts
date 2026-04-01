// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Pure drag / resize computation helpers for compositor layers.
 *
 * Extracted from compositorLayerParsers to keep that module focused on
 * parsing, serialisation, and merge logic.  All functions here are pure
 * (no React dependencies) and operate on LayerState values.
 */

import type { LayerKind } from './compositorConstants';
import type { LayerState, ResizeHandle } from './compositorLayerParsers';

// ── Constants ───────────────────────────────────────────────────────────────

/** Grid step used when snap-to-grid is active (pixels in canvas space). */
export const SNAP_GRID = 10;
/** Distance threshold for snapping to centre guidelines (pixels). */
export const SNAP_THRESHOLD = 8;

// ── Snap guide detection ────────────────────────────────────────────────────

/** Which snap guides are currently active during a drag. */
export interface SnapGuides {
  /** Horizontal centre line (layer centred vertically on canvas). */
  horizontalCenter: boolean;
  /** Vertical centre line (layer centred horizontally on canvas). */
  verticalCenter: boolean;
  /** Layer left edge aligned with canvas left edge (x ≈ 0). */
  leftEdge: boolean;
  /** Layer right edge aligned with canvas right edge (x + w ≈ canvasWidth). */
  rightEdge: boolean;
  /** Layer top edge aligned with canvas top edge (y ≈ 0). */
  topEdge: boolean;
  /** Layer bottom edge aligned with canvas bottom edge (y + h ≈ canvasHeight). */
  bottomEdge: boolean;
}

/** Determine which snap guides are active for a given layer position.
 *  Mirrors the snapping logic in `computeDragPosition` — a guide is shown
 *  whenever the layer snaps to a canvas centre axis or edge. */
export function detectSnapGuides(
  layer: LayerState,
  canvasWidth: number,
  canvasHeight: number
): SnapGuides {
  const midX = layer.x + layer.width / 2;
  const midY = layer.y + layer.height / 2;
  return {
    verticalCenter: Math.abs(midX - canvasWidth / 2) < 1,
    horizontalCenter: Math.abs(midY - canvasHeight / 2) < 1,
    leftEdge: Math.abs(layer.x) < 1,
    rightEdge: Math.abs(layer.x + layer.width - canvasWidth) < 1,
    topEdge: Math.abs(layer.y) < 1,
    bottomEdge: Math.abs(layer.y + layer.height - canvasHeight) < 1,
  };
}

// ── Drag / resize computation ───────────────────────────────────────────────

/** Compute the updated layer position/size from a drag or resize interaction.
 *  Pure function — no React dependencies. */
export function computeUpdatedLayer(
  orig: LayerState,
  type: 'drag' | 'resize',
  handle: ResizeHandle | undefined,
  rawDx: number,
  rawDy: number,
  canvasWidth: number,
  canvasHeight: number,
  layerKind?: LayerKind
): LayerState {
  if (type === 'drag') {
    return computeDragPosition(orig, rawDx, rawDy, canvasWidth, canvasHeight);
  }
  return computeResizePosition(orig, handle!, rawDx, rawDy, canvasWidth, canvasHeight, layerKind);
}

function computeDragPosition(
  orig: LayerState,
  rawDx: number,
  rawDy: number,
  canvasWidth: number,
  canvasHeight: number
): LayerState {
  let nx = orig.x + rawDx;
  let ny = orig.y + rawDy;

  // Only apply snapping when the pointer actually moved
  if (rawDx !== 0 || rawDy !== 0) {
    nx = Math.round(nx / SNAP_GRID) * SNAP_GRID;
    ny = Math.round(ny / SNAP_GRID) * SNAP_GRID;

    // Snap to canvas edges (layer edges → canvas boundaries)
    if (Math.abs(nx) < SNAP_THRESHOLD) {
      nx = 0;
    } else if (Math.abs(nx + orig.width - canvasWidth) < SNAP_THRESHOLD) {
      nx = canvasWidth - orig.width;
    }
    if (Math.abs(ny) < SNAP_THRESHOLD) {
      ny = 0;
    } else if (Math.abs(ny + orig.height - canvasHeight) < SNAP_THRESHOLD) {
      ny = canvasHeight - orig.height;
    }

    // Snap to canvas centre (layer midpoint → canvas midpoint)
    const midX = nx + orig.width / 2;
    const midY = ny + orig.height / 2;
    if (Math.abs(midX - canvasWidth / 2) < SNAP_THRESHOLD) {
      nx = (canvasWidth - orig.width) / 2;
    }
    if (Math.abs(midY - canvasHeight / 2) < SNAP_THRESHOLD) {
      ny = (canvasHeight - orig.height) / 2;
    }
  }

  return { ...orig, x: nx, y: ny };
}

// ── Resize internals ────────────────────────────────────────────────────────

/** Mutable position/size bag passed between resize phases. */
interface Dims {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Transform mouse delta into the layer's local coordinate system so
 *  resize handles behave naturally on rotated layers. */
function rotateToLocalCoords(
  rawDx: number,
  rawDy: number,
  rotationDegrees: number
): { dx: number; dy: number } {
  if (rotationDegrees === 0) return { dx: rawDx, dy: rawDy };
  const rad = (-rotationDegrees * Math.PI) / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  return {
    dx: rawDx * cos - rawDy * sin,
    dy: rawDx * sin + rawDy * cos,
  };
}

/** Compute raw edge deltas from handle direction + mouse movement, enforcing
 *  a minimum layer size of 20 px. */
function applyHandleDelta(orig: LayerState, handle: ResizeHandle, dx: number, dy: number): Dims {
  let newX = orig.x;
  let newY = orig.y;
  let newW = orig.width;
  let newH = orig.height;

  if (handle.includes('e')) newW = Math.max(20, orig.width + dx);
  if (handle.includes('w')) {
    newW = Math.max(20, orig.width - dx);
    newX = orig.x + (orig.width - newW);
  }
  if (handle.includes('s')) newH = Math.max(20, orig.height + dy);
  if (handle.includes('n')) {
    newH = Math.max(20, orig.height - dy);
    newY = orig.y + (orig.height - newH);
  }

  return { x: newX, y: newY, width: newW, height: newH };
}

function constrainAspectRatio(
  orig: LayerState,
  handle: ResizeHandle,
  rawW: number,
  rawH: number
): { width: number; height: number } {
  const ar = orig.width / orig.height;
  const isCorner = handle.length === 2;
  let newW = rawW;
  let newH = rawH;

  if (isCorner) {
    const dw = Math.abs(newW - orig.width);
    const dh = Math.abs(newH - orig.height);
    if (dw >= dh) {
      newH = newW / ar;
    } else {
      newW = newH * ar;
    }
  } else if (handle === 'e' || handle === 'w') {
    newH = newW / ar;
  } else {
    newW = newH * ar;
  }

  return {
    width: Math.max(20, newW),
    height: Math.max(20, newH),
  };
}

/** Snap a single horizontal edge (east or west) to the canvas boundary. */
function snapHorizontalEdge(
  pos: Dims,
  orig: LayerState,
  handle: ResizeHandle,
  canvasWidth: number,
  ar: number,
  deltaE: number,
  deltaW: number
): (Dims & { snapped: boolean }) | null {
  if (deltaE < SNAP_THRESHOLD) {
    const newX = pos.x;
    const newW = canvasWidth - newX;
    const newH = ar > 0 ? newW / ar : pos.height;
    const newY = handle.includes('n') ? orig.y + (orig.height - newH) : pos.y;
    return { x: newX, y: newY, width: newW, height: newH, snapped: true };
  }
  if (deltaW < SNAP_THRESHOLD) {
    const newX = 0;
    const newW = pos.width + pos.x;
    const newH = ar > 0 ? newW / ar : pos.height;
    const newY = handle.includes('n') ? orig.y + (orig.height - newH) : pos.y;
    return { x: newX, y: newY, width: newW, height: newH, snapped: true };
  }
  return null;
}

/** Snap a single vertical edge (south or north) to the canvas boundary. */
function snapVerticalEdge(
  pos: Dims,
  orig: LayerState,
  handle: ResizeHandle,
  canvasHeight: number,
  ar: number,
  deltaS: number,
  deltaN: number
): Dims | null {
  if (deltaS < SNAP_THRESHOLD) {
    const newY = pos.y;
    const newH = canvasHeight - newY;
    const newW = ar > 0 ? newH * ar : pos.width;
    const newX = handle.includes('w') ? orig.x + (orig.width - newW) : pos.x;
    return { x: newX, y: newY, width: newW, height: newH };
  }
  if (deltaN < SNAP_THRESHOLD) {
    let { x: newX, width: newW, height: newH } = pos;
    newH += pos.y;
    const newY = 0;
    if (ar > 0) newW = newH * ar;
    if (handle.includes('w')) newX = orig.x + (orig.width - newW);
    return { x: newX, y: newY, width: newW, height: newH };
  }
  return null;
}

/** For corner handles, determine which axis to skip snapping on.
 *  When both axes are within the snap threshold, only snap the closer one
 *  to avoid the AR correction of one undoing the other's alignment. */
function cornerSkipAxes(
  handle: ResizeHandle,
  deltaE: number,
  deltaW: number,
  deltaS: number,
  deltaN: number
): { skipH: boolean; skipV: boolean } {
  const isCorner =
    (handle.includes('n') || handle.includes('s')) &&
    (handle.includes('e') || handle.includes('w'));
  if (!isCorner) return { skipH: false, skipV: false };

  const bestH = Math.min(deltaE, deltaW);
  const bestV = Math.min(deltaN, deltaS);
  const bothClose = bestH < SNAP_THRESHOLD && bestV < SNAP_THRESHOLD;
  return {
    skipH: bothClose && bestH > bestV,
    skipV: bothClose && bestV > bestH,
  };
}

/** Snap resize edges to canvas boundaries, re-applying the aspect ratio
 *  constraint after each snap so the layer isn't distorted.
 *  For corner handles, only snap the axis closest to a boundary to avoid
 *  the second snap's AR correction undoing the first snap's alignment. */
function snapResizeToEdges(
  pos: Dims,
  orig: LayerState,
  handle: ResizeHandle,
  canvasWidth: number,
  canvasHeight: number,
  ar: number
): Dims & { snappedH: boolean } {
  const deltaE = handle.includes('e') ? Math.abs(pos.x + pos.width - canvasWidth) : Infinity;
  const deltaW = handle.includes('w') ? Math.abs(pos.x) : Infinity;
  const deltaS = handle.includes('s') ? Math.abs(pos.y + pos.height - canvasHeight) : Infinity;
  const deltaN = handle.includes('n') ? Math.abs(pos.y) : Infinity;

  const { skipH, skipV } = cornerSkipAxes(handle, deltaE, deltaW, deltaS, deltaN);

  let snappedH = false;
  let result = pos;

  if (!skipH) {
    const hSnap = snapHorizontalEdge(pos, orig, handle, canvasWidth, ar, deltaE, deltaW);
    if (hSnap) {
      snappedH = hSnap.snapped;
      result = hSnap;
    }
  }

  if (!skipV && !snappedH) {
    const vSnap = snapVerticalEdge(result, orig, handle, canvasHeight, ar, deltaS, deltaN);
    if (vSnap) result = vSnap;
  }

  return { ...result, snappedH };
}

/** Clamp so layers cannot extend past the canvas edges.
 *  For AR-constrained layers, re-derive the complementary dimension after
 *  each clamp so the aspect ratio is preserved. */
function clampResizeToBounds(
  pos: Dims,
  orig: LayerState,
  handle: ResizeHandle,
  canvasWidth: number,
  canvasHeight: number,
  ar: number
): Dims {
  let { x: newX, y: newY, width: newW, height: newH } = pos;

  const maxW = handle.includes('w')
    ? orig.x + orig.width // west handle: right edge is anchored at orig.x + orig.width
    : canvasWidth - newX; // east handle: left edge is anchored at newX
  const maxH = handle.includes('n')
    ? orig.y + orig.height // north handle: bottom edge is anchored
    : canvasHeight - newY; // south handle: top edge is anchored

  if (newW > maxW) {
    newW = maxW;
    if (ar > 0) newH = newW / ar;
  }
  if (newH > maxH) {
    newH = maxH;
    if (ar > 0) {
      newW = newH * ar;
      // Guard: the height-clamp AR correction may have pushed width back
      // above maxW (possible when the layer sits near both the right and
      // bottom edges with an extreme AR).  Re-clamp to keep it in bounds.
      newW = Math.min(newW, maxW);
    }
  }
  // Re-anchor position for handles that move the origin edge.
  if (handle.includes('w')) newX = orig.x + (orig.width - newW);
  if (handle.includes('n')) newY = orig.y + (orig.height - newH);

  return { x: newX, y: newY, width: newW, height: newH };
}

// ── Main resize entry point ─────────────────────────────────────────────────

function computeResizePosition(
  orig: LayerState,
  handle: ResizeHandle,
  rawDx: number,
  rawDy: number,
  canvasWidth: number,
  canvasHeight: number,
  layerKind?: LayerKind
): LayerState {
  // Phase 1: Transform mouse delta into local coordinates.
  const { dx, dy } = rotateToLocalCoords(rawDx, rawDy, orig.rotationDegrees);

  // Phase 2: Apply raw edge deltas.
  let pos = applyHandleDelta(orig, handle, dx, dy);

  // Phase 3: Constrain aspect ratio (video + image layers only).
  // Text overlays resize freely — their dimensions are auto-measured from
  // text content and don't carry a meaningful aspect ratio.  Enforcing AR
  // on text causes small cursor movements to produce large dimension jumps
  // (e.g. a 300×36 single-line overlay has an 8:1 ratio).
  const enforceAR = layerKind !== 'text';
  if (enforceAR && orig.width > 0 && orig.height > 0) {
    const result = constrainAspectRatio(orig, handle, pos.width, pos.height);
    pos.width = result.width;
    pos.height = result.height;

    if (handle.includes('w')) pos.x = orig.x + (orig.width - pos.width);
    if (handle.includes('n')) pos.y = orig.y + (orig.height - pos.height);
  }

  // Phase 4: Snap edges to canvas boundaries.
  // Text overlays skip AR correction (ar=0) so edge snaps don't cascade.
  const ar = enforceAR && orig.width > 0 && orig.height > 0 ? orig.width / orig.height : 0;
  pos = snapResizeToEdges(pos, orig, handle, canvasWidth, canvasHeight, ar);

  // Phase 5: Clamp to canvas bounds.
  pos = clampResizeToBounds(pos, orig, handle, canvasWidth, canvasHeight, ar);

  return { ...orig, x: pos.x, y: pos.y, width: pos.width, height: pos.height };
}
