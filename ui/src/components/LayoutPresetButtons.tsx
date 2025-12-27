// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { useShallow } from 'zustand/shallow';

import { SKTooltip } from '@/components/Tooltip';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import { useLayoutStore, LAYOUT_PRESETS, type LayoutPreset } from '@/stores/layoutStore';

const ButtonsContainer = styled.div`
  display: flex;
  gap: 4px;
  align-items: center;
`;

const PresetButton = styled.button<{ isActive: boolean; isDark: boolean }>`
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: 2px solid ${({ isActive }) => (isActive ? 'var(--sk-accent-indigo)' : 'transparent')};
  border-radius: 6px;
  background: ${({ isActive, isDark }) =>
    isActive ? (isDark ? 'var(--sk-overlay-strong)' : 'var(--sk-overlay-medium)') : 'transparent'};
  color: ${({ isActive, isDark }) =>
    isActive
      ? isDark
        ? 'var(--sk-accent-indigo-light)'
        : 'var(--sk-accent-indigo)'
      : 'var(--sk-text-muted)'};
  font-size: 16px;
  cursor: pointer;
  transition: none;

  &:hover {
    background: ${({ isActive }) => (isActive ? 'var(--sk-overlay-strong)' : 'var(--sk-hover-bg)')};
    border-color: ${({ isActive }) =>
      isActive ? 'var(--sk-accent-indigo-light)' : 'var(--sk-border-strong)'};
    color: var(--sk-text);
  }
`;

const Divider = styled.div`
  width: 1px;
  height: 20px;
  background: var(--sk-border);
  margin: 0 4px;
`;

export function LayoutPresetButtons() {
  const { currentPreset, setPreset } = useLayoutStore(
    useShallow((s) => ({
      currentPreset: s.currentPreset,
      setPreset: s.setPreset,
    }))
  );
  const colorMode = useResolvedColorMode();
  const isDark = colorMode === 'dark';

  const presets: LayoutPreset[] = ['palette-focus', 'balanced', 'focus-canvas', 'inspector-focus'];

  return (
    <>
      <Divider />
      <ButtonsContainer>
        {presets.map((presetId) => {
          const preset = LAYOUT_PRESETS[presetId];
          return (
            <SKTooltip key={presetId} content={preset.name} side="bottom">
              <PresetButton
                isActive={currentPreset === presetId}
                isDark={isDark}
                onClick={() => setPreset(presetId)}
              >
                {preset.icon}
              </PresetButton>
            </SKTooltip>
          );
        })}
      </ButtonsContainer>
    </>
  );
}
