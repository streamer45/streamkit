// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Local slider state that syncs from a prop value when not actively dragging.
 *
 * Used by compositor controls (opacity, rotation) that use the zero-render
 * update path: the parent updates refs + DOM directly during drags, so React
 * state (and thus the prop) stays stale.  This hook maintains a local value
 * that the slider can use as its controlled `value`, while still syncing
 * from the prop when the drag ends.
 */

import { useState, useEffect, useRef } from 'react';

export function useDragLocalValue(propValue: number) {
  const [localValue, setLocalValue] = useState(propValue);
  const draggingRef = useRef(false);
  useEffect(() => {
    if (!draggingRef.current) setLocalValue(propValue);
  }, [propValue]);
  return { localValue, setLocalValue, draggingRef };
}
