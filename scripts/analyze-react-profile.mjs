#!/usr/bin/env node
// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Analyze React DevTools profiling exports (.json) to identify unnecessary
 * re-renders and their cost.
 *
 * Usage:
 *   node scripts/analyze-react-profile.mjs <profile.json> [options]
 *
 * Options:
 *   --top N             Show top N components by wasted time (default: 20)
 *   --commit N          Show details for commit N
 *   --threshold MS      Only show components with total self-time > MS (default: 1)
 *   --filter PATTERN    Only show components matching regex pattern
 *   --cascade           Show the re-render cascade tree for the heaviest commits
 *   --why               Focus on "why did this re-render?" analysis
 *   --summary           Print a short summary only
 *
 * The input file is a JSON export from React DevTools Profiler
 * (Components tab > Profiler > Export).
 *
 * What this script does:
 *   1. Parses the React DevTools profiling JSON (format version 5)
 *   2. Maps fiber IDs to component display names via the snapshots tree
 *   3. For every commit, collects which components re-rendered, their
 *      actual/self durations, and the change descriptions (props/state/
 *      hooks/context that triggered the render)
 *   4. Aggregates per-component stats across all commits
 *   5. Identifies likely-unnecessary re-renders (components that re-render
 *      due to new object references in props, not meaningful data changes)
 *   6. Reports the findings in a human-readable table
 */

import { readFileSync } from "node:fs";

const args = process.argv.slice(2);
const file = args.find((a) => !a.startsWith("--"));
if (!file) {
  console.error(
    "Usage: node scripts/analyze-react-profile.mjs <profile.json> [options]"
  );
  console.error(
    "  --top N           Show top N components (default: 20)"
  );
  console.error(
    "  --threshold MS    Min total self-time to show (default: 1)"
  );
  console.error(
    "  --filter PATTERN  Regex filter on component name"
  );
  console.error(
    "  --commit N        Show detailed breakdown for commit N"
  );
  console.error("  --cascade         Show cascade tree for heaviest commits");
  console.error("  --why             Focus on re-render reasons");
  console.error("  --summary         Print short summary only");
  process.exit(1);
}

function getArg(name, defaultVal) {
  const idx = args.indexOf(name);
  if (idx === -1) return defaultVal;
  return args[idx + 1] ?? defaultVal;
}

const topN = Number(getArg("--top", 20));
const thresholdMs = Number(getArg("--threshold", 1));
const filterPattern = getArg("--filter", null);
const filterRe = filterPattern ? new RegExp(filterPattern, "i") : null;
const singleCommit = getArg("--commit", null);
const showCascade = args.includes("--cascade");
const showWhy = args.includes("--why");
const summaryOnly = args.includes("--summary");

let data;
try {
  data = JSON.parse(readFileSync(file, "utf8"));
} catch (err) {
  console.error(`Failed to read ${file}: ${err.message}`);
  process.exit(1);
}

if (data.version !== 5) {
  console.error(
    `Warning: expected profiling format version 5, got ${data.version}`
  );
}

/**
 * React DevTools profiling format stores component names in `snapshots`.
 * Each snapshot entry is: [commitIndex, { id, children, displayName, ... }]
 */
function buildNameMap(root) {
  const nameMap = new Map();
  for (const value of Object.values(root.snapshots)) {
    const [, info] = value;
    if (info.displayName) {
      nameMap.set(info.id, info.displayName);
    }
  }
  return nameMap;
}

/**
 * Build parent map from snapshots for cascade analysis.
 * Returns Map<childId, parentId>.
 */
function buildParentMap(root) {
  const parentMap = new Map();
  for (const value of Object.values(root.snapshots)) {
    const [, info] = value;
    if (info.children) {
      for (const childId of info.children) {
        parentMap.set(childId, info.id);
      }
    }
  }
  return parentMap;
}

