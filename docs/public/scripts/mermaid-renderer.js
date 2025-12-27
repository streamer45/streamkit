// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
// SPDX-License-Identifier: MPL-2.0

const MERMAID_SRC = '/vendor/mermaid.min.js';

function getTheme() {
  return document.documentElement.dataset.theme === 'light' ? 'light' : 'dark';
}

function getCssVar(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function loadScript(src) {
  return new Promise((resolve, reject) => {
    const existing = document.querySelector(`script[src="${src}"]`);
    if (existing) {
      if (existing.dataset.loaded === 'true') return resolve();
      existing.addEventListener('load', resolve, { once: true });
      existing.addEventListener('error', reject, { once: true });
      return;
    }

    const script = document.createElement('script');
    script.src = src;
    script.async = true;
    script.addEventListener('load', () => {
      script.dataset.loaded = 'true';
      resolve();
    });
    script.addEventListener('error', reject, { once: true });
    document.head.appendChild(script);
  });
}

async function getMermaid() {
  if (window.mermaid) return window.mermaid;
  await loadScript(MERMAID_SRC);
  return window.mermaid;
}

function initMermaidForTheme(theme) {
  const mermaid = window.mermaid;
  if (!mermaid) return;

  const skBg = getCssVar('--sk-bg');
  const skSidebarBg = getCssVar('--sk-sidebar-bg');
  const skPanelBg = getCssVar('--sk-panel-bg');
  const skText = getCssVar('--sk-text');
  const skTextMuted = getCssVar('--sk-text-muted');
  const skBorder = getCssVar('--sk-border');

  mermaid.initialize({
    startOnLoad: false,
    theme: theme === 'light' ? 'default' : 'dark',
    fontFamily: 'var(--__sl-font)',
    themeVariables: {
      background: 'transparent',
      primaryColor: skPanelBg,
      primaryTextColor: skText,
      primaryBorderColor: skBorder,
      lineColor: skTextMuted,
      secondaryColor: skSidebarBg,
      tertiaryColor: skBg,
    },
  });
}

async function renderMermaidDiagrams({ force = false } = {}) {
  const wrappers = document.querySelectorAll('figure.sk-mermaid');
  if (wrappers.length === 0) return;

  await getMermaid();
  if (!window.mermaid) return;

  const theme = getTheme();
  if (force || window.__skMermaidTheme !== theme) {
    initMermaidForTheme(theme);
    window.__skMermaidTheme = theme;
  }

  const mermaid = window.mermaid;

  for (const wrapper of wrappers) {
    if (!(wrapper instanceof HTMLElement)) continue;

    const source = wrapper.querySelector('pre.sk-mermaid-source');
    const target = wrapper.querySelector('.sk-mermaid-target');
    if (!(source instanceof HTMLElement) || !(target instanceof HTMLElement)) continue;

    if (!force && wrapper.dataset.mermaidTheme === theme && wrapper.dataset.rendered === 'true') continue;

    const diagram = source.textContent ?? '';
    const renderKey = `sk-mermaid-${Math.random().toString(16).slice(2)}`;

    try {
      const { svg, bindFunctions } = await mermaid.render(renderKey, diagram);
      target.innerHTML = svg;
      bindFunctions?.(target);
      wrapper.dataset.rendered = 'true';
      wrapper.dataset.mermaidTheme = theme;
    } catch (err) {
      console.error('Mermaid render failed', err);
      wrapper.dataset.rendered = 'error';
      wrapper.dataset.mermaidTheme = theme;
    }
  }
}

function installThemeObserver() {
  const root = document.documentElement;
  const observer = new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      if (mutation.type === 'attributes' && mutation.attributeName === 'data-theme') {
        void renderMermaidDiagrams({ force: true });
        return;
      }
    }
  });
  observer.observe(root, { attributes: true, attributeFilter: ['data-theme'] });
}

document.addEventListener('astro:page-load', () => {
  void renderMermaidDiagrams();
});

installThemeObserver();
if (document.readyState === 'loading') {
  document.addEventListener(
    'DOMContentLoaded',
    () => {
      void renderMermaidDiagrams();
    },
    { once: true }
  );
} else {
  void renderMermaidDiagrams();
}
