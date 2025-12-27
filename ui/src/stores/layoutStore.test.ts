// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  LAYOUT_PRESETS,
  registerFitViewCallback,
  useLayoutStore,
  type LayoutPreset,
} from './layoutStore';

describe('layoutStore', () => {
  beforeEach(() => {
    // Reset to default state
    const { setPreset } = useLayoutStore.getState();
    setPreset('balanced');

    // Clear all timers
    vi.clearAllTimers();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('initial state', () => {
    it('should start with balanced preset', () => {
      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('balanced');
      expect(state.leftCollapsed).toBe(false);
      expect(state.rightCollapsed).toBe(false);
      expect(state.leftSize).toBe(20);
      expect(state.rightSize).toBe(20);
    });
  });

  describe('setPreset', () => {
    it('should apply focus-canvas preset', () => {
      const { setPreset } = useLayoutStore.getState();
      setPreset('focus-canvas');

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('focus-canvas');
      expect(state.leftCollapsed).toBe(true);
      expect(state.rightCollapsed).toBe(true);
    });

    it('should apply balanced preset', () => {
      const { setPreset } = useLayoutStore.getState();
      setPreset('balanced');

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('balanced');
      expect(state.leftCollapsed).toBe(false);
      expect(state.rightCollapsed).toBe(false);
      expect(state.leftSize).toBe(20);
      expect(state.rightSize).toBe(20);
    });

    it('should apply inspector-focus preset', () => {
      const { setPreset } = useLayoutStore.getState();
      setPreset('inspector-focus');

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('inspector-focus');
      expect(state.leftCollapsed).toBe(true);
      expect(state.rightCollapsed).toBe(false);
      expect(state.rightSize).toBe(35);
    });

    it('should apply palette-focus preset', () => {
      const { setPreset } = useLayoutStore.getState();
      setPreset('palette-focus');

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('palette-focus');
      expect(state.leftCollapsed).toBe(false);
      expect(state.rightCollapsed).toBe(true);
      expect(state.leftSize).toBe(35);
    });

    it('should use default sizes when preset does not specify them', () => {
      const { setPreset } = useLayoutStore.getState();
      setPreset('focus-canvas');

      const state = useLayoutStore.getState();
      // focus-canvas doesn't specify sizes, should use defaults
      expect(state.leftSize).toBe(20);
      expect(state.rightSize).toBe(20);
    });
  });

  describe('manual adjustments and preset detection', () => {
    it('should detect focus-canvas preset when both sidebars collapsed', () => {
      const { setLeftCollapsed, setRightCollapsed } = useLayoutStore.getState();

      setLeftCollapsed(true);
      setRightCollapsed(true);

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('focus-canvas');
    });

    it('should detect balanced preset when layout matches exactly', () => {
      const { setPreset, setLeftCollapsed, setRightCollapsed, setLeftSize, setRightSize } =
        useLayoutStore.getState();

      // Start with different preset
      setPreset('focus-canvas');

      // Manually configure to balanced settings
      setLeftCollapsed(false);
      setRightCollapsed(false);
      setLeftSize(20);
      setRightSize(20);

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('balanced');
    });

    it('should detect inspector-focus preset when right panel expanded', () => {
      const { setLeftCollapsed, setRightCollapsed, setRightSize } = useLayoutStore.getState();

      setLeftCollapsed(true);
      setRightCollapsed(false);
      setRightSize(35);

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('inspector-focus');
    });

    it('should detect palette-focus preset when left panel expanded', () => {
      const { setLeftCollapsed, setRightCollapsed, setLeftSize } = useLayoutStore.getState();

      setLeftCollapsed(false);
      setRightCollapsed(true);
      setLeftSize(35);

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('palette-focus');
    });

    it('should fallback to balanced when layout does not match any preset', () => {
      const { setLeftCollapsed, setRightCollapsed, setLeftSize } = useLayoutStore.getState();

      // Create a custom layout that doesn't match any preset
      setLeftCollapsed(false);
      setRightCollapsed(false);
      setLeftSize(25); // Not 20, doesn't match balanced

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('balanced'); // Fallback
    });
  });

  describe('size tolerance', () => {
    it('should detect preset with size within tolerance (0.5)', () => {
      const { setLeftCollapsed, setRightCollapsed, setLeftSize, setRightSize } =
        useLayoutStore.getState();

      // Balanced is 20/20, try 20.3/20.4 (within 0.5 tolerance)
      setLeftCollapsed(false);
      setRightCollapsed(false);
      setLeftSize(20.3);
      setRightSize(20.4);

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('balanced');
    });

    it('should not detect preset with size outside tolerance', () => {
      const { setLeftCollapsed, setRightCollapsed, setLeftSize, setRightSize } =
        useLayoutStore.getState();

      // Balanced is 20/20, try 20.6/20.7 (outside 0.5 tolerance)
      setLeftCollapsed(false);
      setRightCollapsed(false);
      setLeftSize(20.6);
      setRightSize(20.7);

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('balanced'); // Fallback, not detected as balanced
    });
  });

  describe('applyConfig', () => {
    it('should apply config and detect matching preset', () => {
      const { applyConfig } = useLayoutStore.getState();

      applyConfig({
        leftCollapsed: true,
        rightCollapsed: true,
      });

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('focus-canvas');
      expect(state.leftCollapsed).toBe(true);
      expect(state.rightCollapsed).toBe(true);
    });

    it('should use default sizes when not specified in config', () => {
      const { applyConfig } = useLayoutStore.getState();

      applyConfig({
        leftCollapsed: false,
        rightCollapsed: false,
        // No sizes specified
      });

      const state = useLayoutStore.getState();
      expect(state.leftSize).toBe(20);
      expect(state.rightSize).toBe(20);
    });

    it('should apply custom sizes from config', () => {
      const { applyConfig } = useLayoutStore.getState();

      applyConfig({
        leftCollapsed: false,
        rightCollapsed: true,
        leftSize: 35,
      });

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('palette-focus');
      expect(state.leftSize).toBe(35);
    });
  });

  describe('fitView callbacks', () => {
    it('should register and trigger fitView callbacks', () => {
      vi.useFakeTimers();

      const callback = vi.fn();
      const unregister = registerFitViewCallback(callback);

      // Trigger layout change
      const { setPreset } = useLayoutStore.getState();
      setPreset('focus-canvas');

      // Callback should not fire immediately
      expect(callback).not.toHaveBeenCalled();

      // Fast-forward 200ms
      vi.advanceTimersByTime(200);

      // Callback should have fired
      expect(callback).toHaveBeenCalledTimes(1);

      // Cleanup
      unregister();
      vi.useRealTimers();
    });

    it('should not trigger callback after unregistering', () => {
      vi.useFakeTimers();

      const callback = vi.fn();
      const unregister = registerFitViewCallback(callback);

      // Unregister immediately
      unregister();

      // Trigger layout change
      const { setPreset } = useLayoutStore.getState();
      setPreset('focus-canvas');

      // Fast-forward 200ms
      vi.advanceTimersByTime(200);

      // Callback should not have fired
      expect(callback).not.toHaveBeenCalled();

      vi.useRealTimers();
    });

    it('should support multiple callbacks', () => {
      vi.useFakeTimers();

      const callback1 = vi.fn();
      const callback2 = vi.fn();

      registerFitViewCallback(callback1);
      registerFitViewCallback(callback2);

      // Trigger layout change
      const { setPreset } = useLayoutStore.getState();
      setPreset('inspector-focus');

      // Fast-forward 200ms
      vi.advanceTimersByTime(200);

      // Both callbacks should have fired
      expect(callback1).toHaveBeenCalledTimes(1);
      expect(callback2).toHaveBeenCalledTimes(1);

      vi.useRealTimers();
    });
  });

  describe('preset definitions', () => {
    it('should have all required presets defined', () => {
      const presets: LayoutPreset[] = [
        'focus-canvas',
        'balanced',
        'inspector-focus',
        'palette-focus',
      ];

      presets.forEach((preset) => {
        expect(LAYOUT_PRESETS[preset]).toBeDefined();
        expect(LAYOUT_PRESETS[preset].id).toBe(preset);
        expect(LAYOUT_PRESETS[preset].name).toBeTruthy();
        expect(LAYOUT_PRESETS[preset].icon).toBeTruthy();
        expect(LAYOUT_PRESETS[preset].description).toBeTruthy();
        expect(LAYOUT_PRESETS[preset].config).toBeDefined();
      });
    });

    it('should have valid config for each preset', () => {
      Object.values(LAYOUT_PRESETS).forEach((preset) => {
        const { config } = preset;

        // Collapsed flags should be boolean
        expect(typeof config.leftCollapsed).toBe('boolean');
        expect(typeof config.rightCollapsed).toBe('boolean');

        // Sizes should be undefined or number in range 10-50
        if (config.leftSize !== undefined) {
          expect(config.leftSize).toBeGreaterThanOrEqual(10);
          expect(config.leftSize).toBeLessThanOrEqual(50);
        }
        if (config.rightSize !== undefined) {
          expect(config.rightSize).toBeGreaterThanOrEqual(10);
          expect(config.rightSize).toBeLessThanOrEqual(50);
        }
      });
    });
  });

  describe('state transitions', () => {
    it('should handle rapid preset changes', () => {
      const { setPreset } = useLayoutStore.getState();

      setPreset('focus-canvas');
      setPreset('balanced');
      setPreset('inspector-focus');
      setPreset('palette-focus');

      const state = useLayoutStore.getState();
      expect(state.currentPreset).toBe('palette-focus');
      expect(state.leftCollapsed).toBe(false);
      expect(state.rightCollapsed).toBe(true);
      expect(state.leftSize).toBe(35);
    });

    it('should handle alternating collapse state', () => {
      const { setLeftCollapsed, setRightCollapsed } = useLayoutStore.getState();

      setLeftCollapsed(true);
      setRightCollapsed(false);
      setLeftCollapsed(false);
      setRightCollapsed(true);

      const state = useLayoutStore.getState();
      expect(state.leftCollapsed).toBe(false);
      expect(state.rightCollapsed).toBe(true);
    });
  });
});
