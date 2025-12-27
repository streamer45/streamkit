// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { Menu, MenuItem } from './Menu';

interface PaneContextMenuProps {
  top?: number | false;
  left?: number | false;
  right?: number | false;
  bottom?: number | false;
  onImportYaml?: () => void; // Changed: now just triggers the file dialog
  onExportYaml?: () => void;
  onAutoLayout?: () => void;
  onSaveFragment?: () => void;
  hasSelectedNodes?: boolean;
  onClick: () => void; // close menu
}

const PaneContextMenu: React.FC<PaneContextMenuProps> = ({
  onImportYaml,
  onExportYaml,
  onAutoLayout,
  onSaveFragment,
  hasSelectedNodes,
  onClick,
  ...props
}) => {
  const handleImportClick = () => {
    // Close menu first, then trigger the import flow
    onClick();
    // Small delay to ensure menu is closed before opening file dialog
    setTimeout(() => {
      onImportYaml?.();
    }, 0);
  };

  return (
    <Menu onClose={onClick} {...props}>
      {onAutoLayout && (
        <MenuItem
          onClick={() => {
            onAutoLayout();
            onClick();
          }}
        >
          Auto Layout
        </MenuItem>
      )}
      {onSaveFragment && hasSelectedNodes && (
        <MenuItem
          onClick={() => {
            onSaveFragment();
            onClick();
          }}
        >
          Save as Fragment…
        </MenuItem>
      )}
      {onImportYaml && <MenuItem onClick={handleImportClick}>Import YAML…</MenuItem>}
      {onExportYaml && (
        <MenuItem
          onClick={() => {
            onExportYaml();
            onClick();
          }}
        >
          Export YAML…
        </MenuItem>
      )}
    </Menu>
  );
};

export default PaneContextMenu;
