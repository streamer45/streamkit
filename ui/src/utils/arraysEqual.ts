// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

export function arraysEqual<T>(a: readonly T[], b: readonly T[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}
