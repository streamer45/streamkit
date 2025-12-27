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
import { initializePermissions } from './services/permissions';
import { ensureSchemasLoaded } from './stores/schemaStore';
import { getBasePathname } from './utils/baseHref';
import { getLogger } from './utils/logger';
import ConvertView from './views/ConvertView';
import DesignView from './views/DesignView';
import MonitorView from './views/MonitorView';
import StreamView from './views/StreamView';

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
  const [schemasLoaded, setSchemasLoaded] = useState(false);

  useEffect(() => {
    // Initialize permissions and schemas in parallel
    Promise.all([
      initializePermissions().catch((err) => {
        logger.error('Failed to initialize permissions:', err);
      }),
      ensureSchemasLoaded().catch((err) => {
        logger.error('Failed to load schemas on startup:', err);
      }),
    ]).finally(() => {
      setSchemasLoaded(true);
    });
  }, []);

  if (!schemasLoaded) {
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
                  <Route path="/" element={<Layout />}>
                    <Route index element={<Navigate to="/design" replace />} />
                    <Route path="design" element={<DesignView />} />
                    <Route path="monitor" element={<MonitorView />} />
                    <Route path="convert" element={<ConvertView />} />
                    <Route path="stream" element={<StreamView />} />
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
