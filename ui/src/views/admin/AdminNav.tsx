// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { AdminNavBar, AdminNavLink } from './AdminNav.styles';

const AdminNav: React.FC = () => {
  return (
    <AdminNavBar>
      <AdminNavLink to="/admin/plugins" className={({ isActive }) => (isActive ? 'active' : '')}>
        Plugins
      </AdminNavLink>
      <AdminNavLink to="/admin/tokens" className={({ isActive }) => (isActive ? 'active' : '')}>
        Tokens
      </AdminNavLink>
      <AdminNavLink to="/admin/logs" className={({ isActive }) => (isActive ? 'active' : '')}>
        Logs
      </AdminNavLink>
    </AdminNavBar>
  );
};

export default AdminNav;
