// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { Upload } from 'lucide-react';
import React, { useCallback, useEffect, useState } from 'react';

import ConfirmModal from '@/components/ConfirmModal';
import { Button } from '@/components/ui/Button';
import { UploadDropZone } from '@/components/UploadDropZone';
import { useToast } from '@/context/ToastContext';
import { usePermissions } from '@/hooks/usePermissions';
import { deletePlugin, uploadPlugin } from '@/services/plugins';
import { ensurePluginsLoaded, reloadPlugins, usePluginStore } from '@/stores/pluginStore';
import type { PluginSummary } from '@/types/types';
import { getLogger } from '@/utils/logger';

import {
  EmptyState,
  ErrorBox,
  NoticeBox,
  PluginBadge,
  PluginHeader,
  PluginItem,
  PluginList,
  PluginMeta,
  Row,
  Section,
  SectionTitle,
} from '../PluginsView.styles';

const logger = getLogger('PluginsInstalledTab');

const InstalledPluginsTab: React.FC = () => {
  const { can } = usePermissions();
  const toast = useToast();
  const plugins = usePluginStore((s) => s.plugins);
  const upsertPlugin = usePluginStore((s) => s.upsertPlugin);
  const removePlugin = usePluginStore((s) => s.removePlugin);

  const [installedError, setInstalledError] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<PluginSummary | null>(null);
  const [deletingKind, setDeletingKind] = useState<string | null>(null);
  const [isUploading, setIsUploading] = useState(false);

  useEffect(() => {
    ensurePluginsLoaded().catch((err) => {
      logger.error('Failed to load plugins', err);
      setInstalledError('Failed to load plugins.');
    });
  }, []);

  const handleRefreshInstalled = useCallback(async () => {
    setInstalledError(null);
    try {
      await reloadPlugins();
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to refresh plugins.';
      setInstalledError(message);
    }
  }, []);

  const handlePluginFilesSelected = useCallback(
    async (files: FileList) => {
      if (!can.loadPlugin) return;
      const file = files.item(0);
      if (!file) return;
      setIsUploading(true);
      try {
        const summary = await uploadPlugin(file);
        upsertPlugin(summary);
        toast.success(`Uploaded ${summary.kind}`);
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to upload plugin.';
        toast.error(message);
      }
      setIsUploading(false);
    },
    [can.loadPlugin, upsertPlugin, toast]
  );

  const handleConfirmDelete = useCallback(async () => {
    if (!pendingDelete) return;
    setDeletingKind(pendingDelete.kind);
    try {
      await deletePlugin(pendingDelete.kind);
      removePlugin(pendingDelete.kind);
      toast.success(`Unloaded ${pendingDelete.original_kind}`);
      setPendingDelete(null);
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to unload plugin.';
      toast.error(message);
    }
    setDeletingKind(null);
  }, [pendingDelete, removePlugin, toast]);

  return (
    <>
      <Section>
        <SectionTitle>Installed plugins</SectionTitle>
        {installedError && <ErrorBox>{installedError}</ErrorBox>}
        <Row>
          <Button variant="ghost" onClick={handleRefreshInstalled}>
            Refresh
          </Button>
        </Row>
        {plugins.length === 0 ? (
          <EmptyState>No plugins loaded yet.</EmptyState>
        ) : (
          <PluginList>
            {plugins.map((plugin) => {
              const loadedAt = new Date(plugin.loaded_at_ms).toLocaleString();
              return (
                <PluginItem key={plugin.kind}>
                  <PluginHeader>
                    <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                      <span className="code-font" style={{ fontWeight: 600 }}>
                        {plugin.kind}
                      </span>
                      <PluginBadge $variant={plugin.plugin_type}>{plugin.plugin_type}</PluginBadge>
                    </div>
                    <Button
                      variant="danger"
                      size="small"
                      onClick={() => setPendingDelete(plugin)}
                      disabled={!can.deletePlugin || deletingKind === plugin.kind}
                    >
                      {deletingKind === plugin.kind ? 'Removing…' : 'Unload'}
                    </Button>
                  </PluginHeader>
                  <PluginMeta>
                    {plugin.version && <span>Version: {plugin.version}</span>}
                    <span>Original kind: {plugin.original_kind}</span>
                    <span>File: {plugin.file_name}</span>
                    <span>Loaded: {loadedAt}</span>
                  </PluginMeta>
                </PluginItem>
              );
            })}
          </PluginList>
        )}
      </Section>

      <Section>
        <SectionTitle>Manual upload</SectionTitle>
        <NoticeBox>
          Manual uploads are trusted code execution. Prefer marketplace installs when possible.
        </NoticeBox>
        <UploadDropZone
          accept=".wasm,.so,.dylib,.dll"
          disabled={!can.loadPlugin || isUploading}
          icon={<Upload size={24} />}
          text={isUploading ? 'Uploading…' : 'Drop plugin file here or click to browse'}
          hint="Accepted: WASM (.wasm) or native (.so, .dylib, .dll)"
          onFilesSelected={handlePluginFilesSelected}
        />
      </Section>

      <ConfirmModal
        isOpen={pendingDelete !== null}
        title="Unload plugin?"
        message={
          pendingDelete
            ? `This will unload "${pendingDelete.original_kind}" and delete its file from the server.`
            : ''
        }
        confirmLabel="Unload"
        onConfirm={handleConfirmDelete}
        onCancel={() => setPendingDelete(null)}
        isLoading={deletingKind !== null}
      />
    </>
  );
};

export default InstalledPluginsTab;
