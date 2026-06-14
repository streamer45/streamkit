// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { load } from 'js-yaml';

import type { ClientSection, Pipeline } from '@/types/types';

export function extractClientSection(pipeline: Pipeline | null | undefined): ClientSection | null {
  return pipeline?.client ?? null;
}

export function extractClientFromParsed(
  parsed: Record<string, unknown> | null | undefined
): ClientSection | null {
  if (!parsed || typeof parsed !== 'object') return null;
  return (parsed.client as ClientSection) ?? null;
}

export function parseClientFromYaml(yamlContent: string): ClientSection | null {
  try {
    const parsed = load(yamlContent) as Record<string, unknown> | null;
    return extractClientFromParsed(parsed);
  } catch {
    return null;
  }
}

/**
 * Stable signature of a pipeline YAML's `client` section.
 *
 * Used to decide whether a direct YAML edit changed MoQ transport settings and
 * therefore warrants re-deriving the connection store, so edits to the rest of
 * the pipeline don't stomp values the user is mid-typing.
 */
export function clientSectionSignature(yamlContent: string): string {
  return JSON.stringify(parseClientFromYaml(yamlContent));
}

/** Convert a CSS-style `accept` attribute into lowercase format names; null = all accepted. */
export function parseAcceptToFormats(accept: string | null | undefined): string[] | null {
  if (!accept) return null;
  if (accept.includes('*')) return null;

  const formats: string[] = [];
  for (const part of accept.split(',')) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    if (trimmed.startsWith('.')) {
      formats.push(trimmed.slice(1).toLowerCase());
    } else if (trimmed.includes('/')) {
      const sub = trimmed.split('/')[1]?.toLowerCase();
      if (sub) formats.push(sub);
    } else {
      formats.push(trimmed.toLowerCase());
    }
  }
  return formats.length > 0 ? formats : null;
}
