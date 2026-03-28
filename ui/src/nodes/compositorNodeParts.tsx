// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Styled components, sub-components, and helpers for CompositorNode.
 *
 * Extracted to keep the main CompositorNode module under the max-lines
 * lint threshold while preserving identical runtime behaviour.
 */

import styled from '@emotion/styled';

import type { LayerKind } from '@/hooks/useCompositorLayers';

// ── Friendly label helpers ──────────────────────────────────────────────────

/** Convert technical layer IDs to user-friendly labels.
 *  e.g. "in_0" -> "Input 0", "text_1" -> "Text 1" */
export function friendlyLabel(id: string, kind: LayerKind, index?: number): string {
  switch (kind) {
    case 'video': {
      const match = id.match(/(\d+)$/);
      return `Input ${match ? match[1] : (index ?? 0)}`;
    }
    case 'text':
      return `Text ${index ?? 0}`;
    case 'image':
      return `Image ${index ?? 0}`;
    default:
      return id;
  }
}

// ── Styled components ───────────────────────────────────────────────────────

/** Outer wrapper that positions the node body and the side inspector panel */
export const CompositorOuterWrapper = styled.div`
  position: relative;
  display: flex;
  align-items: flex-start;

  &:focus {
    outline: none;
  }
`;

export const CompositorWrapper = styled.div`
  border-top: 1px solid var(--sk-border);
  padding: 10px 8px 6px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
  min-width: 0;
`;

export const CanvasSection = styled.div`
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

export const CanvasHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
`;

export const CanvasLabel = styled.span`
  color: var(--sk-text-muted);
`;

export const ResolutionLabel = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  font-size: 10px;
`;

export const ControlRow = styled.div`
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
`;

export const ControlLabel = styled.span`
  color: var(--sk-text-muted);
  min-width: 52px;
  flex-shrink: 0;
`;

export const ControlValue = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text);
  min-width: 36px;
  text-align: right;
  flex-shrink: 0;
`;

const NumericInput = styled.input`
  width: 44px;
  padding: 3px 4px;
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  text-align: center;
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  background: var(--sk-input-bg);
  color: var(--sk-text);
  outline: none;
  pointer-events: auto;
  transition: border-color 0.15s;

  &:focus {
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 1px var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  &:read-only {
    cursor: default;
    opacity: 0.5;
    border-color: transparent;
    background: transparent;

    &:focus {
      border-color: transparent;
      box-shadow: none;
    }
  }

  /* Hide spinners */
  &::-webkit-inner-spin-button,
  &::-webkit-outer-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  -moz-appearance: textfield;
`;

export const LayerInfoRow = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  padding: 2px 0;
`;

export const NoSelectionText = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  text-align: center;
  padding: 4px 0;
`;

// ── Overlay management styled components ────────────────────────────────────

export const AddOverlayButton = styled.button`
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 6px;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-input-bg);
  color: var(--sk-text-muted);
  cursor: pointer;
  font-size: 10px;
  pointer-events: auto;

  &:hover:not(:disabled) {
    background: var(--sk-overlay-medium);
    color: var(--sk-text);
    border-color: var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }
`;

export const AddMenu = styled.div`
  position: absolute;
  right: 0;
  top: 100%;
  margin-top: 2px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  z-index: 10;
  min-width: 120px;
  overflow: hidden;
`;

export const AddMenuItem = styled.button`
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: none;
  color: var(--sk-text);
  cursor: pointer;
  font-size: 11px;
  text-align: left;
  pointer-events: auto;

  &:hover {
    background: var(--sk-overlay-medium);
  }
`;

export const OverlayLabel = styled.span`
  flex: 1;
  color: var(--sk-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
`;

export const OverlayIcon = styled.span`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--sk-text-muted);
  flex-shrink: 0;
  min-width: 14px;
`;

export const RemoveButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 2px;
  background: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;
  opacity: 0;
  transition: opacity 0.15s;

  &:hover {
    background: var(--sk-danger-alpha, rgba(220, 38, 38, 0.15));
    color: var(--sk-danger);
  }
`;

export const HiddenFileInput = styled.input`
  display: none;
`;

export const OverlayEditRow = styled.div`
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 0;
  font-size: 10px;
`;

export const OverlayTextInput = styled.textarea`
  flex: 1;
  padding: 4px 6px;
  font-size: 11px;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-input-bg);
  color: var(--sk-text);
  outline: none;
  min-width: 0;
  pointer-events: auto;
  resize: vertical;
  min-height: 48px;
  max-height: 200px;
  font-family: inherit;
  line-height: 1.4;
  white-space: pre-wrap;
  word-break: break-word;

  &:focus {
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 1px var(--sk-primary);
  }
`;

export const OverlayNumInput = styled(NumericInput)`
  width: 40px;
  font-size: 10px;
`;

export const ColorInput = styled.input`
  width: 28px;
  height: 22px;
  padding: 0;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: none;
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;

  &::-webkit-color-swatch-wrapper {
    padding: 1px;
  }
  &::-webkit-color-swatch {
    border: none;
    border-radius: 2px;
  }
