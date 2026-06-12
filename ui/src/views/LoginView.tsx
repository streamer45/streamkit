// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';

import { Button } from '@/components/ui/Button';
import { fetchAuthMe, loginWithToken } from '@/services/auth';
import { initializePermissions } from '@/services/permissions';
import { ensureSchemasLoaded } from '@/stores/schemaStore';
import { getLogger } from '@/utils/logger';

const logger = getLogger('LoginView');

const Container = styled.div`
  box-sizing: border-box;
  height: 100%;
  width: 100%;
  min-width: 0;
  overflow: auto;
  display: flex;
  justify-content: center;
  align-items: flex-start;
  padding: 40px 24px;
`;

const Card = styled.div`
  box-sizing: border-box;
  width: 100%;
  max-width: 640px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 16px;
`;

const Title = styled.h1`
  margin: 0;
  font-size: 20px;
  font-weight: 700;
  color: var(--sk-text);
`;

const HelpText = styled.p`
  margin: 0;
  color: var(--sk-text-muted);
  line-height: 1.5;
  font-size: 14px;
`;

const TipBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-primary) 8%, transparent);
  display: flex;
  gap: 10px;
  align-items: flex-start;
`;

const TipIcon = styled.div`
  flex: 0 0 auto;
  font-size: 16px;
  line-height: 1;
  margin-top: 1px;
`;

const TipContent = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
`;

const TipText = styled.div`
  color: var(--sk-text-muted);
  line-height: 1.45;
  font-size: 13px;
`;

const CommandBlock = styled.code`
  display: block;
  padding: 8px 10px;
  border-radius: 8px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-family: var(--sk-font-code);
  font-size: 12px;
  line-height: 1.4;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
`;

const Label = styled.label`
  font-size: 13px;
  color: var(--sk-text-muted);
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const TextArea = styled.textarea`
  box-sizing: border-box;
  width: 100%;
  min-height: 120px;
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-bg);
  color: var(--sk-text);
  resize: vertical;
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New',
    monospace;
  font-size: 12px;
  line-height: 1.5;
`;

const ErrorBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-danger) 10%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

const Actions = styled.div`
  display: flex;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
`;

export interface LoginViewProps {
  onLoggedIn?: () => void;
}

const LoginView: React.FC<LoginViewProps> = ({ onLoggedIn }) => {
  const navigate = useNavigate();
  const [token, setToken] = useState('');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [authEnabled, setAuthEnabled] = useState<boolean | null>(null);

  useEffect(() => {
    fetchAuthMe()
      .then((me) => {
        setAuthEnabled(me.auth_enabled);
        if (me.auth_enabled && me.authenticated) {
          navigate('/design', { replace: true });
        }
      })
      .catch((e) => {
        logger.error('Failed to check auth status:', e);
        setAuthEnabled(null);
      });
  }, [navigate]);

  const onLogin = async () => {
    setError(null);
    const trimmed = token.trim();
    if (!trimmed) {
      setError('Paste a token to continue.');
      return;
    }

    setIsSubmitting(true);
    try {
      await loginWithToken(trimmed);
      await Promise.all([initializePermissions(), ensureSchemasLoaded()]);
      onLoggedIn?.();
      navigate('/design', { replace: true });
    } catch (e) {
      const message = e instanceof Error ? e.message : 'Login failed';
      setError(message);
    }
    setIsSubmitting(false);
  };

  return (
    <Container data-testid="login-view">
      <Card>
        <Title>Sign in to StreamKit</Title>
        <HelpText>
          Paste an admin (or user) token to access this instance. StreamKit stores it in an HttpOnly
          cookie, so your browser can authenticate without query params.
        </HelpText>
        {authEnabled === false && (
          <HelpText>
            Authentication is disabled on this server. You can go straight to the app.
          </HelpText>
        )}

        <Label>
          Token
          <TextArea
            data-testid="login-token-input"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="Paste token here (e.g. from `skit auth print-admin-token`)"
            spellCheck={false}
          />
        </Label>
        <TipBox>
          <TipIcon aria-hidden>💡</TipIcon>
          <TipContent>
            <TipText>
              If you have shell access to this instance, grab the bootstrap token (or mint a
              short-lived one). Make sure you use the same <code>--config</code> file as the running
              server if it’s not <code>skit.toml</code>.
            </TipText>
            <CommandBlock>skit auth print-admin-token</CommandBlock>
            <CommandBlock>skit auth mint api --role admin --ttl-secs 3600</CommandBlock>
          </TipContent>
        </TipBox>

        {error && <ErrorBox data-testid="login-error">{error}</ErrorBox>}

        <Actions>
          <Button data-testid="login-submit" onClick={onLogin} disabled={isSubmitting}>
            {isSubmitting ? 'Signing in…' : 'Sign in'}
          </Button>
          <Button
            variant="ghost"
            onClick={() => navigate('/design')}
            disabled={authEnabled !== false}
            data-testid="login-continue-without-auth"
          >
            Continue without auth
          </Button>
        </Actions>
      </Card>
    </Container>
  );
};

export default LoginView;
