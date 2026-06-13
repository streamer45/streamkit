// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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

  it('shows the error message when the fetch fails', async () => {
    fetchLogsMock.mockRejectedValue(new Error('boom'));

    render(<LogsView />);

    expect(await screen.findByText('boom')).toBeInTheDocument();
    expect(screen.getByText('No log lines to display.')).toBeInTheDocument();
  });

  it('shows a fallback error message when the fetch fails with a non-Error', async () => {
    fetchLogsMock.mockRejectedValue('boom');

    render(<LogsView />);

    expect(await screen.findByText('Failed to load logs')).toBeInTheDocument();
  });

  it('loads newer lines in the forward direction', async () => {
    fetchLogsMock.mockResolvedValueOnce({
      lines: ['old line'],
      file_size: 100,
      next_offset: 10,
      has_more: true,
    } satisfies LogResponse);

    render(<LogsView />);

    expect(await screen.findByText('old line')).toBeInTheDocument();

    fetchLogsMock.mockResolvedValueOnce({
      lines: ['older line'],
      file_size: 100,
      next_offset: 0,
      has_more: true,
    } satisfies LogResponse);

    const older = screen.getByTestId('logs-load-older');
    await waitFor(() => expect(older).toBeEnabled());
    fireEvent.click(older);

    expect(await screen.findByText('older line')).toBeInTheDocument();

    fetchLogsMock.mockResolvedValueOnce({
      lines: ['new line'],
      file_size: 100,
      next_offset: 80,
      has_more: true,
    } satisfies LogResponse);

    const newer = screen.getByTestId('logs-load-newer');
    await waitFor(() => expect(newer).toBeEnabled());
    fireEvent.click(newer);

    expect(await screen.findByText('new line')).toBeInTheDocument();
    expect(fetchLogsMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ direction: 'forward' })
    );
  });
});
