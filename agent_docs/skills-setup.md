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
- `npx skills add` clones the skill into your agent's config directory
- Skills are agent-agnostic — they work with Claude Code, Cursor, Codex,
  Windsurf, and others
- Use `npx skills list` to see installed skills
- Use `npx skills update` to update to latest versions
- Use `npx skills find <query>` to discover new skills

### Important: Install Location

**Do not install skills into `.agents/skills/`** — that directory is
repo-maintained and contains committed skills following the
[Agent Skills specification](https://agentskills.io/specification).
Running `npx skills add` there will mix vendored content into the tracked tree.

Install user/third-party skills into a **personal** (non-tracked) location
instead. For Claude Code, use `~/.claude/skills/` (user-level). For other
agents, use the agent's per-user config directory.

## Repo-Maintained Skills

All committed skills live in **`.agents/skills/`** following the
[Agent Skills specification](https://agentskills.io/specification). Each
`SKILL.md` has YAML frontmatter (`name`, `description`, `license`) on
line 1 and a body that references `guide.md` (symlinked to
`agent_docs/<name>.md`) for progressive disclosure. SPDX compliance is
handled via `REUSE.toml` so that frontmatter remains the first content.

`.claude/skills/` contains backward-compat symlinks into `.agents/skills/`
so Claude Code picks them up from both locations.

When adding a new skill, create it in `.agents/skills/<name>/` with:
1. `SKILL.md` — frontmatter + brief body referencing `guide.md`
2. `guide.md` — symlink to `../../../agent_docs/<name>.md`
3. A symlink in `.claude/skills/<name>` → `../../.agents/skills/<name>`

Learn more: [skills.sh documentation](https://skills.sh/docs)
