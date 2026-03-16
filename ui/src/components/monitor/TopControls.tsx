// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Top-right control bar for the Monitor View canvas.
 *
 * Shows connection status, preview button, and session delete.
 */

import React from 'react';

import { TopRightControls, ButtonGroup } from '@/components/monitor/MonitorView.styles';
import { SKTooltip } from '@/components/Tooltip';
import { Button } from '@/components/ui/Button';

import { ConnectionStatus } from './ConnectionStatus';

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

export interface TopControlsProps {
  isConnected: boolean;
  selectedSessionId: string | null;
  onDelete: () => void;
  onStartPreview: () => void;
  isPreviewConnected: boolean;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

// Memoized TopControls component to prevent re-renders during drag
export const TopControls = React.memo(
  ({
    isConnected,
    selectedSessionId,
    onDelete,
    onStartPreview,
    isPreviewConnected,
  }: TopControlsProps) => {
    return (
      <TopRightControls>
        <ButtonGroup>
          <ConnectionStatus connected={isConnected} />
          {selectedSessionId && !isPreviewConnected && (
            <SKTooltip content="Connect to MoQ gateway and start watching the output preview">
              <Button variant="ghost" size="small" onClick={onStartPreview}>
                Preview
              </Button>
            </SKTooltip>
          )}
        </ButtonGroup>
        {selectedSessionId && (
          <ButtonGroup>
            <SKTooltip content="Delete Session" side="bottom">
              <Button variant="danger" size="small" onClick={onDelete}>
                Delete
              </Button>
            </SKTooltip>
          </ButtonGroup>
        )}
      </TopRightControls>
    );
  }
);
