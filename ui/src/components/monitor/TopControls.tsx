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
  onStopPreview: () => void;
  isPreviewConnected: boolean;
  isPreviewLoading: boolean;
  previewError: string | null;
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
    onStopPreview,
    isPreviewConnected,
    isPreviewLoading,
    previewError,
  }: TopControlsProps) => {
    return (
      <TopRightControls>
        <ButtonGroup>
          <ConnectionStatus connected={isConnected} />
          {selectedSessionId && isPreviewConnected && (
            <SKTooltip content="Stop the preview and tear down the preview subgraph">
              <Button variant="ghost" size="small" onClick={onStopPreview}>
                Stop Preview
              </Button>
            </SKTooltip>
          )}
          {selectedSessionId && !isPreviewConnected && (
            <SKTooltip
              content={
                previewError
                  ? `Preview failed: ${previewError}`
                  : 'Inject a preview tap into the pipeline and start watching'
              }
            >
              <Button
                variant="ghost"
                size="small"
                onClick={onStartPreview}
                disabled={isPreviewLoading}
              >
                {isPreviewLoading ? 'Starting...' : 'Preview'}
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
