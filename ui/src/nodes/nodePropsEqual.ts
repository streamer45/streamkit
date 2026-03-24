// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Custom React.memo comparator for ReactFlow node components.
 *
 * ReactFlow passes position/dimension props (`positionAbsoluteX`,
 * `positionAbsoluteY`, `width`, `height`, `dragging`, `zIndex`, …) to
 * every custom node component.  These change frequently during dimension
 * measurement, auto-layout, and fit-view — but the node components never
 * read them.  The default shallow-equality comparison in React.memo sees
 * them as changed and re-renders every node on every layout tick.
 *
 * This comparator only checks the props that node components actually
 * consume: `id`, `type`, `data` (by reference), and `selected`.
 *
 * Usage — cast to the concrete props type at the call-site so that
 * TypeScript doesn't widen `data` to `unknown`:
 *
 * ```ts
 * React.memo(function MyNode({ id, data, selected }: MyNodeProps) {
 *   …
 * }, areNodePropsEqual as (a: Readonly<MyNodeProps>, b: Readonly<MyNodeProps>) => boolean);
 * ```
 */

interface NodeComponentProps {
  id: string;
  type?: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  data: any;
  selected?: boolean;
}

export function areNodePropsEqual(
  prev: Readonly<NodeComponentProps>,
  next: Readonly<NodeComponentProps>
): boolean {
  return (
    prev.id === next.id &&
    prev.type === next.type &&
    prev.data === next.data &&
    prev.selected === next.selected
  );
}
