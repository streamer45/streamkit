// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { startTransition, useCallback, useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';

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
  CopyToast,
  EmptyState,
  ErrorBox,
  FilterBar,
  LevelSelect,
  LiveIndicator,
  LogContainer,
  LogLine,
  PageSizeSelect,
  PaginationInfo,
  PaginationRow,
  SearchInput,
  Subtle,
  Title,
  TitleRow,
} from './LogsView.styles';

const logger = getLogger('LogsView');

const DEFAULT_PAGE_SIZE = 500;

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

function useDebouncedValue(value: string, delayMs: number): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return debounced;
}

function useLiveTail(
  debouncedFilter: string,
  levelFilter: string,
  setLines: React.Dispatch<React.SetStateAction<string[]>>
) {
  const [liveTail, setLiveTail] = useState(false);
  const eventSourceRef = useRef<EventSource | null>(null);

  useEffect(() => {
    if (!liveTail) {
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
        eventSourceRef.current = null;
      }
      return;
    }

    const es = createLogStream({
      filter: debouncedFilter || undefined,
      level: levelFilter || undefined,
    });

    es.onmessage = (event: MessageEvent) => {
      const newLines = (event.data as string).split('\n').filter(Boolean);
      if (newLines.length > 0) {
        setLines((prev) => {
          const combined = [...prev, ...newLines];
          return combined.length > 5000 ? combined.slice(combined.length - 5000) : combined;
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
  }, [liveTail, debouncedFilter, levelFilter, setLines]);

  return { liveTail, setLiveTail };
}

interface UseLogViewerResult {
  lines: string[];
  isLoading: boolean;
  error: string | null;
  fileSize: number;
  canGoOlder: boolean;
  canGoNewer: boolean;
  liveTail: boolean;
  filterText: string;
  levelFilter: string;
  wrapLines: boolean;
  pageSize: number;
  expanded: boolean;
  copyToastVisible: boolean;
  setFilterText: (v: string) => void;
  handleLoadNewer: () => void;
  handleLoadOlder: () => void;
  handleLoadLatest: () => void;
  handleToggleLiveTail: () => void;
  handleToggleWrap: () => void;
  handleToggleExpand: () => void;
  handleCopyLine: (line: string) => void;
  handlePageSizeChange: (e: React.ChangeEvent<HTMLSelectElement>) => void;
  handleScroll: () => void;
  handleLevelChange: (e: React.ChangeEvent<HTMLSelectElement>) => void;
}

function useLogViewer(
  shouldLoad: boolean,
  logContainerRef: React.RefObject<HTMLDivElement | null>
): UseLogViewerResult {
  const [lines, setLines] = useState<string[]>([]);
  const [isLoading, setIsLoading] = useState(shouldLoad);
  const [error, setError] = useState<string | null>(null);
  const [fileSize, setFileSize] = useState(0);
  const [backwardOffset, setBackwardOffset] = useState(0);
  const [forwardOffset, setForwardOffset] = useState(0);
  const [isAtLatest, setIsAtLatest] = useState(true);
  const [filterText, setFilterText] = useState('');
  const [levelFilter, setLevelFilter] = useState('');
  const [wrapLines, setWrapLines] = useState(true);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [autoScroll, setAutoScroll] = useState(true);
  const [expanded, setExpanded] = useState(false);
  const [copyToastVisible, setCopyToastVisible] = useState(false);
  const copyTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const debouncedFilter = useDebouncedValue(filterText, 300);
  const { liveTail, setLiveTail } = useLiveTail(debouncedFilter, levelFilter, setLines);

  const scrollToBottom = useCallback(() => {
    if (logContainerRef.current && autoScroll) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [autoScroll, logContainerRef]);

  const loadLogs = useCallback(
    async (direction: 'forward' | 'backward', offset?: number) => {
      setIsLoading(true);
      setError(null);
      // Only the await lives in the try: the React Compiler cannot optimize
      // hooks containing value blocks (ternary/logical) inside try/catch.
      const filter = debouncedFilter || undefined;
      const level = levelFilter || undefined;
      let response: LogResponse;
      try {
        response = await fetchLogs({ offset, limit: pageSize, direction, filter, level });
      } catch (err) {
        let message = 'Failed to load logs';
        if (err instanceof Error) {
          message = err.message;
        }
        logger.error('Failed to load logs:', err);
        setError(message);
        setIsLoading(false);
        return;
      }
      setLines(response.lines);
      setFileSize(response.file_size);

      if (direction === 'backward') {
        setBackwardOffset(response.next_offset);
        setForwardOffset(offset ?? response.file_size);
        setIsAtLatest(offset === undefined || offset >= response.file_size);
      } else {
        setForwardOffset(response.next_offset);
        setBackwardOffset(offset ?? 0);
        setIsAtLatest(!response.has_more);
      }
      setIsLoading(false);
    },
    [debouncedFilter, levelFilter, pageSize]
  );

  useEffect(() => {
    if (shouldLoad) {
      startTransition(() => {
        loadLogs('backward');
      });
    }
  }, [loadLogs, shouldLoad]);

  useEffect(() => {
    scrollToBottom();
  }, [lines, scrollToBottom]);

  return {
    lines,
    isLoading,
    error,
    fileSize,
    canGoOlder: backwardOffset > 0,
    canGoNewer: !isAtLatest,
    liveTail,
    filterText,
    levelFilter,
    wrapLines,
    pageSize,
    expanded,
    copyToastVisible,
    setFilterText,
    handleLoadNewer: useCallback(() => {
      if (forwardOffset < fileSize) loadLogs('forward', forwardOffset);
    }, [loadLogs, forwardOffset, fileSize]),
    handleLoadOlder: useCallback(() => {
      if (backwardOffset > 0) loadLogs('backward', backwardOffset);
    }, [loadLogs, backwardOffset]),
    handleLoadLatest: useCallback(() => loadLogs('backward'), [loadLogs]),
    handleToggleLiveTail: useCallback(() => {
      if (!liveTail) {
        loadLogs('backward').then(() => {
          setLiveTail(true);
          setAutoScroll(true);
        });
      } else {
        setLiveTail(false);
      }
    }, [liveTail, loadLogs, setLiveTail]),
    handleToggleWrap: useCallback(() => setWrapLines((prev) => !prev), []),
    handleToggleExpand: useCallback(() => setExpanded((prev) => !prev), []),
    handleCopyLine: useCallback((line: string) => {
      navigator.clipboard
        .writeText(line)
        .then(() => {
          if (copyTimeoutRef.current) clearTimeout(copyTimeoutRef.current);
          flushSync(() => setCopyToastVisible(true));
          copyTimeoutRef.current = setTimeout(() => setCopyToastVisible(false), 1500);
        })
        .catch((err) => {
          logger.error('Failed to copy log line:', err);
        });
    }, []),
    handlePageSizeChange: useCallback(
      (e: React.ChangeEvent<HTMLSelectElement>) => setPageSize(Number(e.target.value)),
      []
    ),
    handleScroll: useCallback(() => {
      const container = logContainerRef.current;
      if (!container) return;
      const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 50;
      setAutoScroll(nearBottom);
    }, [logContainerRef]),
    handleLevelChange: useCallback(
      (e: React.ChangeEvent<HTMLSelectElement>) => setLevelFilter(e.target.value),
      []
    ),
  };
}

const LogsToolbar: React.FC<{ lv: UseLogViewerResult }> = ({ lv }) => (
  <FilterBar>
    <SearchInput
      type="text"
      placeholder="Filter logs..."
      value={lv.filterText}
      onChange={(e) => lv.setFilterText(e.target.value)}
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
    <PageSizeSelect
      value={lv.pageSize}
      onChange={lv.handlePageSizeChange}
      data-testid="logs-page-size"
    >
      <option value={100}>100 lines</option>
      <option value={250}>250 lines</option>
      <option value={500}>500 lines</option>
      <option value={1000}>1000 lines</option>
      <option value={2000}>2000 lines</option>
    </PageSizeSelect>
    <Button onClick={lv.handleToggleWrap} variant="ghost" data-testid="logs-wrap-toggle">
      {lv.wrapLines ? 'No wrap' : 'Wrap'}
    </Button>
    <Button onClick={lv.handleToggleExpand} variant="ghost" data-testid="logs-expand-toggle">
      {lv.expanded ? 'Collapse' : 'Expand'}
    </Button>
    <Button
      onClick={lv.handleToggleLiveTail}
      variant={lv.liveTail ? 'primary' : 'ghost'}
      data-testid="logs-live-tail"
    >
      {lv.liveTail ? 'Stop tail' : 'Live tail'}
    </Button>
  </FilterBar>
);

const LogsPagination: React.FC<{ lv: UseLogViewerResult }> = ({ lv }) => (
  <PaginationRow>
    <div style={{ display: 'flex', gap: '8px' }}>
      <Button
        onClick={lv.handleLoadOlder}
        disabled={lv.isLoading || lv.liveTail || !lv.canGoOlder}
        variant="ghost"
        data-testid="logs-load-older"
      >
        Older
      </Button>
      <Button
        onClick={lv.handleLoadNewer}
        disabled={lv.isLoading || lv.liveTail || !lv.canGoNewer}
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
);

const LogsView: React.FC = () => {
  const { role, isAdmin } = usePermissions();
  const admin = isAdmin();
  const logContainerRef = useRef<HTMLDivElement | null>(null);
  const lv = useLogViewer(admin, logContainerRef);

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
        <ContentWrapper $expanded={lv.expanded}>
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

            <LogsToolbar lv={lv} />

            <LogContainer
              ref={logContainerRef}
              onScroll={lv.handleScroll}
              $wrap={lv.wrapLines}
              data-testid="logs-container"
            >
              {lv.lines.length === 0 && !lv.isLoading && (
                <EmptyState>No log lines to display.</EmptyState>
              )}
              {lv.lines.map((line, i) => (
                <LogLine
                  key={`${i}:${line}`}
                  $level={detectLevel(line)}
                  onClick={() => lv.handleCopyLine(line)}
                  title="Click to copy"
                >
                  {line}
                </LogLine>
              ))}
            </LogContainer>

            <LogsPagination lv={lv} />
          </Card>
        </ContentWrapper>
      </ContentArea>
      <CopyToast $visible={lv.copyToastVisible}>Copied to clipboard</CopyToast>
    </Container>
  );
};

export default LogsView;
