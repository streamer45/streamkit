// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useEffect, useState, useRef } from 'react';

import { getWebSocketService } from '@/services/websocket';
import type { Request, Response, Event } from '@/types/types';

export function useWebSocket() {
  const [isConnected, setIsConnected] = useState(false);
  const wsService = useRef(getWebSocketService());

  useEffect(() => {
    const ws = wsService.current;

    const unsubscribe = ws.onConnectionStatus((connected) => {
      setIsConnected(connected);
    });

    return () => {
      unsubscribe();
    };
  }, []);

  const send = (request: Request): Promise<Response> => {
    return wsService.current.send(request);
  };

  const onMessage = (handler: (message: Response | Event) => void) => {
    return wsService.current.onMessage(handler);
  };

  return {
    isConnected,
    send,
    onMessage,
  };
}