function formatReason(change) {
  if (!change) return "parent re-rendered";
  const parts = [];
  if (change.isFirstMount) return "first mount";
  if (change.props?.length) parts.push(`props=[${change.props.join(", ")}]`);
  if (change.state) parts.push("state changed");
  if (change.didHooksChange) parts.push("hooks changed");
  if (change.hooks?.length) parts.push(`hooks=[${change.hooks.join(", ")}]`);
  if (change.context) parts.push("context changed");
  return parts.length > 0 ? parts.join(", ") : "parent re-rendered";
}

function analyzeRoot(root) {
  const nameMap = buildNameMap(root);
  const parentMap = buildParentMap(root);

  // Per-component aggregated stats
  // key = fiberId, value = { name, renderCount, totalActual, totalSelf, reasons }
  const componentStats = new Map();

  // Per-commit summaries
  const commitSummaries = [];

  for (let ci = 0; ci < root.commitData.length; ci++) {
    const commit = root.commitData[ci];
    const changeMap = new Map(commit.changeDescriptions);
    const actualMap = new Map(commit.fiberActualDurations);
    const selfMap = new Map(commit.fiberSelfDurations);

    let commitTotalSelf = 0;
    let commitComponentCount = 0;
    const commitComponents = [];

    for (const [fiberId, actualDur] of actualMap) {
      const selfDur = selfMap.get(fiberId) ?? 0;
      const name = nameMap.get(fiberId) ?? `unknown_${fiberId}`;
      const change = changeMap.get(fiberId) ?? null;
      const reason = formatReason(change);

      commitTotalSelf += selfDur;
      commitComponentCount++;

      commitComponents.push({
        fiberId,
        name,
        actualDur,
        selfDur,
        reason,
        change,
      });

      // Aggregate
      if (!componentStats.has(fiberId)) {
        componentStats.set(fiberId, {
          name,
          renderCount: 0,
          totalActual: 0,
          totalSelf: 0,
          reasons: new Map(), // reason string -> count
          propsChanged: new Map(), // propName -> count
        });
      }
      const stats = componentStats.get(fiberId);
      stats.renderCount++;
      stats.totalActual += actualDur;
      stats.totalSelf += selfDur;
      stats.reasons.set(reason, (stats.reasons.get(reason) ?? 0) + 1);
      if (change?.props) {
        for (const p of change.props) {
          stats.propsChanged.set(p, (stats.propsChanged.get(p) ?? 0) + 1);
        }
      }
    }

    commitSummaries.push({
      index: ci,
      timestamp: commit.timestamp,
      duration: commit.duration,
      totalSelf: commitTotalSelf,
      componentCount: commitComponentCount,
      components: commitComponents,
      updaters: commit.updaters,
    });
  }

  return { nameMap, parentMap, componentStats, commitSummaries };
}

function pad(str, len) {
  return String(str).padEnd(len);
}

function rpad(str, len) {
  return String(str).padStart(len);
}

function printSummary(analysis) {
  const { commitSummaries, componentStats } = analysis;
  const totalCommits = commitSummaries.length;
  const totalRenderTime = commitSummaries.reduce(
    (s, c) => s + c.totalSelf,
    0
  );
  const uniqueComponents = componentStats.size;
  const avgPerCommit = totalRenderTime / totalCommits;

  console.log("╔══════════════════════════════════════════════════════╗");
  console.log("║           React Profile Analysis Summary            ║");
  console.log("╠══════════════════════════════════════════════════════╣");
  console.log(
    `║  Commits:        ${rpad(totalCommits, 6)}                         ║`
  );
  console.log(
    `║  Components:     ${rpad(uniqueComponents, 6)}                         ║`
  );
  console.log(
    `║  Total self-time: ${rpad(totalRenderTime.toFixed(1) + "ms", 10)}                    ║`
  );
  console.log(
    `║  Avg per commit:  ${rpad(avgPerCommit.toFixed(1) + "ms", 10)}                    ║`
  );
  console.log("╚══════════════════════════════════════════════════════╝");

  // Heaviest commits
  const sorted = [...commitSummaries].sort(
    (a, b) => b.totalSelf - a.totalSelf
  );
  console.log("\n── Heaviest commits ──");
  console.log(
    `${"Commit".padEnd(8)} ${"Self-time".padStart(10)} ${"Components".padStart(11)} ${"Timestamp".padStart(12)}`
  );
  for (const c of sorted.slice(0, 5)) {
    console.log(
      `${String(c.index).padEnd(8)} ${(c.totalSelf.toFixed(1) + "ms").padStart(10)} ${String(c.componentCount).padStart(11)} ${(c.timestamp.toFixed(0) + "ms").padStart(12)}`
    );
  }
}

