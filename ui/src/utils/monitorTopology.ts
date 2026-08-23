// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

export const buildMonitorTopologyKey = (
  selectedSessionId: string | null,
  topologyFingerprint: string
): string => JSON.stringify([selectedSessionId, topologyFingerprint]);
