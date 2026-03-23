// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React, { useState, useEffect } from 'react';
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';

import { ErrorBoundary } from './components/ErrorBoundary';
import { LoadingSpinner } from './components/LoadingSpinner';
import { TooltipProvider } from './components/Tooltip';
import { ThemeProvider } from './context/ThemeContext';
import { ToastProvider } from './context/ToastContext';
import Layout from './Layout';
import { fetchAuthMe } from './services/auth';
import { initializePermissions } from './services/permissions';
import { ensureSchemasLoaded } from './stores/schemaStore';
import { getBasePathname } from './utils/baseHref';
import { getLogger } from './utils/logger';
import ConvertView from './views/ConvertView';
import DesignView from './views/DesignView';
import LoginView from './views/LoginView';
import LogsView from './views/LogsView';
import MonitorView from './views/MonitorView';
import PluginsView from './views/PluginsView';
import StreamView from './views/StreamView';
import TokensView from './views/TokensView';

const logger = getLogger('App');

// Create a client
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      retry: 1,
      staleTime: 5000,
    },
  },
});

const App: React.FC = () => {
  const [appReady, setAppReady] = useState(false);
  const [requiresLogin, setRequiresLogin] = useState(false);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const me = await fetchAuthMe();
        if (cancelled) return;

        if (me.auth_enabled && !me.authenticated) {
          setRequiresLogin(true);
          setAppReady(true);
          return;
        }
      } catch (err) {
        logger.error('Failed to check auth status:', err);
      }

      await Promise.all([
        initializePermissions().catch((err) => {
          logger.error('Failed to initialize permissions:', err);
        }),
        ensureSchemasLoaded().catch((err) => {
          logger.error('Failed to load schemas on startup:', err);
        }),
      ]);

      if (cancelled) return;
      setRequiresLogin(false);
      setAppReady(true);
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  if (!appReady) {
    return (
      <ThemeProvider>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            height: '100vh',
            backgroundColor: 'var(--sk-bg)',
          }}
        >
          <LoadingSpinner message="Loading..." />
        </div>
      </ThemeProvider>
    );
  }

  return (
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <ThemeProvider>
          <ToastProvider>
            <TooltipProvider delayDuration={300} skipDelayDuration={200}>
              <BrowserRouter basename={getBasePathname()}>
                <Routes>
                  <Route
                    path="/login"
                    element={<LoginView onLoggedIn={() => setRequiresLogin(false)} />}
                  />
                  <Route
                    path="/"
                    element={requiresLogin ? <Navigate to="/login" replace /> : <Layout />}
                  >
                    <Route index element={<Navigate to="/design" replace />} />
                    <Route path="design" element={<DesignView />} />
                    <Route path="monitor" element={<MonitorView />} />
                    <Route path="convert" element={<ConvertView />} />
                    <Route path="stream" element={<StreamView />} />
                    <Route path="admin" element={<Navigate to="/admin/plugins" replace />} />
                    <Route
                      path="admin/plugins"
                      element={<Navigate to="/admin/plugins/installed" replace />}
                    />
                    <Route path="admin/plugins/:tab" element={<PluginsView />} />
                    <Route path="admin/tokens" element={<TokensView />} />
                    <Route path="admin/logs" element={<LogsView />} />
                  </Route>
                </Routes>
              </BrowserRouter>
            </TooltipProvider>
          </ToastProvider>
        </ThemeProvider>
      </QueryClientProvider>
    </ErrorBoundary>
  );
};

export default App;
