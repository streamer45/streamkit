// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { Button } from '@/components/ui/Button';
import type { JobInfo } from '@/types/marketplace';

import {
  ErrorBox,
  ProgressBar,
  Row,
  Section,
  SectionTitle,
  StepContent,
  StepError,
  StepList,
  StepMeta,
  StepName,
  StepRow,
  StepSpinner,
  StepStatus,
  Subtle,
} from '../PluginsView.styles';
import { formatStepName, formatStepProgress } from './marketplaceFormatters';

type MarketplaceJobPanelProps = {
  jobId: string | null;
  jobInfo: JobInfo | null;
  jobError: string | null;
  jobProgress: number | null;
  jobIsActive: boolean;
  onCancel: () => void;
  onClear: () => void;
};

export const MarketplaceJobPanel: React.FC<MarketplaceJobPanelProps> = ({
  jobId,
  jobInfo,
  jobError,
  jobProgress,
  jobIsActive,
  onCancel,
  onClear,
}) => {
  if (!jobId) return null;

  return (
    <Section>
      <SectionTitle>Install job</SectionTitle>
      {jobError && <ErrorBox>{jobError}</ErrorBox>}
      {jobInfo && (
        <>
          <Subtle>{jobInfo.summary}</Subtle>
          {jobProgress !== null && <ProgressBar value={jobProgress * 100} max={100} />}
          <StepList>
            {jobInfo.steps.map((step) => {
              const progress = formatStepProgress(step);
              return (
                <StepRow key={step.name}>
                  <StepContent>
                    <StepName>{formatStepName(step.name)}</StepName>
                    {progress && <StepMeta>{progress}</StepMeta>}
                    {step.error && <StepError>{step.error}</StepError>}
                  </StepContent>
                  <StepStatus $status={step.status}>
                    {step.status}
                    {step.status === 'running' && <StepSpinner />}
                  </StepStatus>
                </StepRow>
              );
            })}
          </StepList>
          <Row>
            {jobIsActive ? (
              <Button variant="ghost" onClick={onCancel}>
                Cancel
              </Button>
            ) : (
              <Button variant="ghost" onClick={onClear}>
                Dismiss
              </Button>
            )}
          </Row>
        </>
      )}
    </Section>
  );
};
