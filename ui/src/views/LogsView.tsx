// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useCallback, useEffect, useRef, useState } from 'react';

import { Button } from '@/components/ui/Button';
import { usePermissions } from '@/hooks/usePermissions';
import { createLogStream, fetchLogs, type LogResponse } from '@/services/logs';
import { getLogger } from '@/utils/logger';

import AdminNav from './admin/AdminNav';
import {
  Card,
  Container,
  ContentArea,
  ContentWrapper,
  EmptyState,
  ErrorBox,
  FilterBar,
  LevelSelect,
  LiveIndicator,
  LogContainer,
  LogLine,
  PaginationInfo,
  PaginationRow,
  SearchInput,
  Subtle,
  Title,
  TitleRow,
} from './LogsView.styles';

const logger = getLogger('LogsView');

const PAGE_SIZE = 500;

function detectLevel(line: string): string | undefined {
  if (/ ERROR /i.test(line) || /"level":"ERROR"/i.test(line)) return 'error';
  if (/ WARN /i.test(line) || /"level":"WARN"/i.test(line)) return 'warn';
  if (/ DEBUG /i.test(line) || /"level":"DEBUG"/i.test(line)) return 'debug';
  if (/ TRACE /i.test(line) || /"level":"TRACE"/i.test(line)) return 'trace';
  return undefined;
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface UseLogViewerResult {
  lines: string[];
  isLoading: boolean;
  error: string | null;
  fileSize: number;
  hasMore: boolean;
  liveTail: boolean;
  filterText: string;
  levelFilter: string;
  logContainerRef: React.RefObject<HTMLDivElement | null>;
  setFilterText: (v: string) => void;
  setLevelFilter: (v: string) => void;
  handleApplyFilters: () => void;
  handleKeyDown: (e: React.KeyboardEvent) => void;
  handleLoadNewer: () => void;
  handleLoadOlder: () => void;
  handleLoadLatest: () => void;
  handleToggleLiveTail: () => void;
  handleScroll: () => void;
  handleLevelChange: (e: React.ChangeEvent<HTMLSelectElement>) => void;
}

function useLogViewer(shouldLoad: boolean): UseLogViewerResult {
  const [lines, setLines] = useState<string[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fileSize, setFileSize] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [nextOffset, setNextOffset] = useState(0);
  const [currentOffset, setCurrentOffset] = useState<number | undefined>(undefined);

  const [filterText, setFilterText] = useState('');
  const [levelFilter, setLevelFilter] = useState('');
  const [appliedFilter, setAppliedFilter] = useState('');
  const [appliedLevel, setAppliedLevel] = useState('');

  const [liveTail, setLiveTail] = useState(false);
  const eventSourceRef = useRef<EventSource | null>(null);
  const logContainerRef = useRef<HTMLDivElement | null>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  const scrollToBottom = useCallback(() => {
    if (logContainerRef.current && autoScroll) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [autoScroll]);

  const loadLogs = useCallback(
    async (direction: 'forward' | 'backward', offset?: number) => {
      setIsLoading(true);
      setError(null);
      try {
        const response: LogResponse = await fetchLogs({
          offset,
          limit: PAGE_SIZE,
          direction,
          filter: appliedFilter || undefined,
          level: appliedLevel || undefined,
        });
        setLines(response.lines);
        setFileSize(response.file_size);
        setHasMore(response.has_more);
        setNextOffset(response.next_offset);
        setCurrentOffset(offset);
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to load logs';
        logger.error('Failed to load logs:', err);
        setError(message);
      } finally {
        setIsLoading(false);
      }
    },
    [appliedFilter, appliedLevel]
  );

  // Load latest logs on mount
  useEffect(() => {
    if (shouldLoad) {
      loadLogs('backward');
    }
  }, [loadLogs, shouldLoad]);

  // Scroll to bottom when lines change
  useEffect(() => {
    scrollToBottom();
  }, [lines, scrollToBottom]);

  // Live tail management
  useEffect(() => {
    if (!liveTail) {
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
        eventSourceRef.current = null;
      }
      return;
    }

    const es = createLogStream({
      filter: appliedFilter || undefined,
      level: appliedLevel || undefined,
    });

    es.onmessage = (event: MessageEvent) => {
      const newLines = (event.data as string).split('\n').filter(Boolean);
      if (newLines.length > 0) {
        setLines((prev) => {
          const combined = [...prev, ...newLines];
          // Keep a reasonable buffer during live tail
          if (combined.length > 5000) {
            return combined.slice(combined.length - 5000);
          }
          return combined;
        });
      }
    };

    es.onerror = () => {
      logger.error('Log stream connection error');
    };

    es.addEventListener('truncated', () => {
      setLines([]);
    });

    eventSourceRef.current = es;

    return () => {
      es.close();
      eventSourceRef.current = null;
    };
  }, [liveTail, appliedFilter, appliedLevel]);

  const handleApplyFilters = useCallback(() => {
    setAppliedFilter(filterText);
    setAppliedLevel(levelFilter);
    setLiveTail(false);
  }, [filterText, levelFilter]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter') {
        handleApplyFilters();
      }
    },
    [handleApplyFilters]
  );

  const handleLoadNewer = useCallback(() => {
    loadLogs('forward', nextOffset);
  }, [loadLogs, nextOffset]);

  const handleLoadOlder = useCallback(() => {
    if (currentOffset !== undefined && currentOffset > 0) {
      loadLogs('backward', currentOffset);
    } else {
      loadLogs('backward');
    }
  }, [loadLogs, currentOffset]);

  const handleLoadLatest = useCallback(() => {
    loadLogs('backward');
  }, [loadLogs]);

  const handleToggleLiveTail = useCallback(() => {
    if (!liveTail) {
      // Starting live tail — first load latest, then enable streaming
      loadLogs('backward').then(() => {
        setLiveTail(true);
        setAutoScroll(true);
      });
    } else {
      setLiveTail(false);
    }
  }, [liveTail, loadLogs]);

  const handleScroll = useCallback(() => {
    const container = logContainerRef.current;
    if (!container) return;
    const isAtBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 50;
    setAutoScroll(isAtBottom);
  }, []);

  const handleLevelChange = useCallback((e: React.ChangeEvent<HTMLSelectElement>) => {
    setLevelFilter(e.target.value);
  }, []);

  return {
    lines,
    isLoading,
    error,
    fileSize,
    hasMore,
    liveTail,
    filterText,
    levelFilter,
    logContainerRef,
    setFilterText,
    setLevelFilter,
    handleApplyFilters,
    handleKeyDown,
    handleLoadNewer,
    handleLoadOlder,
    handleLoadLatest,
    handleToggleLiveTail,
    handleScroll,
    handleLevelChange,
  };
}

