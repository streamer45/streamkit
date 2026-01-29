// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useCallback, useEffect, useMemo, useState } from 'react';

import ConfirmModal from '@/components/ConfirmModal';
import { CopyButton } from '@/components/CopyButton';
import { Button } from '@/components/ui/Button';
import {
  createApiToken,
  createMoqToken,
  fetchAuthMe,
  listTokens,
  logout,
  revokeToken,
  type TokenInfo,
} from '@/services/auth';
import { useStreamStore } from '@/stores/streamStore';
import { getBasePathname } from '@/utils/baseHref';
import { getLogger } from '@/utils/logger';

import AdminNav from './admin/AdminNav';
import { MintedTokensTable } from './MintedTokensTable';
import {
  BottomSpacer,
  Card,
  Container,
  ContentArea,
  ContentWrapper,
  ErrorBox,
  Grid,
  Input,
  Label,
  NoticeBox,
  Row,
  Section,
  SectionTitle,
  Select,
  Subtle,
  SuccessBox,
  TextArea,
  TextAreaWithCopy,
  TextAreaWithCopyWrapper,
  Title,
  TitleRow,
} from './TokensView.styles';

const logger = getLogger('TokensView');

function splitLines(value: string): string[] {
  return value
    .split('\n')
    .map((v) => v.trim())
    .filter(Boolean);
}

function formatAuthStatus(authEnabled: boolean | null): string {
  if (authEnabled === null) return 'unknown';
  return authEnabled ? 'enabled' : 'disabled';
}

function shouldShowLogout(authEnabled: boolean | null, authenticated: boolean | null): boolean {
  return authEnabled === true && authenticated === true;
}

function canAdminManageTokens(
  authEnabled: boolean | null,
  authenticated: boolean | null,
  role: string | null
): boolean {
  return authEnabled === true && authenticated === true && role === 'admin';
}

function renderError(error: string | null): React.ReactNode {
  if (!error) return null;
  return <ErrorBox>{error}</ErrorBox>;
}

function renderLogoutButton(canLogout: boolean, onLogout: () => void): React.ReactNode {
  if (!canLogout) return null;
  return (
    <Button variant="ghost" onClick={onLogout} data-testid="tokens-logout">
      Logout
    </Button>
  );
}

function renderAuthDisabledNotice(authEnabled: boolean | null): React.ReactNode {
  if (authEnabled !== false) return null;
  return (
    <NoticeBox>
      Built-in authentication is disabled on this server. Enable auth to mint, list, and revoke
      tokens.
    </NoticeBox>
  );
}

function renderAdminRequiredNotice(
  authEnabled: boolean | null,
  authenticated: boolean | null,
  role: string | null
): React.ReactNode {
  if (!(authEnabled === true && authenticated === true)) return null;
  if (role === 'admin') return null;
  return <ErrorBox>Admin role required to manage tokens on this server.</ErrorBox>;
}

function renderCreatedToken(label: string, token: string | null): React.ReactNode {
  if (!token) return null;
  return (
    <SuccessBox>
      <div>{label}</div>
      <TextAreaWithCopyWrapper>
        <CopyButton text={token} />
        <TextAreaWithCopy value={token} readOnly />
      </TextAreaWithCopyWrapper>
    </SuccessBox>
  );
}

function renderUseInStreamButton(token: string | null, onUse: () => void): React.ReactNode {
  if (!token) return null;
  return (
    <Button variant="ghost" onClick={onUse}>
      Use in Stream view
    </Button>
  );
}

function getRevokeConfirmMessage(token: { jti: string; label: string | null } | null): string {
  const suffix =
    'This action cannot be undone. Any applications using this token will immediately lose access.';

  if (!token) {
    return `Revoke token? ${suffix}`;
  }

  const target = token.label ? `"${token.label}"` : `with JTI "${token.jti.substring(0, 8)}..."`;
  return `Revoke token ${target}? ${suffix}`;
}

function useApiTokenFormState() {
  const [apiRole, setApiRole] = useState('viewer');
  const [apiLabel, setApiLabel] = useState('');
  const [apiTtlSecs, setApiTtlSecs] = useState<number>(86400);
  const [createdApiToken, setCreatedApiToken] = useState<string | null>(null);

  return {
    apiRole,
    setApiRole,
    apiLabel,
    setApiLabel,
    apiTtlSecs,
    setApiTtlSecs,
    createdApiToken,
    setCreatedApiToken,
  };
}

