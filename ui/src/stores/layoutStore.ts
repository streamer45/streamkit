// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type LayoutPreset = 'focus-canvas' | 'balanced' | 'inspector-focus' | 'palette-focus';

export interface LayoutConfig {
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftSize?: number; // Percentage (10-50)
  rightSize?: number; // Percentage (10-50)
}

export interface LayoutPresetDefinition {
  id: LayoutPreset;
  name: string;
  icon: string;
  description: string;
  config: LayoutConfig;
}

const DEFAULT_PRESET: LayoutPreset = 'balanced';
const DEFAULT_LEFT_SIZE = 20;
const DEFAULT_RIGHT_SIZE = 20;
const SIZE_TOLERANCE = 0.5;
const PERSISTED_SIZE_PRECISION = 2;

interface LayoutSnapshot {
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftSize: number;
  rightSize: number;
}

// Preset definitions
export const LAYOUT_PRESETS: Record<LayoutPreset, LayoutPresetDefinition> = {
  'focus-canvas': {
    id: 'focus-canvas',
    name: 'Focus Canvas',
    icon: '⬌',
    description: 'Both sidebars collapsed for maximum canvas space',
    config: {
      leftCollapsed: true,
      rightCollapsed: true,
    },
  },
  balanced: {
    id: 'balanced',
    name: 'Balanced',
    icon: '⊞',
    description: 'Default comfortable working state with both panels visible',
    config: {
      leftCollapsed: false,
      rightCollapsed: false,
      leftSize: 20,
      rightSize: 20,
    },
  },
  'inspector-focus': {
    id: 'inspector-focus',
    name: 'Inspector Focus',
    icon: '⊣',
    description: 'Maximized right panel for parameter tuning and YAML editing',
    config: {
      leftCollapsed: true,
      rightCollapsed: false,
      rightSize: 35,
    },
  },
  'palette-focus': {
    id: 'palette-focus',
    name: 'Palette Focus',
    icon: '⊢',
    description: 'Maximized left panel for exploring available nodes',
    config: {
      leftCollapsed: false,
      rightCollapsed: true,
      leftSize: 35,
    },
  },
};

const approximatelyEqual = (a: number, b: number) => Math.abs(a - b) <= SIZE_TOLERANCE;

const normalizePanelSize = (value: unknown, fallback: number) => {
  const raw = normalizeNumber(value, fallback);
  const clamped = Math.max(0, Math.min(100, raw));
  const scale = 10 ** PERSISTED_SIZE_PRECISION;
  return Math.round(clamped * scale) / scale;
};

const findMatchingPreset = (snapshot: LayoutSnapshot): LayoutPreset | undefined => {
  for (const preset of Object.values(LAYOUT_PRESETS)) {
    const { config } = preset;
    if (config.leftCollapsed !== snapshot.leftCollapsed) continue;
    if (config.rightCollapsed !== snapshot.rightCollapsed) continue;
    if (config.leftSize !== undefined && !approximatelyEqual(snapshot.leftSize, config.leftSize)) {
      continue;
    }
    if (
      config.rightSize !== undefined &&
      !approximatelyEqual(snapshot.rightSize, config.rightSize)
    ) {
      continue;
    }
    return preset.id;
  }
  return undefined;
};

// Callback registry for fitView triggers (not persisted)
type FitViewCallback = () => void;
const fitViewCallbacks = new Set<FitViewCallback>();

export const registerFitViewCallback = (callback: FitViewCallback): (() => void) => {
  fitViewCallbacks.add(callback);
  // Return cleanup function
  return () => fitViewCallbacks.delete(callback);
};

const triggerFitView = () => {
  // Delay to allow panel animations to complete
  setTimeout(() => {
    fitViewCallbacks.forEach((callback) => callback());
  }, 200);
};

interface LayoutStore {
  // Current preset
  currentPreset: LayoutPreset;

  // Current layout state (may differ from preset if user manually adjusted)
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftSize: number;
  rightSize: number;

  // Actions
  setPreset: (preset: LayoutPreset) => void;
  setLeftCollapsed: (collapsed: boolean) => void;
  setRightCollapsed: (collapsed: boolean) => void;
  setLeftSize: (size: number) => void;
  setRightSize: (size: number) => void;
  applyConfig: (config: LayoutConfig) => void;
}

