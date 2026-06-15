// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback, useEffect, useRef } from 'react';

import { useStreamStore } from '@/stores/streamStore';
import type { ClientSection } from '@/types/types';
import { parseClientFromYaml } from '@/utils/clientSection';
import {
  applyMoqSettings,
  extractMoqSettingsFromClient,
  type MoqSettingsActions,
} from '@/utils/moqPeerSettings';

/** Debounce for re-deriving MoQ settings while the user edits the YAML editor. */
const MOQ_DERIVE_DEBOUNCE_MS = 300;

const clientSignature = (client: ClientSection | null): string => JSON.stringify(client);

/**
 * Keep the MoQ connection store in sync with the Stream view's pipeline YAML.
 *
 * Selecting a sample derives MoQ broadcast/transport settings immediately via
 * {@link deriveMoqFromYaml}. Direct edits to the YAML editor re-derive on a
 * debounce, but only when the pipeline's `client` section actually changes — so
 * editing the rest of the pipeline doesn't stomp broadcast names the user is
 * mid-typing, and pasting a different (or non-MoQ) pipeline clears the broadcast
 * names carried over from the previously-selected sample (issue #550).
 *
 * {@link flushPendingDerive} applies an in-flight debounced edit immediately and
 * is a no-op otherwise, so callers can settle the store before reading it (e.g.
 * before auto-connect) without clobbering manual edits to the broadcast/server
 * fields, which write to the store directly and never schedule a debounce.
 */
export function useMoqYamlSync(
  storeActions: MoqSettingsActions,
  setPipelineYaml: (yaml: string) => void
): {
  deriveMoqFromYaml: (yaml: string) => void;
  handleYamlChange: (yaml: string) => void;
  flushPendingDerive: () => void;
} {
  const timerRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  const pendingYamlRef = useRef<string | null>(null);
  // `null` is a "never derived" sentinel — it never equals a signature string
  // (e.g. "null" for non-MoQ YAML), so the first debounced edit always derives.
  const lastDerivedClientRef = useRef<string | null>(null);

  const cancelPending = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = undefined;
    pendingYamlRef.current = null;
  }, []);

  const applyFromClient = useCallback(
    (client: ClientSection | null) => {
      cancelPending();
      lastDerivedClientRef.current = clientSignature(client);
      applyMoqSettings(
        extractMoqSettingsFromClient(client),
        storeActions,
        useStreamStore.getState().configServerUrl
      );
    },
    [storeActions, cancelPending]
  );

  const deriveMoqFromYaml = useCallback(
    (yaml: string) => applyFromClient(parseClientFromYaml(yaml)),
    [applyFromClient]
  );

  const deriveIfClientChanged = useCallback(
    (yaml: string) => {
      const client = parseClientFromYaml(yaml);
      if (clientSignature(client) === lastDerivedClientRef.current) return;
      applyFromClient(client);
    },
    [applyFromClient]
  );

  const handleYamlChange = useCallback(
    (yaml: string) => {
      setPipelineYaml(yaml);

      if (timerRef.current) clearTimeout(timerRef.current);
      pendingYamlRef.current = yaml;
      timerRef.current = setTimeout(() => {
        const pending = pendingYamlRef.current;
        timerRef.current = undefined;
        pendingYamlRef.current = null;
        if (pending !== null) deriveIfClientChanged(pending);
      }, MOQ_DERIVE_DEBOUNCE_MS);
    },
    [setPipelineYaml, deriveIfClientChanged]
  );

  const flushPendingDerive = useCallback(() => {
    if (!timerRef.current) return;
    const pending = pendingYamlRef.current;
    cancelPending();
    if (pending !== null) deriveIfClientChanged(pending);
  }, [cancelPending, deriveIfClientChanged]);

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return { deriveMoqFromYaml, handleYamlChange, flushPendingDerive };
}
