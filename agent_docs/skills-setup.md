<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Recommended Agent Skills (skills.sh)

[skills.sh](https://skills.sh/) is an open ecosystem for curated agent skills —
reusable procedural knowledge that helps coding agents work more effectively
with specific tools and frameworks.

Install skills with the `skills` CLI (`npx skills`). Skills are installed as
markdown files into your agent's configuration directory.

## Recommended Skills for StreamKit

### For UI Work (React, Frontend)

StreamKit's UI uses React 19, Zustand, Jotai, and Radix UI. These skills
provide best practices for React development:

```bash
# React patterns, hooks, and performance best practices
npx skills add vercel-labs/agent-skills --skill react-best-practices -y

# Component composition and code organization patterns
npx skills add vercel-labs/agent-skills --skill composition-patterns -y

# Visual design and CSS/layout guidelines
npx skills add vercel-labs/agent-skills --skill web-design-guidelines -y
```

### For E2E Testing (Playwright)

StreamKit uses Playwright for end-to-end tests. This skill provides patterns
for browser automation and testing:

```bash
# Playwright-based webapp testing patterns
npx skills add anthropics/skills --skill webapp-testing -y
```

### Install All Recommended Skills at Once

```bash
npx skills add vercel-labs/agent-skills \
  --skill react-best-practices \
  --skill composition-patterns \
  --skill web-design-guidelines -y

npx skills add anthropics/skills --skill webapp-testing -y
```

## How Skills Work

- Skills are stored in GitHub repositories as `SKILL.md` files
- `npx skills add` clones the skill and symlinks it into your agent's config
  directory (e.g., `.claude/skills/`, `.cursor/skills/`)
- Skills are agent-agnostic — they work with Claude Code, Cursor, Codex,
  Windsurf, and others
- Use `npx skills list` to see installed skills
- Use `npx skills update` to update to latest versions
- Use `npx skills find <query>` to discover new skills

## Creating Custom Skills

StreamKit also maintains its own Devin-specific skills in `.agents/skills/` for
testing workflows. If you need to create a new skill for the project, follow the
existing pattern in that directory. For skills that should work across multiple
agents, consider the `SKILL.md` format used by skills.sh.

Learn more: [skills.sh documentation](https://skills.sh/docs)
