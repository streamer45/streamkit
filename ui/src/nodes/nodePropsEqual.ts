// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

interface NodeComponentProps {
  id: string;
  type?: string;
  // `any` is intentional: React.memo infers the component's prop types from the
  // comparator's parameter types.  Using `unknown` here would widen `data` to
  // `unknown` inside every node component body, breaking property access.
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