function printTopComponents(analysis) {
  const { componentStats } = analysis;

  let entries = [...componentStats.values()];

  // Apply filters
  if (filterRe) {
    entries = entries.filter((e) => filterRe.test(e.name));
  }
  entries = entries.filter((e) => e.totalSelf >= thresholdMs);

  // Sort by total self-time descending
  entries.sort((a, b) => b.totalSelf - a.totalSelf);

  console.log(
    `\n── Top ${topN} components by total self-time (threshold: ${thresholdMs}ms) ──`
  );
  console.log(
    `${"Component".padEnd(40)} ${"Renders".padStart(8)} ${"Total".padStart(10)} ${"Avg".padStart(8)} ${"Primary reason".padEnd(40)}`
  );
  console.log("─".repeat(110));

  for (const entry of entries.slice(0, topN)) {
    const primaryReason = [...entry.reasons.entries()].sort(
      (a, b) => b[1] - a[1]
    )[0];
    const reasonStr = primaryReason
      ? `${primaryReason[0]} (${primaryReason[1]}x)`
      : "";

    console.log(
      `${pad(entry.name, 40)} ${rpad(entry.renderCount, 8)} ${rpad(entry.totalSelf.toFixed(1) + "ms", 10)} ${rpad((entry.totalSelf / entry.renderCount).toFixed(1) + "ms", 8)} ${reasonStr}`
    );
  }
}

function printWhyAnalysis(analysis) {
  const { componentStats } = analysis;

  console.log("\n── Why did components re-render? ──");
  console.log(
    "Components that re-rendered most often due to prop changes (potential object-reference issues):\n"
  );

  let entries = [...componentStats.values()];
  if (filterRe) entries = entries.filter((e) => filterRe.test(e.name));

  // Find components with lots of prop-driven re-renders
  const propDriven = entries
    .filter((e) => {
      const propReasons = [...e.reasons.entries()].filter(([r]) =>
        r.startsWith("props=")
      );
      return propReasons.reduce((s, [, c]) => s + c, 0) > 2;
    })
    .sort((a, b) => b.totalSelf - a.totalSelf);

  for (const entry of propDriven.slice(0, 15)) {
    console.log(`  ${entry.name} (${entry.renderCount} renders, ${entry.totalSelf.toFixed(1)}ms total)`);

    // Show which props changed
    if (entry.propsChanged.size > 0) {
      const sorted = [...entry.propsChanged.entries()].sort(
        (a, b) => b[1] - a[1]
      );
      console.log(
        `    Props changed: ${sorted.map(([p, c]) => `${p} (${c}x)`).join(", ")}`
      );
    }

    // Show reasons
    for (const [reason, count] of [...entry.reasons.entries()].sort(
      (a, b) => b[1] - a[1]
    )) {
      console.log(`    ${count}x: ${reason}`);
    }
    console.log();
  }

  // Also show components re-rendering from "parent re-rendered" (cascade victims)
  console.log("Components re-rendered only because their parent re-rendered (cascade victims):\n");
  const cascadeVictims = entries
    .filter((e) => {
      const parentReason = e.reasons.get("parent re-rendered") ?? 0;
      return parentReason === e.renderCount && e.totalSelf >= thresholdMs;
    })
    .sort((a, b) => b.totalSelf - a.totalSelf);

  for (const entry of cascadeVictims.slice(0, 10)) {
    console.log(
      `  ${entry.name}: ${entry.renderCount} renders, ${entry.totalSelf.toFixed(1)}ms total (pure cascade waste)`
    );
  }
}

