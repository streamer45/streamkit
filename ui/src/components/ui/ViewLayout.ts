// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared layout primitives for top-level views (Convert, Stream, etc.).
 *
 * These styled components provide a consistent page structure:
 *   ViewContainer → ContentArea → ContentWrapper → Section(s)
 *
 * Views that need additional, view-specific components can define them
 * locally while importing the common ones from here.
 */

import styled from '@emotion/styled';

/** Full-height flex column with the standard background. */
export const ViewContainer = styled.div`
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--sk-bg);
`;

/** Scrollable centred column that holds the page content. */
export const ContentArea = styled.div`
  flex: 1;
  overflow-y: auto;
  display: flex;
  justify-content: center;
  padding: 40px;
`;

/** Max-width wrapper inside ContentArea. */
export const ContentWrapper = styled.div`
  width: 100%;
  max-width: 1200px;
  display: flex;
  flex-direction: column;
  gap: 32px;
`;

/** Small spacer at the bottom of the content wrapper (fills the gap
 *  between the last section and the edge of the scroll container). */
export const BottomSpacer = styled.div`
  height: 8px;
  flex-shrink: 0;
  /* With gap: 32px from ContentWrapper, this gives us 40px total bottom spacing */
`;

/** A rounded card section with vertical flex layout. */
export const Section = styled.div`
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 24px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
`;

/** Section heading (h2). */
export const SectionTitle = styled.h2`
  font-size: 18px;
  font-weight: 600;
  color: var(--sk-text);
  margin: 0;
`;

/** Accent-bordered info callout used at the top of views. */
export const InfoBox = styled.div`
  padding: 20px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-left: 4px solid var(--sk-primary);
  border-radius: 8px;
  color: var(--sk-text);
  font-size: 14px;
  line-height: 1.6;
  display: flex;
  flex-direction: column;
  gap: 16px;
`;

/** Vertical flex container for paragraphs inside an InfoBox. */
export const InfoContent = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

/** Title inside an InfoBox. */
export const InfoTitle = styled.h2`
  font-size: 18px;
  font-weight: 600;
  color: var(--sk-text);
  margin: 0;
`;

/** Toggle button for collapsible technical-details blocks. */
export const TechnicalDetailsToggle = styled.button`
  padding: 8px 12px;
  background: transparent;
  color: var(--sk-text-muted);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: none;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  align-self: flex-start;

  &:hover {
    background: var(--sk-hover-bg);
    color: var(--sk-text);
    border-color: var(--sk-border-strong);
  }
`;

/** Container for expanded technical-details content. */
export const TechnicalDetails = styled.div`
  padding-top: 12px;
  border-top: 1px solid var(--sk-border);
  color: var(--sk-text-muted);
  font-size: 13px;
  display: flex;
  flex-direction: column;
  gap: 12px;
`;
