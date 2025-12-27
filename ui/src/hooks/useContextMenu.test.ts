// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { renderHook, act } from '@testing-library/react';
import type { Node } from '@xyflow/react';
import { describe, it, expect, beforeEach } from 'vitest';

import { useContextMenu } from './useContextMenu';

describe('useContextMenu', () => {
  // Extracted constant to avoid duplication (sonarjs/no-duplicate-string)
  const TEST_NODE_ID = 'test-node-1';

  beforeEach(() => {
    // Reset any state between tests
  });

  it('should initialize with no menus open', () => {
    const { result } = renderHook(() => useContextMenu());

    expect(result.current.menu).toBeNull();
    expect(result.current.paneMenu).toBeNull();
    expect(result.current.reactFlowWrapper.current).toBeNull();
  });

  it('should open node context menu and position it correctly', () => {
    const { result } = renderHook(() => useContextMenu());

    const mockNode: Node = {
      id: TEST_NODE_ID,
      position: { x: 100, y: 100 },
      data: {},
    };

    const mockEvent = {
      preventDefault: () => {},
      clientX: 200,
      clientY: 150,
    } as React.MouseEvent;

    act(() => {
      result.current.onNodeContextMenu(mockEvent, mockNode);
    });

    expect(result.current.menu).toEqual({
      id: TEST_NODE_ID,
      top: 150, // clientY
      left: 200,
      right: false,
      bottom: false,
    });
    expect(result.current.paneMenu).toBeNull();
  });

  it('should close pane menu when opening node context menu', () => {
    const { result } = renderHook(() => useContextMenu());

    // First open pane menu
    const paneEvent = {
      preventDefault: () => {},
      clientX: 100,
      clientY: 100,
    } as React.MouseEvent;

    act(() => {
      result.current.onPaneContextMenu(paneEvent);
    });

    expect(result.current.paneMenu).toBeDefined();

    // Then open node menu
    const mockNode: Node = {
      id: TEST_NODE_ID,
      position: { x: 100, y: 100 },
      data: {},
    };

    const nodeEvent = {
      preventDefault: () => {},
      clientX: 200,
      clientY: 150,
    } as React.MouseEvent;

    act(() => {
      result.current.onNodeContextMenu(nodeEvent, mockNode);
    });

    expect(result.current.menu).toBeDefined();
    expect(result.current.paneMenu).toBeNull();
  });

  it('should open pane context menu and position it correctly', () => {
    const { result } = renderHook(() => useContextMenu());

    const mockEvent = {
      preventDefault: () => {},
      clientX: 300,
      clientY: 250,
    } as React.MouseEvent;

    act(() => {
      result.current.onPaneContextMenu(mockEvent);
    });

    expect(result.current.paneMenu).toEqual({
      top: 250, // clientY
      left: 300,
      right: false,
      bottom: false,
    });
    expect(result.current.menu).toBeNull();
  });

  it('should close node menu when opening pane context menu', () => {
    const { result } = renderHook(() => useContextMenu());

    // First open node menu
    const mockNode: Node = {
      id: TEST_NODE_ID,
      position: { x: 100, y: 100 },
      data: {},
    };

    const nodeEvent = {
      preventDefault: () => {},
      clientX: 200,
      clientY: 150,
    } as React.MouseEvent;

    act(() => {
      result.current.onNodeContextMenu(nodeEvent, mockNode);
    });

    expect(result.current.menu).toBeDefined();

    // Then open pane menu
    const paneEvent = {
      preventDefault: () => {},
      clientX: 300,
      clientY: 250,
    } as React.MouseEvent;

    act(() => {
      result.current.onPaneContextMenu(paneEvent);
    });

    expect(result.current.menu).toBeNull();
    expect(result.current.paneMenu).toBeDefined();
  });

  it('should close all menus on pane click', () => {
    const { result } = renderHook(() => useContextMenu());

    // Open node menu
    const mockNode: Node = {
      id: TEST_NODE_ID,
      position: { x: 100, y: 100 },
      data: {},
    };

    const nodeEvent = {
      preventDefault: () => {},
      clientX: 200,
      clientY: 150,
    } as React.MouseEvent;

    act(() => {
      result.current.onNodeContextMenu(nodeEvent, mockNode);
    });

    expect(result.current.menu).toBeDefined();

    // Click pane to close
    act(() => {
      result.current.onPaneClick();
    });

    expect(result.current.menu).toBeNull();
    expect(result.current.paneMenu).toBeNull();
  });

  it('should close pane menu on pane click', () => {
    const { result } = renderHook(() => useContextMenu());

    // Open pane menu
    const paneEvent = {
      preventDefault: () => {},
      clientX: 300,
      clientY: 250,
    } as React.MouseEvent;

    act(() => {
      result.current.onPaneContextMenu(paneEvent);
    });

    expect(result.current.paneMenu).toBeDefined();

    // Click pane to close
    act(() => {
      result.current.onPaneClick();
    });

    expect(result.current.menu).toBeNull();
    expect(result.current.paneMenu).toBeNull();
  });

  it('should handle multiple node context menu opens', () => {
    const { result } = renderHook(() => useContextMenu());

    const mockNode1: Node = {
      id: TEST_NODE_ID,
      position: { x: 100, y: 100 },
      data: {},
    };

    const mockNode2: Node = {
      id: 'test-node-2',
      position: { x: 200, y: 200 },
      data: {},
    };

    const event1 = {
      preventDefault: () => {},
      clientX: 150,
      clientY: 100,
    } as React.MouseEvent;

    act(() => {
      result.current.onNodeContextMenu(event1, mockNode1);
    });

    expect(result.current.menu?.id).toBe(TEST_NODE_ID);

    const event2 = {
      preventDefault: () => {},
      clientX: 250,
      clientY: 200,
    } as React.MouseEvent;

    act(() => {
      result.current.onNodeContextMenu(event2, mockNode2);
    });

    expect(result.current.menu?.id).toBe('test-node-2');
    expect(result.current.menu?.left).toBe(250);
    expect(result.current.menu?.top).toBe(200); // clientY
  });
});
