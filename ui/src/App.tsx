// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React, { Suspense, useState, useEffect } from 'react';
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
import DesignView from './views/DesignView';

const ConvertView = React.lazy(() => import('./views/ConvertView'));
const MonitorView = React.lazy(() => import('./views/MonitorView'));
const LoginView = React.lazy(() => import('./views/LoginView'));
const LogsView = React.lazy(() => import('./views/LogsView'));
const PluginsView = React.lazy(() => import('./views/PluginsView'));
const StreamView = React.lazy(() => import('./views/StreamView'));
const TokensView = React.lazy(() => import('./views/TokensView'));

const logger = getLogger('App');

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
                    element={
                      <Suspense fallback={<LoadingSpinner message="Loading..." />}>
                        <LoginView onLoggedIn={() => setRequiresLogin(false)} />
                      </Suspense>
                    }
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