const normalizeNumber = (value: unknown, fallback: number) => {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string') {
    const parsed = Number.parseFloat(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return fallback;
};

export const useLayoutStore = create<LayoutStore>()(
  persist(
    (set) => ({
      currentPreset: DEFAULT_PRESET,
      leftCollapsed: false,
      rightCollapsed: false,
      leftSize: DEFAULT_LEFT_SIZE,
      rightSize: DEFAULT_RIGHT_SIZE,

      setPreset: (preset) => {
        const presetDef = LAYOUT_PRESETS[preset];
        set({
          currentPreset: preset,
          leftCollapsed: presetDef.config.leftCollapsed,
          rightCollapsed: presetDef.config.rightCollapsed,
          leftSize: presetDef.config.leftSize ?? DEFAULT_LEFT_SIZE,
          rightSize: presetDef.config.rightSize ?? DEFAULT_RIGHT_SIZE,
        });
        // Trigger fitView after layout changes
        triggerFitView();
      },

      setLeftCollapsed: (collapsed) =>
        set((state) => {
          if (collapsed === state.leftCollapsed) return state;
          const snapshot: LayoutSnapshot = {
            leftCollapsed: collapsed,
            rightCollapsed: state.rightCollapsed,
            leftSize: state.leftSize,
            rightSize: state.rightSize,
          };
          return {
            leftCollapsed: collapsed,
            currentPreset: findMatchingPreset(snapshot) ?? DEFAULT_PRESET,
          };
        }),

      setRightCollapsed: (collapsed) =>
        set((state) => {
          if (collapsed === state.rightCollapsed) return state;
          const snapshot: LayoutSnapshot = {
            leftCollapsed: state.leftCollapsed,
            rightCollapsed: collapsed,
            leftSize: state.leftSize,
            rightSize: state.rightSize,
          };
          return {
            rightCollapsed: collapsed,
            currentPreset: findMatchingPreset(snapshot) ?? DEFAULT_PRESET,
          };
        }),

      setLeftSize: (size) =>
        set((state) => {
          const nextSize = normalizePanelSize(size, state.leftSize);
          if (Object.is(nextSize, state.leftSize)) return state;
          const snapshot: LayoutSnapshot = {
            leftCollapsed: state.leftCollapsed,
            rightCollapsed: state.rightCollapsed,
            leftSize: nextSize,
            rightSize: state.rightSize,
          };
          return {
            leftSize: nextSize,
            currentPreset: findMatchingPreset(snapshot) ?? DEFAULT_PRESET,
          };
        }),

      setRightSize: (size) =>
        set((state) => {
          const nextSize = normalizePanelSize(size, state.rightSize);
          if (Object.is(nextSize, state.rightSize)) return state;
          const snapshot: LayoutSnapshot = {
            leftCollapsed: state.leftCollapsed,
            rightCollapsed: state.rightCollapsed,
            leftSize: state.leftSize,
            rightSize: nextSize,
          };
          return {
            rightSize: nextSize,
            currentPreset: findMatchingPreset(snapshot) ?? DEFAULT_PRESET,
          };
        }),

      applyConfig: (config) =>
        set(() => {
          const snapshot: LayoutSnapshot = {
            leftCollapsed: config.leftCollapsed,
            rightCollapsed: config.rightCollapsed,
            leftSize: config.leftSize ?? DEFAULT_LEFT_SIZE,
            rightSize: config.rightSize ?? DEFAULT_RIGHT_SIZE,
          };
          return {
            leftCollapsed: snapshot.leftCollapsed,
            rightCollapsed: snapshot.rightCollapsed,
            leftSize: snapshot.leftSize,
            rightSize: snapshot.rightSize,
            currentPreset: findMatchingPreset(snapshot) ?? DEFAULT_PRESET,
          };
        }),
    }),
    {
      name: 'layout-storage',
      merge: (persisted, current) => {
        const next = persisted as Partial<LayoutStore> | undefined;
        if (!next) return current;

        const persistedPreset = next.currentPreset as LayoutPreset | undefined;
        const isBalancedButCollapsed =
          persistedPreset === 'balanced' &&
          (next.leftCollapsed === true || next.rightCollapsed === true);

        if (isBalancedButCollapsed) {
          const balanced = LAYOUT_PRESETS.balanced.config;
          return {
            ...current,
            currentPreset: 'balanced',
            leftCollapsed: balanced.leftCollapsed,
            rightCollapsed: balanced.rightCollapsed,
            leftSize: balanced.leftSize ?? DEFAULT_LEFT_SIZE,
            rightSize: balanced.rightSize ?? DEFAULT_RIGHT_SIZE,
          };
        }

        const mergedLeftCollapsed =
          typeof next.leftCollapsed === 'boolean' ? next.leftCollapsed : current.leftCollapsed;
        const mergedRightCollapsed =
          typeof next.rightCollapsed === 'boolean' ? next.rightCollapsed : current.rightCollapsed;
        const mergedLeftSize = normalizePanelSize(next.leftSize, current.leftSize);
        const mergedRightSize = normalizePanelSize(next.rightSize, current.rightSize);

        const snapshot: LayoutSnapshot = {
          leftCollapsed: mergedLeftCollapsed,
          rightCollapsed: mergedRightCollapsed,
          leftSize: mergedLeftSize,
          rightSize: mergedRightSize,
        };

        return {
          ...current,
          leftCollapsed: mergedLeftCollapsed,
          rightCollapsed: mergedRightCollapsed,
          leftSize: mergedLeftSize,
          rightSize: mergedRightSize,
          currentPreset: findMatchingPreset(snapshot) ?? DEFAULT_PRESET,
        };
      },
    }
  )
);
