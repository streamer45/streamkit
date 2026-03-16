// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Session selection, auto-select, and deletion logic for the Monitor view.
 *
 * Extracted from MonitorViewContent to reduce the component's statement count
 * and keep session-lifecycle concerns in one place.
 *
 * Auto-layout triggering is communicated via an `onSessionActivated` ref-based
 * callback so that the hook has no direct dependency on `useAutoLayout`.
 */

import React, { useState, useEffect, useCallback, useRef } from 'react';
import { useLocation } from 'react-router-dom';
import { v4 as uuidv4 } from 'uuid';

import { useToast } from '@/context/ToastContext';
import { useSessionList } from '@/hooks/useSessionList';
import { getWebSocketService } from '@/services/websocket';
import type { MessageType } from '@/types/types';
import { viewsLogger } from '@/utils/logger';

/**
 * Callback fired every time a session becomes the active session.
 * Auto-layout always runs on activation (no saved positions without staging store).
 */
export type OnSessionActivated = (sessionId: string, hasPositions: boolean) => void;

interface UseMonitorSessionManagerOptions {
  /** Ref-based callback — the component wires auto-layout through it. */
  onSessionActivatedRef: React.RefObject<OnSessionActivated>;
}

export function useMonitorSessionManager({
  onSessionActivatedRef,
}: UseMonitorSessionManagerOptions) {
  const location = useLocation();
  const toast = useToast();

  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [sessionToDelete, setSessionToDelete] = useState<string | null>(null);
  const [isDeletingSession, setIsDeletingSession] = useState(false);

  // Fetch session list
  const { data: sessions = [], isLoading: isLoadingSessions } = useSessionList();

  // Memoize the selected session to prevent unnecessary re-renders.
  const prevSelectedSessionRef = useRef<
    { id: string; name: string | null; created_at: string } | undefined
  >(undefined);

  const selectedSession = React.useMemo(() => {
    const found = sessions.find((s) => s.id === selectedSessionId);
    const prev = prevSelectedSessionRef.current;

    if (!found && !prev) return undefined;
    if (!found || !prev) {
      prevSelectedSessionRef.current = found;
      return found;
    }

    if (found.id === prev.id && found.name === prev.name && found.created_at === prev.created_at) {
      return prev;
    }

    prevSelectedSessionRef.current = found;
    return found;
  }, [sessions, selectedSessionId]);

  /** Notify the component that a session was activated. */
  const notifyActivated = useCallback(
    (sessionId: string) => {
      // Without staging store, we always trigger auto-layout
      onSessionActivatedRef.current(sessionId, false);
    },
    [onSessionActivatedRef]
  );

  // Auto-select session from navigation state (e.g., from Stream view)
  useEffect(() => {
    const state = location.state as { sessionId?: string } | null;
    if (state?.sessionId && !selectedSessionId) {
      const sessionId = state.sessionId;
      setSelectedSessionId(sessionId);
      notifyActivated(sessionId);
      // Clear the state to avoid auto-selecting on subsequent visits
      window.history.replaceState({}, document.title);
    }
  }, [location.state, selectedSessionId, notifyActivated]);

  // Auto-select the first session when none is selected (e.g., initial load)
  useEffect(() => {
    if (!selectedSessionId && !isLoadingSessions && sessions.length > 0) {
      const sessionId = sessions[0].id;
      setSelectedSessionId(sessionId);
      notifyActivated(sessionId);
    }
  }, [selectedSessionId, isLoadingSessions, sessions, notifyActivated]);

  // When a session is destroyed, eagerly clear the selection.
  const sessionSeenInListRef = useRef(false);
  if (selectedSession) {
    sessionSeenInListRef.current = true;
  }
  useEffect(() => {
    if (
      selectedSessionId &&
      !selectedSession &&
      !isLoadingSessions &&
      sessionSeenInListRef.current
    ) {
      sessionSeenInListRef.current = false;
      setSelectedSessionId(null);
    }
  }, [selectedSessionId, selectedSession, isLoadingSessions]);

  // ── Handlers ──────────────────────────────────────────────────────────

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      React.startTransition(() => {
        setSelectedSessionId(sessionId);
        notifyActivated(sessionId);
      });
    },
    [notifyActivated]
  );

  const handleQuickDeleteSession = useCallback((sessionId: string) => {
    setSessionToDelete(sessionId);
  }, []);

  const handleConfirmQuickDelete = useCallback(async () => {
    if (!sessionToDelete) return;

    setIsDeletingSession(true);
    try {
      const wsService = getWebSocketService();
      const response = await wsService.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'destroysession' as const,
          session_id: sessionToDelete,
        },
      });

      if (response.payload.action === 'sessiondestroyed') {
        toast.success('Session deleted successfully');
        if (selectedSessionId === sessionToDelete) {
          setSelectedSessionId(null);
        }
        setSessionToDelete(null);
      } else if (response.payload.action === 'error') {
        throw new Error(response.payload.message);
      }
    } catch (error) {
      viewsLogger.error('Failed to delete session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to delete session');
    } finally {
      setIsDeletingSession(false);
    }
  }, [sessionToDelete, selectedSessionId, toast]);

  const handleDeleteSession = useCallback(async () => {
    if (!selectedSessionId) return;

    setIsDeletingSession(true);
    try {
      const wsService = getWebSocketService();
      const response = await wsService.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'destroysession' as const,
          session_id: selectedSessionId,
        },
      });

      if (response.payload.action === 'sessiondestroyed') {
        toast.success(`Session ${selectedSessionId} deleted successfully`);
        setSelectedSessionId(null);
        setShowDeleteModal(false);
      } else if (response.payload.action === 'error') {
        throw new Error(response.payload.message);
      }
    } catch (error) {
      viewsLogger.error('Failed to delete session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to delete session');
    } finally {
      setIsDeletingSession(false);
    }
  }, [selectedSessionId, toast]);

  const handleDeleteModalOpen = useCallback(() => {
    setShowDeleteModal(true);
  }, []);

  return {
    selectedSessionId,
    selectedSession,
    sessions,
    isLoadingSessions,
    showDeleteModal,
    setShowDeleteModal,
    sessionToDelete,
    setSessionToDelete,
    isDeletingSession,
    handleSessionClick,
    handleQuickDeleteSession,
    handleConfirmQuickDelete,
    handleDeleteSession,
    handleDeleteModalOpen,
  };
}
