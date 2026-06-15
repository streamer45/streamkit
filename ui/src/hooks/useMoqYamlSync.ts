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
 */
export function useMoqYamlSync(
  storeActions: MoqSettingsActions,
  setPipelineYaml: (yaml: string) => void
): {
  deriveMoqFromYaml: (yaml: string) => void;
  handleYamlChange: (yaml: string) => void;
} {
  const timerRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  // `null` is a "never derived" sentinel — it never equals a signature string
  // (e.g. "null" for non-MoQ YAML), so the first debounced edit always derives.
  const lastDerivedClientRef = useRef<string | null>(null);

  const applyFromClient = useCallback(
    (client: ClientSection | null) => {
      if (timerRef.current) clearTimeout(timerRef.current);
      lastDerivedClientRef.current = clientSignature(client);
      applyMoqSettings(
        extractMoqSettingsFromClient(client),
        storeActions,
        useStreamStore.getState().configServerUrl
      );
    },
    [storeActions]
  );

  const deriveMoqFromYaml = useCallback(
    (yaml: string) => applyFromClient(parseClientFromYaml(yaml)),
    [applyFromClient]
  );

  const handleYamlChange = useCallback(
    (yaml: string) => {
      setPipelineYaml(yaml);

      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        const client = parseClientFromYaml(yaml);
        if (clientSignature(client) === lastDerivedClientRef.current) return;
        applyFromClient(client);
      }, MOQ_DERIVE_DEBOUNCE_MS);
    },
    [setPipelineYaml, applyFromClient]
  );

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return { deriveMoqFromYaml, handleYamlChange };
}
