// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as RadixTooltip from '@radix-ui/react-tooltip';
import { render, screen, fireEvent } from '@testing-library/react';
import { Provider as JotaiProvider } from 'jotai/react';
import { describe, it, expect, vi } from 'vitest';

import type { NodeState, NodeStats } from '@/types/types';

import { NodeStateIndicator } from './NodeStateIndicator';

vi.mock('@/utils/logger', () => ({
  getLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

vi.mock('@/stores/sessionStore', () => ({
  useSessionStore: () => null,
}));

function renderIndicator(
  state: NodeState,
  opts: { showLabel?: boolean; stats?: NodeStats; nodeId?: string; sessionId?: string } = {}
) {
  return render(
    <RadixTooltip.Provider delayDuration={0}>
      <JotaiProvider>
        <NodeStateIndicator
          state={state}
          showLabel={opts.showLabel}
          stats={opts.stats}
          nodeId={opts.nodeId}
          sessionId={opts.sessionId}
        />
      </JotaiProvider>
    </RadixTooltip.Provider>
  );
}

function openTooltip() {
  const trigger = screen.getByTestId('state-dot').closest('.nodrag');
  expect(trigger).not.toBeNull();
  fireEvent.focus(trigger!);
}

describe('NodeStateIndicator', () => {
  describe('string states', () => {
    it('renders Creating state with label', () => {
      renderIndicator('Creating', { showLabel: true });
      expect(screen.getByText('Creating')).toBeInTheDocument();
    });

    it('renders Initializing state with label', () => {
      renderIndicator('Initializing', { showLabel: true });
      expect(screen.getByText('Initializing')).toBeInTheDocument();
    });

    it('renders Running state with label', () => {
      renderIndicator('Running', { showLabel: true });
      expect(screen.getByText('Running')).toBeInTheDocument();
    });

    it('hides label by default', () => {
      renderIndicator('Running');
      expect(screen.queryByText('Running')).not.toBeInTheDocument();
    });
  });

  describe('object states', () => {
    it('renders Failed state with label', () => {
      const state: NodeState = { Failed: { reason: 'connection lost' } };
      renderIndicator(state, { showLabel: true });
      expect(screen.getByText('Failed')).toBeInTheDocument();
    });

    it('renders Stopped state with label', () => {
      const state: NodeState = { Stopped: { reason: 'completed' } };
      renderIndicator(state, { showLabel: true });
      expect(screen.getByText('Stopped')).toBeInTheDocument();
    });

    it('renders Recovering state with label', () => {
      const state: NodeState = { Recovering: { reason: 'retrying', details: null } };
      renderIndicator(state, { showLabel: true });
      expect(screen.getByText('Recovering')).toBeInTheDocument();
    });

    it('renders Degraded state with label', () => {
      const state: NodeState = { Degraded: { reason: 'slow input', details: null } };
      renderIndicator(state, { showLabel: true });
      expect(screen.getByText('Degraded')).toBeInTheDocument();
    });
  });

  describe('indicator dot', () => {
    it('renders an indicator dot for each state', () => {
      const states: NodeState[] = [
        'Creating',
        'Initializing',
        'Running',
        { Failed: { reason: 'err' } },
        { Stopped: { reason: 'completed' } },
        { Recovering: { reason: 'retry', details: null } },
        { Degraded: { reason: 'slow', details: null } },
      ];

      for (const state of states) {
        const { unmount } = renderIndicator(state);
        expect(screen.getByTestId('state-dot')).toBeInTheDocument();
        unmount();
      }
    });
  });

  describe('error badge', () => {
    it('does not show error badge when tooltip is closed', () => {
      const stats: NodeStats = {
        received: BigInt(100),
        sent: BigInt(90),
        errored: BigInt(10),
        discarded: BigInt(0),
        duration_secs: 10,
      };
      renderIndicator('Running', { stats });
      expect(screen.queryByTestId('error-badge')).not.toBeInTheDocument();
    });
  });

  describe('tooltip content', () => {
    const stats: NodeStats = {
      received: BigInt(100),
      sent: BigInt(90),
      errored: BigInt(0),
      discarded: BigInt(0),
      duration_secs: 10,
    };

    it('shows static state details when opened without a node id', async () => {
      renderIndicator('Running', { stats });
      openTooltip();
      expect(
        (await screen.findAllByText('Node is operating normally and processing data')).length
      ).toBeGreaterThan(0);
    });

    it('shows live state details when opened with a node id', async () => {
      renderIndicator('Running', { stats, nodeId: 'node-1', sessionId: 'session-1' });
      openTooltip();
      expect(
        (await screen.findAllByText('Node is operating normally and processing data')).length
      ).toBeGreaterThan(0);
    });

    it('shows degraded details with slow pins when opened with a node id', async () => {
      const state: NodeState = {
        Degraded: { reason: 'slow input', details: { slow_pins: ['in_0'] } },
      };
      renderIndicator(state, { nodeId: 'node-1', sessionId: 'session-1' });
      openTooltip();
      expect((await screen.findAllByText('slow input')).length).toBeGreaterThan(0);
    });
  });

  describe('all stop reasons render', () => {
    const stopReasons = [
      'completed',
      'input_closed',
      'output_closed',
      'shutdown',
      'no_inputs',
      'unknown',
    ] as const;

    for (const reason of stopReasons) {
      it(`renders Stopped with reason "${reason}"`, () => {
        const state: NodeState = { Stopped: { reason } };
        const { container } = renderIndicator(state, { showLabel: true });
        expect(screen.getByText('Stopped')).toBeInTheDocument();
        expect(container).toBeTruthy();
      });
    }
  });
});
