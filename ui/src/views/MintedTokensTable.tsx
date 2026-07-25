// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { ArrowDown, ArrowUp, ArrowUpDown } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import { Button } from '@/components/ui/Button';
import type { TokenInfo } from '@/services/auth';

import {
  Badge,
  FilterRow,
  HeaderButton,
  HeaderContent,
  JtiCell,
  ResizeHandle,
  Row,
  SearchInput,
  Select,
  SortIcon,
  Subtle,
  Table,
  TableHeaderCell,
  TableWrapper,
} from './TokensView.styles';

function formatUnixSeconds(ts: number): string {
  try {
    return new Date(ts * 1000).toLocaleString(undefined, {
      year: '2-digit',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return String(ts);
  }
}

function formatIsoUtc(ts: number): string {
  try {
    return new Date(ts * 1000).toISOString();
  } catch {
    return String(ts);
  }
}

type TokenStatus = 'active' | 'expired' | 'revoked';

function tokenStatus(token: TokenInfo): TokenStatus {
  if (token.revoked) return 'revoked';
  if (token.exp * 1000 <= Date.now()) return 'expired';
  return 'active';
}

function shortJti(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return trimmed;
  const idx = trimmed.indexOf('-');
  if (idx > 0) return trimmed.slice(0, idx);
  return trimmed.slice(0, 8);
}

type SortDirection = 'asc' | 'desc';
type SortState = { columnId: ColumnId; direction: SortDirection };

type ColumnId = 'jti' | 'token_type' | 'role' | 'label' | 'created_at' | 'exp' | 'status';

type ColumnDef = {
  id: ColumnId;
  label: string;
  sortable: boolean;
  resizable: boolean;
  size: number;
  minSize: number;
  renderCell: (token: TokenInfo) => ReactNode;
  getSortValue: (token: TokenInfo) => string | number | boolean | null;
};

const allRoles = ['(any)', '(none)', 'viewer', 'user', 'admin'] as const;
type RoleFilter = (typeof allRoles)[number];

const allStatuses = ['(any)', 'active', 'expired', 'revoked'] as const;
type StatusFilter = (typeof allStatuses)[number];

export type MintedTokensTableProps = {
  isLoading: boolean;
  tokens: TokenInfo[];
  canManageTokens: boolean;
  currentJti?: string | null;
  onRevoke: (jti: string, label: string | null) => void;
};

export function MintedTokensTable({
  isLoading,
  tokens,
  canManageTokens,
  currentJti,
  onRevoke,
}: MintedTokensTableProps) {
  const [search, setSearch] = useState('');
  const [roleFilter, setRoleFilter] = useState<RoleFilter>('(any)');
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('(any)');
  const [sort, setSort] = useState<SortState>({ columnId: 'created_at', direction: 'desc' });
  const [columnWidths, setColumnWidths] = useState<Record<ColumnId, number>>({
    jti: 110,
    token_type: 80,
    role: 80,
    label: 220,
    created_at: 140,
    exp: 140,
    status: 90,
  });
  const [activeResize, setActiveResize] = useState<{
    columnId: ColumnId;
    startX: number;
    startWidth: number;
  } | null>(null);
  const activeResizeRef = useRef<{
    columnId: ColumnId;
    startX: number;
    startWidth: number;
  } | null>(null);
  const resizeListenersRef = useRef<{
    move: (e: MouseEvent) => void;
    up: (e: MouseEvent) => void;
  } | null>(null);

  const columns = useMemo<ColumnDef[]>(
    () => [
      {
        id: 'jti',
        label: 'JTI',
        sortable: true,
        resizable: true,
        size: 110,
        minSize: 90,
        renderCell: (t) => <JtiCell title={t.jti}>{shortJti(t.jti)}</JtiCell>,
        getSortValue: (t) => t.jti,
      },
      {
        id: 'token_type',
        label: 'Type',
        sortable: true,
        resizable: true,
        size: 80,
        minSize: 70,
        renderCell: (t) => <Badge $variant="neutral">{t.token_type}</Badge>,
        getSortValue: (t) => t.token_type,
      },
      {
        id: 'role',
        label: 'Role',
        sortable: true,
        resizable: true,
        size: 80,
        minSize: 70,
        renderCell: (t) => {
          const role = t.role;
          if (!role) return <span>-</span>;
          const variant = role === 'admin' ? 'warning' : 'neutral';
          return <Badge $variant={variant}>{role}</Badge>;
        },
        getSortValue: (t) => t.role ?? '',
      },
      {
        id: 'label',
        label: 'Label',
        sortable: true,
        resizable: true,
        size: 220,
        minSize: 120,
        renderCell: (t) => t.label ?? '-',
        getSortValue: (t) => t.label ?? '',
      },
      {
        id: 'created_at',
        label: 'Created',
        sortable: true,
        resizable: true,
        size: 140,
        minSize: 120,
        renderCell: (t) => (
          <span title={formatIsoUtc(t.created_at)}>{formatUnixSeconds(t.created_at)}</span>
        ),
        getSortValue: (t) => t.created_at,
      },
      {
        id: 'exp',
        label: 'Expires',
        sortable: true,
        resizable: true,
        size: 140,
        minSize: 120,
        renderCell: (t) => (
          <span
            title={formatIsoUtc(t.exp)}
            style={tokenStatus(t) === 'expired' ? { opacity: 0.6 } : undefined}
          >
            {formatUnixSeconds(t.exp)}
          </span>
        ),
        getSortValue: (t) => t.exp,
      },
      {
        id: 'status',
        label: 'Status',
        sortable: true,
        resizable: true,
        size: 90,
        minSize: 90,
        renderCell: (t) => {
          const status = tokenStatus(t);
          const variant =
            status === 'revoked' ? 'danger' : status === 'expired' ? 'warning' : 'success';
          return <Badge $variant={variant}>{status}</Badge>;
        },
        getSortValue: (t) => tokenStatus(t),
      },
    ],
    []
  );

  const minTableWidth = useMemo(
    () =>
      columns.reduce((total, col) => total + (columnWidths[col.id] ?? col.size), 0) +
      // actions column
      110,
    [columns, columnWidths]
  );

  const totalRowCount = tokens.length;

  const filteredTokens = useMemo(() => {
    const searchValue = search.trim().toLowerCase();
    return tokens.filter((t) => {
      if (roleFilter === '(none)') {
        if (t.role !== null) return false;
      } else if (roleFilter !== '(any)') {
        if (t.role !== roleFilter) return false;
      }

      if (statusFilter !== '(any)' && tokenStatus(t) !== statusFilter) return false;

      if (!searchValue) return true;

      const searchableText = [t.jti, t.label, t.token_type, t.role]
        .filter(Boolean)
        .join(' ')
        .toLowerCase();
      return searchableText.includes(searchValue);
    });
  }, [tokens, search, roleFilter, statusFilter]);

  const sortedTokens = useMemo(() => {
    const column = columns.find((c) => c.id === sort.columnId);
    if (!column) return filteredTokens;

    const directionMultiplier = sort.direction === 'asc' ? 1 : -1;
    const compare = (a: TokenInfo, b: TokenInfo) => {
      const av = column.getSortValue(a);
      const bv = column.getSortValue(b);

      if (typeof av === 'number' && typeof bv === 'number') return (av - bv) * directionMultiplier;
      if (typeof av === 'boolean' && typeof bv === 'boolean')
        return (Number(av) - Number(bv)) * directionMultiplier;
      return (
        String(av ?? '').localeCompare(String(bv ?? ''), undefined, {
          sensitivity: 'base',
          numeric: true,
        }) * directionMultiplier
      );
    };

    return [...filteredTokens].sort(compare);
  }, [columns, filteredTokens, sort.columnId, sort.direction]);

  const onHeaderClick = (columnId: ColumnId, sortable: boolean) => {
    if (!sortable) return;
    setSort((prev) => {
      if (prev.columnId !== columnId) return { columnId, direction: 'asc' };
      return { columnId, direction: prev.direction === 'asc' ? 'desc' : 'asc' };
    });
  };

  useEffect(() => {
    return () => {
      const listeners = resizeListenersRef.current;
      if (!listeners) return;
      window.removeEventListener('mousemove', listeners.move);
      window.removeEventListener('mouseup', listeners.up);
      resizeListenersRef.current = null;
    };
  }, []);

  const beginResize = (column: ColumnDef, e: React.MouseEvent<HTMLDivElement>) => {
    if (!column.resizable) return;
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();

    const next = {
      columnId: column.id,
      startX: e.clientX,
      startWidth: columnWidths[column.id] ?? column.size,
    };

    activeResizeRef.current = next;
    setActiveResize(next);

    const onMove = (ev: MouseEvent) => {
      const current = activeResizeRef.current;
      if (!current) return;
      if (current.columnId !== column.id) return;
      const delta = ev.clientX - current.startX;
      const nextWidth = Math.max(current.startWidth + delta, column.minSize);
      setColumnWidths((prev) => ({ ...prev, [column.id]: nextWidth }));
    };

    const onUp = () => {
      const listeners = resizeListenersRef.current;
      if (listeners) {
        window.removeEventListener('mousemove', listeners.move);
        window.removeEventListener('mouseup', listeners.up);
      }
      resizeListenersRef.current = null;
      activeResizeRef.current = null;
      setActiveResize(null);
    };

    resizeListenersRef.current = { move: onMove, up: onUp };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  if (isLoading) {
    return <Subtle>Loading…</Subtle>;
  }

  return (
    <>
      <FilterRow>
        <SearchInput
          type="text"
          placeholder="Search tokens by JTI, label, type, or role..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <Select value={roleFilter} onChange={(e) => setRoleFilter(e.target.value as RoleFilter)}>
          {allRoles.map((r) => (
            <option key={r} value={r}>
              {r === '(any)' ? 'Role: any' : r === '(none)' ? 'Role: none' : `Role: ${r}`}
            </option>
          ))}
        </Select>
        <Select
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value as StatusFilter)}
        >
          {allStatuses.map((s) => (
            <option key={s} value={s}>
              {s === '(any)' ? 'Status: any' : `Status: ${s}`}
            </option>
          ))}
        </Select>
      </FilterRow>
      <TableWrapper>
        <Table role="table" style={{ width: minTableWidth }}>
          <colgroup>
            {columns.map((column) => (
              <col
                key={column.id}
                style={{ width: `${columnWidths[column.id] ?? column.size}px` }}
              />
            ))}
            <col style={{ width: '110px' }} />
          </colgroup>
          <thead>
            <tr>
              {columns.map((column) => {
                const isSorted = sort.columnId === column.id;
                const icon = !column.sortable ? null : !isSorted ? (
                  <ArrowUpDown size={14} opacity={0.3} />
                ) : sort.direction === 'asc' ? (
                  <ArrowUp size={14} />
                ) : (
                  <ArrowDown size={14} />
                );

                return (
                  <TableHeaderCell
                    key={column.id}
                    $isSortable={column.sortable}
                    style={{ width: `${columnWidths[column.id] ?? column.size}px` }}
                  >
                    <HeaderContent>
                      {column.sortable ? (
                        <HeaderButton type="button" onClick={() => onHeaderClick(column.id, true)}>
                          <span>{column.label}</span>
                          <SortIcon>{icon}</SortIcon>
                        </HeaderButton>
                      ) : (
                        <span>{column.label}</span>
                      )}
                    </HeaderContent>
                    {column.resizable && (
                      <ResizeHandle
                        $isResizing={activeResize?.columnId === column.id}
                        data-testid={`resize-handle-${column.id}`}
                        onMouseDown={(e) => beginResize(column, e)}
                      />
                    )}
                  </TableHeaderCell>
                );
              })}
              <TableHeaderCell style={{ width: '110px' }}>
                <span />
              </TableHeaderCell>
            </tr>
          </thead>
          <tbody>
            {sortedTokens.map((t) => (
              <tr key={t.jti}>
                {columns.map((column) => (
                  <td
                    key={column.id}
                    style={{ width: `${columnWidths[column.id] ?? column.size}px` }}
                  >
                    {column.renderCell(t)}
                  </td>
                ))}
                <td style={{ width: '110px' }}>
                  <Row>
                    {canManageTokens && !t.revoked && t.jti !== currentJti && (
                      <Button
                        variant="danger"
                        size="small"
                        onClick={() => onRevoke(t.jti, t.label)}
                      >
                        Revoke
                      </Button>
                    )}
                  </Row>
                </td>
              </tr>
            ))}
            {sortedTokens.length === 0 && (
              <tr>
                <td colSpan={columns.length + 1}>
                  <Subtle>
                    {search.trim() || roleFilter !== '(any)' || statusFilter !== '(any)'
                      ? 'No tokens match your filters.'
                      : 'No tokens found.'}
                  </Subtle>
                </td>
              </tr>
            )}
          </tbody>
        </Table>
      </TableWrapper>
      {(search.trim() || roleFilter !== '(any)' || statusFilter !== '(any)') &&
        sortedTokens.length !== totalRowCount && (
          <Subtle style={{ marginTop: '8px' }}>
            Showing {sortedTokens.length} of {totalRowCount} tokens
          </Subtle>
        )}
    </>
  );
}