function useMoqTokenFormState(defaultMoqRoot: string) {
  const [moqRoot, setMoqRoot] = useState(defaultMoqRoot);
  const [moqSubscribe, setMoqSubscribe] = useState('output');
  const [moqPublish, setMoqPublish] = useState('input');
  const [moqTtlSecs, setMoqTtlSecs] = useState<number>(3600);
  const [createdMoqToken, setCreatedMoqToken] = useState<string | null>(null);
  const [createdMoqUrl, setCreatedMoqUrl] = useState<string | null>(null);

  useEffect(() => {
    setMoqRoot(defaultMoqRoot);
  }, [defaultMoqRoot]);

  return {
    moqRoot,
    setMoqRoot,
    moqSubscribe,
    setMoqSubscribe,
    moqPublish,
    setMoqPublish,
    moqTtlSecs,
    setMoqTtlSecs,
    createdMoqToken,
    setCreatedMoqToken,
    createdMoqUrl,
    setCreatedMoqUrl,
  };
}

const TokensView: React.FC = () => {
  const setMoqToken = useStreamStore((s) => s.setMoqToken);
  const serverUrl = useStreamStore((s) => s.serverUrl);

  const defaultMoqRoot = useMemo(() => {
    try {
      const url = new URL(serverUrl);
      return url.pathname || '/moq';
    } catch {
      return '/moq';
    }
  }, [serverUrl]);

  const [isLoading, setIsLoading] = useState(true);
  const [authenticated, setAuthenticated] = useState<boolean | null>(null);
  const [role, setRole] = useState<string | null>(null);
  const [authEnabled, setAuthEnabled] = useState<boolean | null>(null);
  const [currentJti, setCurrentJti] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tokens, setTokens] = useState<TokenInfo[]>([]);

  const {
    apiRole,
    setApiRole,
    apiLabel,
    setApiLabel,
    apiTtlSecs,
    setApiTtlSecs,
    createdApiToken,
    setCreatedApiToken,
  } = useApiTokenFormState();

  const {
    moqRoot,
    setMoqRoot,
    moqSubscribe,
    setMoqSubscribe,
    moqPublish,
    setMoqPublish,
    moqTtlSecs,
    setMoqTtlSecs,
    createdMoqToken,
    setCreatedMoqToken,
    createdMoqUrl,
    setCreatedMoqUrl,
  } = useMoqTokenFormState(defaultMoqRoot);

  const [revokeConfirmOpen, setRevokeConfirmOpen] = useState(false);
  const [revokeConfirmLoading, setRevokeConfirmLoading] = useState(false);
  const [tokenToRevoke, setTokenToRevoke] = useState<{ jti: string; label: string | null } | null>(
    null
  );

  const canLogout = shouldShowLogout(authEnabled, authenticated);
  const canManageTokens = canAdminManageTokens(authEnabled, authenticated, role);

  const onUseCreatedMoqToken = useCallback(() => {
    if (!createdMoqToken) return;
    setMoqToken(createdMoqToken);
  }, [createdMoqToken, setMoqToken]);

  const refresh = useCallback(async () => {
    setError(null);
    setIsLoading(true);
    try {
      const me = await fetchAuthMe();
      setAuthEnabled(me.auth_enabled);
      setRole(me.role);
      setAuthenticated(me.authenticated);
      setCurrentJti(me.jti);
      if (!me.auth_enabled) {
        setTokens([]);
        return;
      }
      const list = await listTokens();
      setTokens(list);
    } catch (e) {
      const message = e instanceof Error ? e.message : 'Failed to load tokens';
      logger.error('Failed to refresh tokens view:', e);
      setError(message);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const onCreateApiToken = async () => {
    setError(null);
    setCreatedApiToken(null);
    if (!canManageTokens) {
      return;
    }
    try {
      const res = await createApiToken({
        role: apiRole,
        label: apiLabel.trim() ? apiLabel.trim() : undefined,
        ttl_secs: apiTtlSecs || undefined,
      });
      setCreatedApiToken(res.token);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create token');
    }
  };

  const onCreateMoqToken = async () => {
    setError(null);
    setCreatedMoqToken(null);
    setCreatedMoqUrl(null);
    if (!canManageTokens) {
      return;
    }
    try {
      const res = await createMoqToken({
        root: moqRoot.trim() || '/moq',
        subscribe: splitLines(moqSubscribe),
        publish: splitLines(moqPublish),
        ttl_secs: moqTtlSecs || undefined,
        label: 'ui-moq',
      });
      setCreatedMoqToken(res.token);
      setCreatedMoqUrl(res.url_template ?? null);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create MoQ token');
    }
  };

  const onRevoke = useCallback(
    (jti: string, label: string | null) => {
      if (!canManageTokens) {
        return;
      }
      setTokenToRevoke({ jti, label });
      setRevokeConfirmOpen(true);
    },
    [canManageTokens]
  );

  const confirmRevoke = useCallback(async () => {
    if (!tokenToRevoke) return;

    setError(null);
    setRevokeConfirmLoading(true);
    try {
      await revokeToken(tokenToRevoke.jti);
      await refresh();
      setRevokeConfirmOpen(false);
      setTokenToRevoke(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to revoke token');
    } finally {
      setRevokeConfirmLoading(false);
    }
  }, [tokenToRevoke, refresh]);

  const onLogout = async () => {
    setError(null);
    if (!canLogout) {
      return;
    }
    try {
      await logout();
      const basePathname = getBasePathname();
      window.location.assign(`${basePathname}/login`);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to logout');
    }
  };

  return (
    <>
      <ConfirmModal
        isOpen={revokeConfirmOpen}
        title="Revoke token?"
        message={getRevokeConfirmMessage(tokenToRevoke)}
        confirmLabel="Revoke Token"
        cancelLabel="Cancel"
        onConfirm={confirmRevoke}
        onCancel={() => setRevokeConfirmOpen(false)}
        isLoading={revokeConfirmLoading}
      />
      <Container data-testid="tokens-view">
        <ContentArea>
          <ContentWrapper>
            <Card>
              <TitleRow>
                <div>
                  <Title>Access Tokens</Title>
                  <Subtle>
                    Auth: {formatAuthStatus(authEnabled)} • Role: {role ?? 'unknown'}
                  </Subtle>
                </div>
                <Row>
                  <Button onClick={refresh} disabled={isLoading}>
                    Refresh
                  </Button>
                  {renderLogoutButton(canLogout, onLogout)}
                </Row>
              </TitleRow>

              <AdminNav />

              {renderError(error)}
              {renderAuthDisabledNotice(authEnabled)}
              {renderAdminRequiredNotice(authEnabled, authenticated, role)}

              <Grid>
                <Section>
                  <SectionTitle>Mint API Token</SectionTitle>
                  <Row>
                    <Label>
                      Role
                      <Select
                        value={apiRole}
                        onChange={(e) => setApiRole(e.target.value)}
                        disabled={!canManageTokens}
                      >
                        <option value="viewer">viewer</option>
                        <option value="user">user</option>
                        <option value="admin">admin</option>
                      </Select>
                    </Label>
                    <Label>
                      Label (optional)
                      <Input
                        value={apiLabel}
                        onChange={(e) => setApiLabel(e.target.value)}
                        disabled={!canManageTokens}
                      />
                    </Label>
                    <Label>
                      TTL seconds
                      <Input
                        type="number"
                        min={1}
                        value={apiTtlSecs}
                        onChange={(e) => setApiTtlSecs(Number(e.target.value))}
                        disabled={!canManageTokens}
                      />
                    </Label>
                  </Row>
                  <Row>
                    <Button onClick={onCreateApiToken} disabled={!canManageTokens}>
                      Mint API token
                    </Button>
                  </Row>
                  {renderCreatedToken('New token (copy now):', createdApiToken)}
                </Section>

                <Section>
                  <SectionTitle>Mint MoQ Token</SectionTitle>
                  <Row>
                    <Label>
                      Root (URL path prefix)
                      <Input
                        value={moqRoot}
                        onChange={(e) => setMoqRoot(e.target.value)}
                        disabled={!canManageTokens}
                      />
                    </Label>
                    <Label>
                      TTL seconds
                      <Input
                        type="number"
                        min={1}
                        value={moqTtlSecs}
                        onChange={(e) => setMoqTtlSecs(Number(e.target.value))}
                        disabled={!canManageTokens}
                      />
                    </Label>
                  </Row>
                  <Row>
                    <Label>
                      Subscribe paths (one per line, empty = deny all)
                      <TextArea
                        value={moqSubscribe}
                        onChange={(e) => setMoqSubscribe(e.target.value)}
                        disabled={!canManageTokens}
                      />
                    </Label>
                    <Label>
                      Publish paths (one per line, empty = deny all)
                      <TextArea
                        value={moqPublish}
                        onChange={(e) => setMoqPublish(e.target.value)}
                        disabled={!canManageTokens}
                      />
                    </Label>
                  </Row>
                  <Row>
                    <Button onClick={onCreateMoqToken} disabled={!canManageTokens}>
                      Mint MoQ token
                    </Button>
                    {renderUseInStreamButton(createdMoqToken, onUseCreatedMoqToken)}
                  </Row>
                  {renderCreatedToken('New MoQ token (copy now):', createdMoqToken)}
                  {renderCreatedToken(
                    createdMoqUrl && !/^https?:\/\//.test(createdMoqUrl)
                      ? 'New MoQ path (append to gateway URL):'
                      : 'New MoQ URL (copy now):',
                    createdMoqUrl
                  )}
                </Section>
              </Grid>

              <Section>
                <SectionTitle>Minted Tokens</SectionTitle>
                <MintedTokensTable
                  isLoading={isLoading}
                  tokens={tokens}
                  canManageTokens={canManageTokens}
                  currentJti={currentJti}
                  onRevoke={onRevoke}
                />
              </Section>
            </Card>
            <BottomSpacer />
          </ContentWrapper>
        </ContentArea>
      </Container>
    </>
  );
};

export default TokensView;
