// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { LogResponse } from '@/services/logs';

import LogsView from './LogsView';

vi.mock('@/hooks/usePermissions', () => ({
  usePermissions: () => ({ role: 'admin', isAdmin: () => true }),
}));

const fetchLogsMock = vi.hoisted(() => vi.fn());
vi.mock('@/services/logs', () => ({
  fetchLogs: fetchLogsMock,
  createLogStream: vi.fn(),
}));

vi.mock('./admin/AdminNav', () => ({ default: () => null }));

describe('LogsView', () => {
  it('does not show the empty state while the initial fetch is in flight', async () => {
    let resolveFetch!: (r: LogResponse) => void;
    fetchLogsMock.mockReturnValue(
      new Promise<LogResponse>((resolve) => {
        resolveFetch = resolve;
      })
    );

    render(<LogsView />);

    expect(screen.queryByText('No log lines to display.')).not.toBeInTheDocument();
    await waitFor(() => expect(fetchLogsMock).toHaveBeenCalled());
    expect(screen.queryByText('No log lines to display.')).not.toBeInTheDocument();

    resolveFetch({
      lines: ['line one'],
      file_size: 10,
      next_offset: 0,
      has_more: false,
    });

    expect(await screen.findByText('line one')).toBeInTheDocument();
  });

  it('shows the empty state when the fetch returns no lines', async () => {
    fetchLogsMock.mockResolvedValue({
      lines: [],
      file_size: 0,
      next_offset: 0,
      has_more: false,
    });

    render(<LogsView />);

    expect(await screen.findByText('No log lines to display.')).toBeInTheDocument();
  });
});
