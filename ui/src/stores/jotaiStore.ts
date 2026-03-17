// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Vanilla Jotai store for non-React contexts (WebSocket service, etc.).
 *
 * Components should use the default provider-less mode (useAtom / useAtomValue
 * / useSetAtom) which shares this same store under the hood.
 * Non-React code (e.g. WebSocketService) can use `jotaiStore.get(atom)` and
 * `jotaiStore.set(atom, value)` directly.
 */

import { createStore } from 'jotai';

export const jotaiStore = createStore();
