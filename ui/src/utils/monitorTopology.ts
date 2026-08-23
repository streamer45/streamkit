// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

export const buildMonitorTopologyKey = (
  selectedSessionId: string | null,
  topologyFingerprint: string
): string => JSON.stringify([selectedSessionId, topologyFingerprint]);

export type MonitorNodePosition = { x: number; y: number };

export const resolveMonitorNodePosition = (
  nodeName: string,
  reusePreviousPositions: boolean,
  previousPositions: ReadonlyMap<string, MonitorNodePosition>,
  savedPositions: Readonly<Record<string, MonitorNodePosition>>
): MonitorNodePosition =>
  (reusePreviousPositions ? previousPositions.get(nodeName) : undefined) ??
  savedPositions[nodeName] ?? { x: 0, y: 0 };