`;

/** Convert [R, G, B, A] to a hex color string (#rrggbb) for <input type="color"> */
export function rgbaToHex(color: [number, number, number, number]): string {
  const [r, g, b] = color;
  return `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`;
}

/** Convert a hex color string (#rrggbb) + alpha byte -> [R, G, B, A] */
export function hexToRgba(hex: string, alpha: number): [number, number, number, number] {
  const r = Number.parseInt(hex.slice(1, 3), 16);
  const g = Number.parseInt(hex.slice(3, 5), 16);
  const b = Number.parseInt(hex.slice(5, 7), 16);
  return [r, g, b, alpha];
}

/** Bundled fonts that are always available (compiled into the server binary). */
export const BUNDLED_FONT_OPTIONS = [
  { value: 'dejavu-sans', label: 'DejaVu Sans' },
  { value: 'dejavu-serif', label: 'DejaVu Serif' },
  { value: 'dejavu-sans-mono', label: 'DejaVu Mono' },
  { value: 'dejavu-sans-bold', label: 'DejaVu Sans Bold' },
  { value: 'dejavu-serif-bold', label: 'DejaVu Serif Bold' },
  { value: 'dejavu-sans-mono-bold', label: 'DejaVu Mono Bold' },
] as const;

export const FontSelect = styled.select`
  flex: 1;
  padding: 2px 4px;
  font-size: 10px;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  color-scheme: dark light;
  outline: none;
  min-width: 0;
  pointer-events: auto;
  cursor: pointer;

  option {
    background: var(--sk-panel-bg);
    color: var(--sk-text);
  }

  &:focus {
    border-color: var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
`;

export const VisibilityButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 2px;
  background: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;

  &:hover {
    color: var(--sk-text);
  }
`;

export const LayerListItem = styled.div<{ isSelected?: boolean; isHidden?: boolean }>`
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 4px;
  font-size: 11px;
  border-radius: 3px;
  cursor: pointer;
  pointer-events: auto;
  opacity: ${(p) => (p.isHidden ? 0.45 : 1)};
  background: ${(p) => (p.isSelected ? 'var(--sk-overlay-medium)' : 'transparent')};
  border: 1px solid ${(p) => (p.isSelected ? 'var(--sk-primary)' : 'transparent')};

  &:hover {
    background: var(--sk-overlay-medium);
  }

  /* Show remove button on hover */
  &:hover .layer-remove-btn {
    opacity: 1;
  }

  /* Show z-order buttons on hover */
  &:hover .layer-z-btn {
    opacity: 1;
  }
`;

/** Unified entry representing any layer kind for sorting / display */
export interface CompositorEntry {
  id: string;
  kind: LayerKind;
  label: string;
  zIndex: number;
  visible: boolean;
}

// ── Rotation presets ────────────────────────────────────────────────────────

export const ROTATION_PRESETS = [0, 90, 180, 270] as const;

export const RotationPresetsRow = styled.div`
  display: flex;
  align-items: center;
  gap: 3px;
`;

export const PresetButton = styled.button<{ isActive?: boolean }>`
  padding: 2px 6px;
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  border: 1px solid ${(p) => (p.isActive ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 3px;
  background: ${(p) => (p.isActive ? 'var(--sk-overlay-medium)' : 'transparent')};
  color: ${(p) => (p.isActive ? 'var(--sk-primary)' : 'var(--sk-text-muted)')};
  cursor: pointer;
  pointer-events: auto;
  transition: all 0.1s;
  flex: 1;

  &:hover:not(:disabled) {
    border-color: var(--sk-primary);
    color: var(--sk-text);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }
`;

export const ResetButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: transparent;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;
  transition: all 0.1s;

  &:hover:not(:disabled) {
    border-color: var(--sk-primary);
    color: var(--sk-text);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }
`;

// ── Side Inspector Panel ────────────────────────────────────────────────────

export const SidePanel = styled.div`
  position: absolute;
  left: calc(100% + 28px);
  top: 18px;
  width: 280px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  box-shadow: 0 2px 8px var(--sk-shadow, rgba(0, 0, 0, 0.15));
  pointer-events: auto;
  z-index: 5;
`;

export const SidePanelDivider = styled.div`
  height: 1px;
  background: var(--sk-border);
  margin: 8px 0;
`;

export const InspectorControls = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

export const InspectorHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-bottom: 4px;
  border-bottom: 1px solid var(--sk-border);
`;

export const InspectorTitle = styled.span`
  font-size: 11px;
  font-weight: 600;
  color: var(--sk-primary);
`;

export const InspectorSection = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

export const InspectorSectionLabel = styled.div`
  font-size: 10px;
  font-weight: 600;
  color: var(--sk-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.3px;
`;

export const MirrorToggleRow = styled.div`
  display: flex;
  gap: 4px;
`;

export const MirrorButton = styled.button<{ isActive?: boolean }>`
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border: 1px solid ${(p) => (p.isActive ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 4px;
  background: ${(p) =>
    p.isActive ? 'rgba(var(--sk-primary-rgb, 99,102,241), 0.15)' : 'transparent'};
  color: ${(p) => (p.isActive ? 'var(--sk-primary)' : 'var(--sk-text-muted)')};
  font-size: 10px;
  cursor: pointer;
  transition: all 0.15s ease;
  &:hover:not(:disabled) {
    border-color: var(--sk-primary);
    color: var(--sk-text);
  }
  &:disabled {
    opacity: 0.4;
    cursor: default;
  }
`;
