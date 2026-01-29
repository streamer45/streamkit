// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { NavLink } from 'react-router-dom';

export const AdminNavBar = styled.nav`
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  padding-bottom: 12px;
  border-bottom: 1px solid var(--sk-border);
  margin-bottom: 16px;
`;

export const AdminNavLink = styled(NavLink)`
  padding: 6px 12px;
  border-radius: 999px;
  border: 1px solid transparent;
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text);
  text-decoration: none;
  background: var(--sk-panel-bg);

  &:hover {
    border-color: var(--sk-primary);
  }

  &.active {
    border-color: var(--sk-primary);
    background: var(--sk-primary-alpha);
  }
`;
