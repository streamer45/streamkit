// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as AlertDialog from '@radix-ui/react-alert-dialog';
import { Upload } from 'lucide-react';
import React from 'react';

import { AudioAssetLibrary } from '@/components/AudioAssetLibrary';
import NodePalette from '@/components/NodePalette';
import { SKTooltip } from '@/components/Tooltip';
import { Button } from '@/components/ui/Button';
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from '@/components/ui/Tabs';
import { UploadDropZone } from '@/components/UploadDropZone';
import { useToast } from '@/context/ToastContext';
import { usePermissions } from '@/hooks/usePermissions';
import { uploadPlugin, deletePlugin } from '@/services/plugins';
import { ensurePluginsLoaded, usePluginStore } from '@/stores/pluginStore';
import { reloadSchemas } from '@/stores/schemaStore';
import type { NodeDefinition, PacketType, PluginSummary } from '@/types/types';
import type { JsonSchema, JsonSchemaProperty } from '@/utils/jsonSchema';
import { getLogger } from '@/utils/logger';
import {
  formatPacketType,
  getPacketTypeColor,
  formatPinCardinality,
  getPinCardinalityIcon,
  getPinCardinalityDescription,
} from '@/utils/packetTypes';

import SamplePipelinesPane, {
  type SamplePipelinesPaneRef,
  type FragmentSample,
} from './SamplePipelinesPane';

const logger = getLogger('ControlPane');

const PanelWrapper = styled.aside`
  position: relative;
  height: 100%;
  width: 100%;
  border-right: 1px solid var(--sk-border);
  background-color: var(--sk-sidebar-bg);
  color: var(--sk-text);
  z-index: 1;
  word-break: break-word;
  display: flex;
  flex-direction: column;
`;

const BackButton = styled(Button)`
  width: fit-content;
`;

const PaneHeader = styled.div`
  padding: 12px;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

const PaneTitle = styled.h3`
  margin: 0 0 4px 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const PaneSubtitle = styled.p`
  margin: 0;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const PluginContent = styled.div`
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const PluginSection = styled.section`
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

const PluginList = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const PluginItem = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 10px 12px;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  background: var(--sk-panel-bg);
  position: relative;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
  }

  &:hover .plugin-delete-button {
    opacity: 1;
    pointer-events: auto;
  }
`;

const PluginDeleteButton = styled.button`
  position: absolute;
  top: 8px;
  right: 8px;
  padding: 4px 8px;
  background: var(--sk-danger);
  color: white;
  border: none;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.15s ease;

  &:hover {
    opacity: 1 !important;
    background: var(--sk-danger-hover);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

const PluginRow = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
`;

const PluginInfo = styled.div`
  display: flex;
  flex-direction: column;
  gap: 2px;
  color: var(--sk-text-muted);
  font-size: 11px;
`;

const PluginBadge = styled.span<{ $pluginType?: 'wasm' | 'native' }>`
  background: ${(props) =>
    props.$pluginType === 'native' ? 'var(--sk-success)' : 'var(--sk-primary)'};
  color: var(--sk-text-white);
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 999px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
`;

const EmptyState = styled.div`
  color: var(--sk-text-muted);
  font-size: 12px;
`;

// Reuse base dialog styles and create AlertDialog-specific wrappers
const AlertDialogOverlay = styled(AlertDialog.Overlay)`
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  backdrop-filter: blur(4px);
  z-index: 1000;
  animation: overlayShow 0.15s cubic-bezier(0.16, 1, 0.3, 1);

  @keyframes overlayShow {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
`;

const AlertDialogContent = styled(AlertDialog.Content)`
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  max-width: 420px;
  width: min(420px, 90vw);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  z-index: 1001;
  overflow: hidden;
  animation: contentShow 0.15s cubic-bezier(0.16, 1, 0.3, 1);

  &:focus {
    outline: none;
  }

  @keyframes contentShow {
    from {
      opacity: 0;
      transform: translate(-50%, -48%) scale(0.96);
    }
    to {
      opacity: 1;
      transform: translate(-50%, -50%) scale(1);
    }
  }
`;

const DialogHeader = styled.div`
  padding: 16px 20px;
  border-bottom: 1px solid var(--sk-border);
`;

const DialogTitle = styled(AlertDialog.Title)`
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: var(--sk-text);
`;

const DialogBody = styled.div`
  padding: 16px 20px;
`;

const DialogDescription = styled(AlertDialog.Description)`
  margin: 0;
  font-size: 14px;
  color: var(--sk-text-muted);
  line-height: 1.5;
`;

