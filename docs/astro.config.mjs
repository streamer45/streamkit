// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import githubAlertsToStarlightAsides from './src/plugins/github-alerts-to-starlight-asides.mjs';

// https://astro.build/config
export default defineConfig({
	site: 'https://streamkit.dev',
	integrations: [
		starlight({
			title: 'StreamKit',
			description: 'High-performance real-time media processing engine',
			logo: {
				src: './src/assets/logo.png',
				alt: 'StreamKit',
			},
			favicon: '/favicon.ico',
			social: [
				{
					icon: 'github',
					label: 'GitHub',
					href: 'https://github.com/streamer45/streamkit',
				},
			],
			editLink: {
				baseUrl: 'https://github.com/streamer45/streamkit/edit/main/docs/',
			},
			components: {
				ThemeProvider: './src/components/ThemeProvider.astro',
				ThemeSelect: './src/components/ThemeSelect.astro',
			},
			head: [
				{
					tag: 'script',
					attrs: { type: 'module', src: '/scripts/mermaid-renderer.js' },
				},
			],
			sidebar: [
				{
					label: 'Getting Started',
					items: [
						{ label: 'Introduction', slug: 'getting-started/introduction' },
						{ label: 'Quick Start', slug: 'getting-started/quick-start' },
						{ label: 'Installation', slug: 'getting-started/installation' },
					],
				},
				{
					label: 'Architecture',
					items: [{ label: 'Overview', slug: 'architecture/overview' }],
				},
				{
					label: 'Guides',
					items: [
						{ label: 'Creating Pipelines', slug: 'guides/creating-pipelines' },
						{ label: 'Performance Tuning', slug: 'guides/performance' },
						{ label: 'Observability', slug: 'guides/observability' },
						{ label: 'Script Node', slug: 'guides/script-node' },
						{ label: 'Using the Web UI', slug: 'guides/web-ui' },
						{ label: 'Security', slug: 'guides/security' },
						{ label: 'Development Workflow', slug: 'guides/development' },
						{ label: 'Writing Plugins', slug: 'guides/writing-plugins' },
					],
				},
				{
					label: 'Deployment',
					items: [
						{ label: 'Docker', slug: 'deployment/docker' },
						{ label: 'GPU Setup', slug: 'deployment/gpu' },
						{ label: 'systemd', slug: 'deployment/systemd' },
					],
				},
				{
					label: 'Reference',
					autogenerate: { directory: 'reference' },
				},
			],
			customCss: ['./src/styles/custom.css'],
			expressiveCode: {
				themes: ['github-dark', 'github-light'],
				useStarlightDarkModeSwitch: true,
				useStarlightUiThemeColors: true,
				styleOverrides: {
					borderRadius: '0.5rem',
				},
			},
		}),
	],
	markdown: {
		rehypePlugins: [githubAlertsToStarlightAsides],
	},
});
