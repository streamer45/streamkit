// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixSlider from '@radix-ui/react-slider';

export const CompactSliderRoot = styled(RadixSlider.Root)`
  position: relative;
  display: flex;
  align-items: center;
  user-select: none;
  touch-action: none;
  flex: 1;
  height: 16px;
  min-width: 0;

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

export const CompactSliderTrack = styled(RadixSlider.Track)`
  position: relative;
  flex-grow: 1;
  height: 3px;
  background: var(--sk-border);
  border-radius: 9999px;
`;

export const CompactSliderRange = styled(RadixSlider.Range)`
  position: absolute;
  height: 100%;
  background: var(--sk-primary);
  border-radius: 9999px;
`;

export const CompactSliderThumb = styled(RadixSlider.Thumb)`
  display: block;
  width: 12px;
  height: 12px;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-primary);
  border-radius: 50%;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  cursor: grab;
  transition: border-color 0.1s ease;

  &:hover {
    border-color: var(--sk-primary-hover, var(--sk-primary));
  }

  &:focus-visible {
    outline: none;
    box-shadow: var(--sk-focus-ring, 0 0 0 3px rgba(14, 165, 233, 0.2));
  }

  &:active {
    cursor: grabbing;
  }

  &[data-disabled] {
    cursor: not-allowed;
  }
`;
