// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Node } from '@xyflow/react';
import { useState, useCallback, useRef } from 'react';

export function useContextMenu() {
  const [menu, setMenu] = useState<{
    id: string;
    top?: number | false;
    left?: number | false;
    right?: number | false;
    bottom?: number | false;
  } | null>(null);

  const [paneMenu, setPaneMenu] = useState<{
    top?: number | false;
    left?: number | false;
    right?: number | false;
    bottom?: number | false;
  } | null>(null);

  const reactFlowWrapper = useRef<HTMLDivElement>(null);

  const onNodeContextMenu = useCallback((event: React.MouseEvent, node: Node) => {
    event.preventDefault();
    setPaneMenu(null);
    setMenu({
      id: node.id,
      top: event.clientY,
      left: event.clientX,
      right: false,
      bottom: false,
    });
  }, []);

  const onPaneContextMenu = useCallback((event: React.MouseEvent | MouseEvent) => {
    event.preventDefault();
    setMenu(null);
    setPaneMenu({
      top: event.clientY,
      left: event.clientX,
      right: false,
      bottom: false,
    });
  }, []);

  const onPaneClick = useCallback(() => {
    setMenu(null);
    setPaneMenu(null);
  }, []);

  return {
    menu,
    paneMenu,
    reactFlowWrapper,
    onNodeContextMenu,
    onPaneContextMenu,
    onPaneClick,
  };
}