const LogsView: React.FC = () => {
  const { role, isAdmin } = usePermissions();
  const admin = isAdmin();
  const lv = useLogViewer(admin);

  if (!admin) {
    return (
      <Container data-testid="logs-view">
        <ContentArea>
          <ContentWrapper>
            <Card>
              <TitleRow>
                <div>
                  <Title>Logs</Title>
                  <Subtle>Role: {role ?? 'unknown'}</Subtle>
                </div>
              </TitleRow>
              <AdminNav />
              <ErrorBox>Admin role required to view logs.</ErrorBox>
            </Card>
          </ContentWrapper>
        </ContentArea>
      </Container>
    );
  }

  return (
    <Container data-testid="logs-view">
      <ContentArea>
        <ContentWrapper>
          <Card>
            <TitleRow>
              <div>
                <Title>Logs</Title>
                <Subtle>
                  Server log viewer{lv.fileSize > 0 && ` \u2022 ${formatFileSize(lv.fileSize)}`}
                </Subtle>
              </div>
              <LiveIndicator $active={lv.liveTail}>{lv.liveTail ? 'Live' : 'Paused'}</LiveIndicator>
            </TitleRow>

            <AdminNav />

            {lv.error && <ErrorBox>{lv.error}</ErrorBox>}

            <FilterBar>
              <SearchInput
                type="text"
                placeholder="Filter logs..."
                value={lv.filterText}
                onChange={(e) => lv.setFilterText(e.target.value)}
                onKeyDown={lv.handleKeyDown}
                data-testid="logs-filter-input"
              />
              <LevelSelect
                value={lv.levelFilter}
                onChange={lv.handleLevelChange}
                data-testid="logs-level-select"
              >
                <option value="">All levels</option>
                <option value="error">Error</option>
                <option value="warn">Warn</option>
                <option value="info">Info</option>
                <option value="debug">Debug</option>
              </LevelSelect>
              <Button
                onClick={lv.handleApplyFilters}
                disabled={lv.isLoading}
                data-testid="logs-apply-filter"
              >
                Filter
              </Button>
              <Button
                onClick={lv.handleToggleLiveTail}
                variant={lv.liveTail ? 'primary' : 'ghost'}
                data-testid="logs-live-tail"
              >
                {lv.liveTail ? 'Stop tail' : 'Live tail'}
              </Button>
            </FilterBar>

            <LogContainer
              ref={lv.logContainerRef}
              onScroll={lv.handleScroll}
              data-testid="logs-container"
            >
              {lv.lines.length === 0 && !lv.isLoading && (
                <EmptyState>No log lines to display.</EmptyState>
              )}
              {lv.lines.map((line, i) => (
                <LogLine key={i} $level={detectLevel(line)}>
                  {line}
                </LogLine>
              ))}
            </LogContainer>

            <PaginationRow>
              <div style={{ display: 'flex', gap: '8px' }}>
                <Button
                  onClick={lv.handleLoadOlder}
                  disabled={lv.isLoading || lv.liveTail}
                  variant="ghost"
                  data-testid="logs-load-older"
                >
                  Older
                </Button>
                <Button
                  onClick={lv.handleLoadNewer}
                  disabled={lv.isLoading || !lv.hasMore || lv.liveTail}
                  variant="ghost"
                  data-testid="logs-load-newer"
                >
                  Newer
                </Button>
                <Button
                  onClick={lv.handleLoadLatest}
                  disabled={lv.isLoading || lv.liveTail}
                  variant="ghost"
                  data-testid="logs-load-latest"
                >
                  Latest
                </Button>
              </div>
              <PaginationInfo>
                {lv.lines.length} lines
                {lv.fileSize > 0 && ` \u2022 ${formatFileSize(lv.fileSize)}`}
              </PaginationInfo>
            </PaginationRow>
          </Card>
        </ContentWrapper>
      </ContentArea>
    </Container>
  );
};

export default LogsView;
