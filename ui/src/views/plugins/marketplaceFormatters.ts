// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { JobInfo, JobStep } from '@/types/marketplace';

export const formatBytes = (bytes?: number | null): string => {
  if (bytes === undefined || bytes === null) return '';
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value < 10 && unitIndex > 0 ? 1 : 0)} ${units[unitIndex]}`;
};

export const formatStepName = (name: string): string => name.replace(/_/g, ' ');

export const formatStepProgress = (step: JobStep): string | null => {
  if (!step.progress) return null;
  const parts: string[] = [];

  if (step.progress.current_item) {
    parts.push(step.progress.current_item);
  }

  if (step.progress.bytes_done !== undefined) {
    const total = step.progress.bytes_total ? ` / ${formatBytes(step.progress.bytes_total)}` : '';
    parts.push(`${formatBytes(step.progress.bytes_done)}${total}`);
  }

  if (step.progress.items_done !== undefined) {
    const total = step.progress.items_total ? ` / ${step.progress.items_total}` : '';
    parts.push(`${step.progress.items_done}${total} items`);
  }

  if (step.progress.rate_bytes_per_sec) {
    parts.push(`${formatBytes(step.progress.rate_bytes_per_sec)}/s`);
  }

  return parts.length > 0 ? parts.join(' • ') : null;
};

export const computeJobProgress = (jobInfo?: JobInfo | null): number | null => {
  if (!jobInfo) return null;
  const totalSteps = jobInfo.steps.length || 1;
  const completed = jobInfo.steps.filter((step) => step.status === 'succeeded').length;
  let progress = completed / totalSteps;
  const running = jobInfo.steps.find((step) => step.status === 'running');
  if (
    running?.progress?.bytes_done !== undefined &&
    running.progress.bytes_total &&
    running.progress.bytes_total > 0
  ) {
    progress += running.progress.bytes_done / running.progress.bytes_total / totalSteps;
  }
  return Math.min(1, progress);
};
