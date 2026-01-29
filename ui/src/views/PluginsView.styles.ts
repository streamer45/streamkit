// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';

export const Container = styled.div`
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--sk-bg);
`;

export const ContentArea = styled.div`
  flex: 1;
  overflow-y: auto;
  display: flex;
  justify-content: center;
  min-width: 0;
  min-height: 0;
`;

export const ContentWrapper = styled.div`
  width: 100%;
  max-width: 1200px;
  padding: 40px;
  box-sizing: border-box;

  @media (max-width: 768px) {
    padding: 24px;
  }
`;

export const BottomSpacer = styled.div`
  height: 40px;
  flex-shrink: 0;

  @media (max-width: 768px) {
    height: 24px;
  }
`;

export const Card = styled.div`
  box-sizing: border-box;
  width: 100%;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  min-width: 0;
`;

export const TitleRow = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
`;

export const Title = styled.h1`
  margin: 0;
  font-size: 20px;
  font-weight: 700;
  color: var(--sk-text);
`;

export const Subtle = styled.div`
  color: var(--sk-text-muted);
  font-size: 13px;
`;

export const NoticeBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-primary) 8%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

export const ErrorBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-danger) 10%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

export const Section = styled.section`
  box-sizing: border-box;
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 16px;
  background: var(--sk-bg);
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
`;

export const SectionTitle = styled.h2`
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--sk-text);
`;

export const Row = styled.div`
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
  align-items: center;
`;

export const Label = styled.label`
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 12px;
  color: var(--sk-text-muted);
  min-width: 200px;
  flex: 1 1 240px;
`;

export const Input = styled.input`
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 13px;
`;

export const Select = styled.select`
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 13px;
`;

export const PluginList = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

export const PluginItem = styled.div`
  border: 1px solid var(--sk-border);
  border-radius: 10px;
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  background: var(--sk-panel-bg);
`;

export const PluginHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
`;

export const PluginMeta = styled.div`
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

export const PluginBadge = styled.span<{ $variant?: 'native' | 'wasm' }>`
  background: ${(props) =>
    props.$variant === 'native' ? 'var(--sk-success)' : 'var(--sk-primary)'};
  color: var(--sk-text-white);
  font-size: 10px;
  font-weight: 700;
  padding: 2px 8px;
  border-radius: 999px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
`;

export const EmptyState = styled.div`
  color: var(--sk-text-muted);
  font-size: 12px;
`;

export const MarketplaceGrid = styled.div`
  display: grid;
  grid-template-columns: minmax(240px, 1fr) minmax(320px, 2fr);
  gap: 16px;
  align-items: start;

  @media (max-width: 1000px) {
    grid-template-columns: 1fr;
  }
`;

export const DetailsSection = styled.section`
  box-sizing: border-box;
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 16px;
  background: var(--sk-bg);
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  min-height: 420px;
  position: relative;
`;

export const DetailsLoadingOverlay = styled.div`
  position: absolute;
  inset: 0;
  border-radius: 12px;
  background: color-mix(in srgb, var(--sk-bg) 70%, transparent);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1;
  color: var(--sk-text-muted);
  font-size: 13px;
`;

export const MarketplaceList = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  overflow-y: auto;
`;

export const MarketplaceListItem = styled.button<{ $active?: boolean }>`
  text-align: left;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid ${(props) => (props.$active ? 'var(--sk-primary)' : 'var(--sk-border)')};
  background: ${(props) => (props.$active ? 'var(--sk-primary-alpha)' : 'var(--sk-panel-bg)')};
  color: var(--sk-text);
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 4px;

  &:hover {
    border-color: var(--sk-primary);
  }
`;

export const MarketplaceListTitle = styled.div`
  font-weight: 600;
  font-size: 13px;
  color: var(--sk-text);
`;

export const MarketplaceListDescription = styled.div`
  font-size: 12px;
  color: var(--sk-text-muted);
`;

export const DetailsHeader = styled.div`
  display: flex;
  flex-direction: column;
  gap: 6px;
`;

export const DetailsTitle = styled.div`
  font-size: 16px;
  font-weight: 700;
  color: var(--sk-text);
`;

export const DetailsDescription = styled.div`
  font-size: 13px;
  color: var(--sk-text-muted);
`;

export const KeyValueGrid = styled.div`
  display: grid;
  grid-template-columns: minmax(120px, 200px) 1fr;
  gap: 8px 12px;
  font-size: 13px;
  color: var(--sk-text);
`;

export const KeyLabel = styled.div`
  color: var(--sk-text-muted);
  font-size: 12px;
`;

export const KeyValue = styled.div`
  color: var(--sk-text);
  word-break: break-word;
`;

export const ProgressBar = styled.progress`
  width: 100%;
  height: 8px;
  border-radius: 999px;
  overflow: hidden;

  &::-webkit-progress-bar {
    background: var(--sk-border);
    border-radius: 999px;
  }

  &::-webkit-progress-value {
    background: var(--sk-primary);
    border-radius: 999px;
  }

  &::-moz-progress-bar {
    background: var(--sk-primary);
    border-radius: 999px;
  }
`;

export const StepList = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

export const StepRow = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 10px;
  border-radius: 8px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
`;

export const StepHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
`;

export const StepName = styled.div`
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text);
`;

export const StepStatus = styled.div<{ $status: string }>`
  font-size: 11px;
  font-weight: 600;
  color: ${(props) => {
    switch (props.$status) {
      case 'succeeded':
        return 'var(--sk-success)';
      case 'failed':
        return 'var(--sk-danger)';
      case 'running':
        return 'var(--sk-primary)';
      case 'cancelled':
        return 'var(--sk-text-muted)';
      default:
        return 'var(--sk-text-muted)';
    }
  }};
`;

export const StepMeta = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
`;

export const StepError = styled.div`
  font-size: 11px;
  color: var(--sk-danger);
`;

export const SectionDivider = styled.hr`
  border: none;
  border-top: 1px solid var(--sk-border);
  margin: 4px 0;
`;

export const SubSectionLabel = styled.div`
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
`;

export const SignatureValue = styled.span<{ $verified: boolean }>`
  color: ${(props) => (props.$verified ? 'var(--sk-success)' : 'var(--sk-warning)')};
`;

export const WarningBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-warning) 10%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

export const ModelRow = styled.div`
  display: flex;
  gap: 10px;
  align-items: center;
  padding: 6px 0;
`;

export const ModelName = styled.span`
  font-weight: 500;
  color: var(--sk-text);
  font-size: 13px;
`;

export const ModelMeta = styled.span`
  font-size: 11px;
  color: var(--sk-text-muted);
`;
