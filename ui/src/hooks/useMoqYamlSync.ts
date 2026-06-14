// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback, useEffect, useRef } from 'react';

import { useStreamStore } from '@/stores/streamStore';
import { clientSectionSignature } from '@/utils/clientSection';
import {
  applyMoqSettings,
  extractMoqPeerSettings,
  type MoqSettingsActions,
} from '@/utils/moqPeerSettings';

/** Debounce for re-deriving MoQ settings while the user edits the YAML editor. */
const MOQ_DERIVE_DEBOUNCE_MS = 300;

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

  const deriveMoqFromYaml = useCallback(
    (yaml: string) => {
      if (timerRef.current) clearTimeout(timerRef.current);
      lastDerivedClientRef.current = clientSectionSignature(yaml);
      applyMoqSettings(
        extractMoqPeerSettings(yaml),
        storeActions,
        useStreamStore.getState().configServerUrl
      );
    },
    [storeActions]
  );

  const handleYamlChange = useCallback(
    (yaml: string) => {
      setPipelineYaml(yaml);

      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        if (clientSectionSignature(yaml) === lastDerivedClientRef.current) return;
        deriveMoqFromYaml(yaml);
      }, MOQ_DERIVE_DEBOUNCE_MS);
    },
    [setPipelineYaml, deriveMoqFromYaml]
  );

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return { deriveMoqFromYaml, handleYamlChange };
}
