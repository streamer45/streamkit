// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { MintedTokensTable } from './MintedTokensTable';

function makeToken(overrides: Partial<import('@/services/auth').TokenInfo> = {}) {
  const base: import('@/services/auth').TokenInfo = {
    jti: 'jti',
    token_type: 'api',
    role: 'user',
    label: 'label',
    created_at: 100,
    exp: 200,
    revoked: false,
    created_by: 'test',
  };

  return { ...base, ...overrides };
}

describe('MintedTokensTable', () => {
  it('filters rows via the search input', async () => {
    const user = userEvent.setup();
    render(
      <MintedTokensTable
        isLoading={false}
        tokens={[
          makeToken({ jti: 'aaa', label: 'hello world' }),
          makeToken({ jti: 'bbb', label: 'something else' }),
        ]}
        canManageTokens={false}
        onRevoke={() => {}}
      />
    );

    expect(screen.getByText('aaa')).toBeInTheDocument();
    expect(screen.getByText('bbb')).toBeInTheDocument();

    const input = screen.getByPlaceholderText(/search tokens/i);
    await user.type(input, 'hello');

    expect(screen.getByText('aaa')).toBeInTheDocument();
    expect(screen.queryByText('bbb')).not.toBeInTheDocument();
  });

  it('sorts rows when clicking a header', async () => {
    const user = userEvent.setup();
    render(
      <MintedTokensTable
        isLoading={false}
        tokens={[
          makeToken({ jti: 'aaa', label: 'zeta', created_at: 100 }),
          makeToken({ jti: 'bbb', label: 'alpha', created_at: 200 }),
        ]}
        canManageTokens={false}
        onRevoke={() => {}}
      />
    );

    const table = screen.getByRole('table');
    const getFirstBodyRow = () => within(table).getAllByRole('row')[1]!;

    // Initial sort is by created_at desc, so bbb (200) comes first.
    expect(within(getFirstBodyRow()).getByText('bbb')).toBeInTheDocument();

    await user.click(screen.getByText(/^label$/i));

    // Now label should sort asc, so alpha (bbb) stays first.
    expect(within(getFirstBodyRow()).getByText('bbb')).toBeInTheDocument();

    await user.click(screen.getByText(/^label$/i));

    // Desc should put zeta (aaa) first.
    expect(within(getFirstBodyRow()).getByText('aaa')).toBeInTheDocument();
  });

  it('updates column widths when dragging a resize handle', () => {
    render(
      <MintedTokensTable
        isLoading={false}
        tokens={[makeToken({ jti: 'aaa', label: 'hello world' })]}
        canManageTokens={false}
        onRevoke={() => {}}
      />
    );

    const table = screen.getByRole('table');
    const headerRow = within(table).getAllByRole('row')[0]!;
    const getJtiHeader = () => within(headerRow).getByText(/^jti$/i).closest('th');
    const jtiHeader = getJtiHeader();
    expect(jtiHeader).toBeTruthy();

    const before = (jtiHeader as HTMLTableCellElement).style.width;
    const handle = within(jtiHeader as HTMLTableCellElement).getByTestId('resize-handle-jti');

    fireEvent.mouseDown(handle, { clientX: 200, button: 0 });
    fireEvent.mouseMove(window, { clientX: 260 });
    fireEvent.mouseUp(window);

    return waitFor(() => {
      const after = (getJtiHeader() as HTMLTableCellElement).style.width;
      expect(after).not.toEqual(before);
    });
  });

  it('shows token status and filters by active only', async () => {
    const user = userEvent.setup();
    const future = Math.floor(Date.now() / 1000) + 3600;
    render(
      <MintedTokensTable
        isLoading={false}
        tokens={[
          makeToken({ jti: 'aaa', label: 'still valid', exp: future }),
          makeToken({ jti: 'bbb', label: 'long gone', exp: 200 }),
          makeToken({ jti: 'ccc', label: 'pulled', exp: future, revoked: true }),
        ]}
        canManageTokens={false}
        onRevoke={() => {}}
      />
    );

    expect(screen.getByText('active')).toBeInTheDocument();
    expect(screen.getByText('expired')).toBeInTheDocument();
    expect(screen.getByText('revoked')).toBeInTheDocument();

    await user.selectOptions(screen.getByDisplayValue(/status: any/i), 'active');

    expect(screen.getByText('aaa')).toBeInTheDocument();
    expect(screen.queryByText('bbb')).not.toBeInTheDocument();
    expect(screen.queryByText('ccc')).not.toBeInTheDocument();
  });

  it('hides revoke button for current token', () => {
    render(
      <MintedTokensTable
        isLoading={false}
        tokens={[makeToken({ jti: 'current-token', label: 'bootstrap', revoked: false })]}
        canManageTokens={true}
        currentJti="current-token"
        onRevoke={() => {}}
      />
    );

    expect(screen.queryByRole('button', { name: /^revoke$/i })).not.toBeInTheDocument();
  });
});