function printCommitDetail(analysis, commitIdx) {
  const summary = analysis.commitSummaries[commitIdx];
  if (!summary) {
    console.error(`Commit ${commitIdx} not found (have ${analysis.commitSummaries.length} commits)`);
    return;
  }

  console.log(`\n── Commit ${commitIdx} detail ──`);
  console.log(`  Timestamp: ${summary.timestamp.toFixed(0)}ms`);
  console.log(`  Total self-time: ${summary.totalSelf.toFixed(1)}ms`);
  console.log(`  Components: ${summary.componentCount}`);
  if (summary.updaters?.length) {
    const updaterNames = summary.updaters.map(
      (u) => analysis.nameMap.get(u.id) ?? `unknown_${u.id}`
    );
    console.log(`  Triggered by: ${updaterNames.join(", ")}`);
  }

  console.log(
    `\n  ${"Component".padEnd(40)} ${"Self".padStart(8)} ${"Actual".padStart(8)} ${"Reason"}`
  );
  console.log("  " + "─".repeat(108));

  // Sort by self-duration descending
  const sorted = [...summary.components].sort(
    (a, b) => b.selfDur - a.selfDur
  );

  for (const comp of sorted) {
    if (filterRe && !filterRe.test(comp.name)) continue;
    console.log(
      `  ${pad(comp.name, 40)} ${rpad(comp.selfDur.toFixed(1) + "ms", 8)} ${rpad(comp.actualDur.toFixed(1) + "ms", 8)} ${comp.reason}`
    );
  }
}

function printCascade(analysis) {
  const { commitSummaries, nameMap, parentMap } = analysis;

  // Find the heaviest commits
  const heaviest = [...commitSummaries]
    .sort((a, b) => b.totalSelf - a.totalSelf)
    .slice(0, 3);

  for (const commit of heaviest) {
    console.log(`\n── Cascade tree for commit ${commit.index} (${commit.totalSelf.toFixed(1)}ms) ──`);

    // Build a tree of re-rendered components
    const renderedIds = new Set(commit.components.map((c) => c.fiberId));
    const compMap = new Map(commit.components.map((c) => [c.fiberId, c]));

    // Find roots of the cascade (re-rendered components whose parent was NOT re-rendered)
    const roots = commit.components.filter((c) => {
      const parentId = parentMap.get(c.fiberId);
      return !parentId || !renderedIds.has(parentId);
    });

    function printTree(fiberId, indent) {
      const comp = compMap.get(fiberId);
      if (!comp) return;
      const prefix = indent === 0 ? "→ " : "  " + "│ ".repeat(indent - 1) + "├─";
      const selfStr = comp.selfDur > 0.1 ? ` (${comp.selfDur.toFixed(1)}ms)` : "";
      console.log(`${prefix}${comp.name}${selfStr} — ${comp.reason}`);

      // Find children that also re-rendered (using parent map in reverse)
      const children = [...renderedIds].filter(
        (id) => parentMap.get(id) === fiberId
      );
      for (const childId of children) {
        printTree(childId, indent + 1);
      }
    }

    for (const root of roots.sort((a, b) => b.actualDur - a.actualDur).slice(0, 5)) {
      printTree(root.fiberId, 0);
      console.log();
    }
  }
}

for (const root of data.dataForRoots) {
  const analysis = analyzeRoot(root);

  if (summaryOnly) {
    printSummary(analysis);
    continue;
  }

  printSummary(analysis);

  if (singleCommit !== null) {
    printCommitDetail(analysis, Number(singleCommit));
  } else {
    printTopComponents(analysis);
  }

  if (showWhy) {
    printWhyAnalysis(analysis);
  }

  if (showCascade) {
    printCascade(analysis);
  }
}