const DialogActions = styled.div`
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px;
  border-top: 1px solid var(--sk-border);
  background: var(--sk-sidebar-bg);
`;

const DialogButton = styled(Button)`
  min-width: 90px;
`;

const DialogDangerButton = styled(Button)`
  min-width: 90px;
`;

interface ControlPaneProps {
  nodeDefinitions: NodeDefinition[];
  onDragStart: (event: React.DragEvent, nodeType: string) => void;
  onAssetDragStart?: (
    event: React.DragEvent,
    asset: import('@/types/generated/api-types').AudioAsset
  ) => void;
  onLoadSample?: (yaml: string, name: string, description: string) => void;
  samplesRef?: React.RefObject<SamplePipelinesPaneRef | null>;
  mode?: 'oneshot' | 'dynamic';
  onFragmentDragStart?: (event: React.DragEvent, fragment: FragmentSample) => void;
  onFragmentInsert?: (fragment: FragmentSample) => void;
}

const ControlPane: React.FC<ControlPaneProps> = ({
  nodeDefinitions,
  onDragStart,
  onAssetDragStart,
  mode,
  onLoadSample,
  samplesRef,
  onFragmentDragStart,
  onFragmentInsert,
}) => {
  const [selectedDef, setSelectedDef] = React.useState<NodeDefinition | null>(null);
  const [selectedTop, setSelectedTop] = React.useState<string | null>(null);
  const toast = useToast();
  const toastRef = React.useRef(toast);
  const { can, isAdmin } = usePermissions();
  const plugins = usePluginStore((s) => s.plugins);
  const upsertPlugin = usePluginStore((s) => s.upsertPlugin);
  const removePluginFromStore = usePluginStore((s) => s.removePlugin);
  const [isUploading, setIsUploading] = React.useState(false);
  const [deletingKind, setDeletingKind] = React.useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = React.useState<PluginSummary | null>(null);

  React.useEffect(() => {
    toastRef.current = toast;
  }, [toast]);

  React.useEffect(() => {
    ensurePluginsLoaded().catch((err) => {
      logger.error('Failed to load plugins', err);
      toastRef.current.error('Could not load plugin list. Check logs for details.');
    });
  }, []);

  const pluginKinds = React.useMemo(() => new Set(plugins.map((p) => p.kind)), [plugins]);
  const pluginTypes = React.useMemo(
    () => new Map(plugins.map((p) => [p.kind, p.plugin_type])),
    [plugins]
  );
  const [activeTab, setActiveTab] = React.useState<'nodes' | 'plugins' | 'samples'>('nodes');

  const handlePluginFilesSelected = React.useCallback(
    async (files: FileList) => {
      const file = files?.[0];
      if (!file) return;
      if (isUploading || !can.loadPlugin) return;

      const isWasm = file.name.endsWith('.wasm');
      const isNative =
        file.name.endsWith('.so') || file.name.endsWith('.dylib') || file.name.endsWith('.dll');

      if (!isWasm && !isNative) {
        toast.error('Plugins must be .wasm (WASM) or .so/.dylib/.dll (native) files');
        return;
      }

      try {
        setIsUploading(true);
        const summary = await uploadPlugin(file);
        upsertPlugin(summary);
        await reloadSchemas();
        toast.success(`Loaded plugin ${summary.kind}`);
      } catch (err) {
        logger.error('Failed to upload plugin:', err);
        toast.error(err instanceof Error ? err.message : 'Failed to upload plugin');
      } finally {
        setIsUploading(false);
      }
    },
    [can.loadPlugin, isUploading, toast, upsertPlugin]
  );

  const performRemovePlugin = React.useCallback(
    async (kind: string) => {
      try {
        setDeletingKind(kind);
        await deletePlugin(kind);
        removePluginFromStore(kind);
        await reloadSchemas();
        toast.success(`Unloaded plugin ${kind}`);
      } catch (err) {
        logger.error('Failed to unload plugin:', err);
        toast.error(err instanceof Error ? err.message : 'Failed to unload plugin');
      } finally {
        setDeletingKind(null);
      }
    },
    [removePluginFromStore, toast]
  );

  const handleConfirmDelete = React.useCallback(async () => {
    if (!pendingDelete) return;
    const kind = pendingDelete.kind;
    setPendingDelete(null);
    await performRemovePlugin(kind);
  }, [pendingDelete, performRemovePlugin]);

  // Memoize callback for back button
  const handleBackToNodeList = React.useCallback(() => setSelectedDef(null), []);

  // Memoize node details JSX to prevent re-renders
  const nodeDetailsContent = React.useMemo(() => {
    if (!selectedDef) return null;

    return (
      <div
        style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: 12, overflowY: 'auto' }}
      >
        <BackButton
          variant="ghost"
          size="small"
          onClick={handleBackToNodeList}
          aria-label="Back to node list"
        >
          ← Back
        </BackButton>

        <div
          style={{
            fontWeight: 'bold',
            fontSize: '16px',
            display: 'flex',
            alignItems: 'center',
            gap: 8,
          }}
          className="code-font"
        >
          {selectedDef.kind}
          {pluginKinds.has(selectedDef.kind) &&
            (() => {
              const plugin = plugins.find((p) => p.kind === selectedDef.kind);
              return plugin ? (
                <PluginBadge $pluginType={plugin.plugin_type}>
                  {plugin.plugin_type === 'native' ? 'Native' : 'WASM'}
                </PluginBadge>
              ) : (
                <PluginBadge>Plugin</PluginBadge>
              );
            })()}
        </div>

        <div
          style={{
            backgroundColor: 'var(--sk-panel-bg)',
            border: '1px solid var(--sk-border)',
            borderRadius: '8px',
            padding: '16px',
            display: 'flex',
            flexDirection: 'column',
            gap: '16px',
            boxSizing: 'border-box',
          }}
        >
          <section style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
            <div style={{ fontWeight: 'bold' }}>Inputs</div>
            {selectedDef.inputs.length === 0 ? (
              <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>No inputs</div>
            ) : (
              <ul
                style={{
                  paddingLeft: 16,
                  margin: 0,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: '4px',
                }}
              >
                {selectedDef.inputs.map((inp) => {
                  const primaryType = (inp.accepts_types?.[0] ?? 'Any') as unknown as PacketType;
                  const color = getPacketTypeColor(primaryType);
                  const cardinalityIcon = getPinCardinalityIcon(inp.cardinality);
                  const cardinalityText = formatPinCardinality(inp.cardinality);
                  const cardinalityDescription = getPinCardinalityDescription(
                    inp.cardinality,
                    true
                  );
                  return (
                    <li key={inp.name}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                        <span
                          style={{
                            width: 10,
                            height: 10,
                            background: color,
                            borderRadius: 4,
                            border: '1px solid var(--sk-border-strong)',
                            display: 'inline-block',
                          }}
                        />
                        <div style={{ flex: 1 }}>
                          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                            <span className="code-font" style={{ fontWeight: 600 }}>
                              {inp.name}
                            </span>
                            <SKTooltip
                              content={
                                <div style={{ fontSize: 11 }}>
                                  <div style={{ fontWeight: 600, marginBottom: 4 }}>
                                    {cardinalityText}
                                  </div>
                                  <div>{cardinalityDescription}</div>
                                </div>
                              }
                            >
                              <span
                                style={{
                                  fontSize: 11,
                                  color: 'var(--sk-text-muted)',
                                  cursor: 'help',
                                }}
                              >
                                {cardinalityIcon}
                              </span>
                            </SKTooltip>
                          </div>
                          <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                            {(inp.accepts_types || [])
                              .map((t: PacketType) => formatPacketType(t))
                              .join(' | ')}
                          </div>
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          <section style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
            <div style={{ fontWeight: 'bold' }}>Outputs</div>
            {selectedDef.outputs.length === 0 ? (
              <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>No outputs</div>
            ) : (
              <ul
                style={{
                  paddingLeft: 16,
                  margin: 0,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: '4px',
                }}
              >
                {selectedDef.outputs.map((outp) => {
                  const color = getPacketTypeColor(outp.produces_type as PacketType);
                  const cardinalityIcon = getPinCardinalityIcon(outp.cardinality);
                  const cardinalityText = formatPinCardinality(outp.cardinality);
                  const cardinalityDescription = getPinCardinalityDescription(
                    outp.cardinality,
                    false
                  );
                  return (
                    <li key={outp.name}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                        <span
                          style={{
                            width: 10,
                            height: 10,
                            background: color,
                            borderRadius: 4,
                            border: '1px solid var(--sk-border-strong)',
                            display: 'inline-block',
                          }}
                        />
                        <div style={{ flex: 1 }}>
                          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                            <span className="code-font" style={{ fontWeight: 600 }}>
                              {outp.name}
                            </span>
                            <SKTooltip
                              content={
                                <div style={{ fontSize: 11 }}>
                                  <div style={{ fontWeight: 600, marginBottom: 4 }}>
                                    {cardinalityText}
                                  </div>
                                  <div>{cardinalityDescription}</div>
                                </div>
                              }
                            >
                              <span
                                style={{
                                  fontSize: 11,
                                  color: 'var(--sk-text-muted)',
                                  cursor: 'help',
                                }}
                              >
                                {cardinalityIcon}
                              </span>
                            </SKTooltip>
                          </div>
                          <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                            {formatPacketType(outp.produces_type as PacketType)}
                          </div>
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          <section style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
            <div style={{ fontWeight: 'bold' }}>Parameters</div>
            {(selectedDef.param_schema as JsonSchema | undefined)?.properties &&
            Object.keys((selectedDef.param_schema as JsonSchema).properties!).length > 0 ? (
              <ul
                style={{
                  paddingLeft: 16,
                  margin: 0,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: '8px',
                }}
              >
                {Object.entries(
                  (selectedDef.param_schema as JsonSchema).properties! as Record<
                    string,
                    JsonSchemaProperty
                  >
                ).map(([key, schema]) => (
                  <li key={key}>
                    <div>
                      <span className="code-font" style={{ fontWeight: 600 }}>
                        {key}
                      </span>
                      {schema.type && (
                        <span
                          style={{ marginLeft: 8, fontSize: 12, color: 'var(--sk-text-muted)' }}
                        >
                          ({schema.type})
                        </span>
                      )}
                      {schema.default !== undefined && (
                        <span
                          style={{ marginLeft: 8, fontSize: 12, color: 'var(--sk-text-muted)' }}
                        >
                          default: {String(schema.default)}
                        </span>
                      )}
                    </div>
                    {schema.description && (
                      <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                        {schema.description}
                      </div>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                No configurable parameters
              </div>
            )}
          </section>
        </div>
      </div>
    );
  }, [selectedDef, handleBackToNodeList, pluginKinds, plugins]);

  // Memoize node library JSX to prevent re-renders
  const nodeLibraryContent = React.useMemo(() => {
    if (nodeDefinitions.length === 0) {
      return (
        <div style={{ padding: 12, fontSize: 12, color: 'var(--sk-text-muted)' }}>
          Loading nodes…
        </div>
      );
    }

    if (selectedDef) {
      return nodeDetailsContent;
    }

    return (
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <NodePalette
          nodeDefinitions={nodeDefinitions}
          onDragStart={onDragStart}
          onNodeClick={setSelectedDef}
          pluginKinds={pluginKinds}
          pluginTypes={pluginTypes}
          selectedTop={selectedTop}
          onSelectedTopChange={setSelectedTop}
        />
      </div>
    );
  }, [
    nodeDefinitions,
    selectedDef,
    nodeDetailsContent,
    onDragStart,
    setSelectedDef,
    pluginKinds,
    pluginTypes,
    selectedTop,
    setSelectedTop,
  ]);

  // Memoize tab change handler to prevent re-renders
  const handleTabChange = React.useCallback((value: string) => {
    setActiveTab(value as 'nodes' | 'samples' | 'plugins');
  }, []);

  // Memoize dialog open change handler to prevent re-renders
  const handleDialogOpenChange = React.useCallback((open: boolean) => {
    if (!open) {
      setPendingDelete(null);
    }
  }, []);

  // Memoize samples tab content to prevent re-renders
  const samplesTabContent = React.useMemo(() => {
    if (!onLoadSample) return null;
    return (
      <SamplePipelinesPane
        ref={samplesRef}
        onLoadSample={onLoadSample}
        mode={mode}
        onFragmentDragStart={onFragmentDragStart}
        onFragmentInsert={onFragmentInsert}
      />
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- samplesRef is stable and shouldn't trigger re-memoization
  }, [onLoadSample, mode, onFragmentDragStart, onFragmentInsert]);

  // Memoize plugin management JSX to prevent re-renders
  const pluginManagementContent = React.useMemo(
    () => (
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <PaneHeader>
          <PaneTitle>Plugin Management</PaneTitle>
          <PaneSubtitle>Upload WASM or native plugins, or unload existing ones</PaneSubtitle>
        </PaneHeader>
        <PluginContent>
          <SKTooltip
            content={
              !can.loadPlugin
                ? 'You do not have permission to upload plugins'
                : 'Upload a new WASM or native plugin'
            }
          >
            <div>
              <UploadDropZone
                accept=".wasm,.so,.dylib,.dll"
                disabled={isUploading || !can.loadPlugin}
                icon={<Upload size={24} />}
                text={isUploading ? 'Uploading…' : 'Drop plugin file here or click to browse'}
                hint="Accepted: WASM (.wasm) or native (.so, .dylib, .dll)"
                onFilesSelected={handlePluginFilesSelected}
              />
            </div>
          </SKTooltip>
          <PluginSection>
            {plugins.length === 0 ? (
              <EmptyState>No plugins loaded yet</EmptyState>
            ) : (
              <PluginList>
                {plugins.map((plugin) => {
                  const loadedAt = new Date(plugin.loaded_at_ms).toLocaleString();
                  return (
                    <PluginItem key={plugin.kind}>
                      <PluginRow>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                          <span className="code-font" style={{ fontWeight: 600 }}>
                            {plugin.kind}
                          </span>
                          <PluginBadge $pluginType={plugin.plugin_type}>
                            {plugin.plugin_type === 'native' ? 'Native' : 'WASM'}
                          </PluginBadge>
                        </div>
                      </PluginRow>
                      <PluginInfo>
                        <span>Original kind: {plugin.original_kind}</span>
                        <span>File: {plugin.file_name}</span>
                        <span>Loaded: {loadedAt}</span>
                      </PluginInfo>
                      <SKTooltip
                        content={
                          !can.deletePlugin
                            ? 'You do not have permission to delete plugins'
                            : 'Unload this plugin'
                        }
                      >
                        <PluginDeleteButton
                          className="plugin-delete-button"
                          onClick={() => setPendingDelete(plugin)}
                          disabled={deletingKind === plugin.kind || !can.deletePlugin}
                        >
                          {deletingKind === plugin.kind ? '⏳ Removing…' : '🗑️ Unload'}
                        </PluginDeleteButton>
                      </SKTooltip>
                    </PluginItem>
                  );
                })}
              </PluginList>
            )}
          </PluginSection>
        </PluginContent>
      </div>
    ),
    [
      isUploading,
      handlePluginFilesSelected,
      plugins,
      deletingKind,
      can.loadPlugin,
      can.deletePlugin,
    ]
  );

  return (
    <>
      <PanelWrapper data-testid="control-pane">
        <TabsRoot value={activeTab} onValueChange={handleTabChange}>
          <TabsList>
            <TabsTrigger value="nodes">Nodes</TabsTrigger>
            <TabsTrigger value="assets">Assets</TabsTrigger>
            <TabsTrigger value="samples" data-testid="samples-tab">
              Samples
            </TabsTrigger>
            {isAdmin() && <TabsTrigger value="plugins">Plugins</TabsTrigger>}
          </TabsList>
          <TabsContent value="nodes">{nodeLibraryContent}</TabsContent>
          <TabsContent value="assets">
            <AudioAssetLibrary onDragStart={onAssetDragStart} />
          </TabsContent>
          <TabsContent value="samples">{samplesTabContent}</TabsContent>
          {isAdmin() && <TabsContent value="plugins">{pluginManagementContent}</TabsContent>}
        </TabsRoot>
      </PanelWrapper>

      <AlertDialog.Root open={pendingDelete !== null} onOpenChange={handleDialogOpenChange}>
        <AlertDialog.Portal>
          <AlertDialogOverlay />
          <AlertDialogContent>
            <DialogHeader>
              <DialogTitle>Unload plugin?</DialogTitle>
            </DialogHeader>
            <DialogBody>
              <DialogDescription>
                {pendingDelete
                  ? `This will remove the "${pendingDelete.original_kind}" plugin and delete its file from the server.`
                  : ''}
              </DialogDescription>
            </DialogBody>
            <DialogActions>
              <AlertDialog.Cancel asChild>
                <DialogButton
                  variant="ghost"
                  type="button"
                  onClick={() => setPendingDelete(null)}
                  disabled={deletingKind !== null}
                >
                  Cancel
                </DialogButton>
              </AlertDialog.Cancel>
              <AlertDialog.Action asChild>
                <DialogDangerButton
                  variant="danger"
                  type="button"
                  onClick={handleConfirmDelete}
                  disabled={deletingKind !== null}
                >
                  {deletingKind ? 'Removing…' : 'Unload'}
                </DialogDangerButton>
              </AlertDialog.Action>
            </DialogActions>
          </AlertDialogContent>
        </AlertDialog.Portal>
      </AlertDialog.Root>
    </>
  );
};

export default React.memo(ControlPane);
