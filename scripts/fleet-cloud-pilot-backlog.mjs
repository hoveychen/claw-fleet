#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const jsonPath = resolve(root, "docs/fleet-cloud-pilot-backlog.json");
const csvPath = resolve(root, "docs/fleet-cloud-pilot-backlog.csv");
const create = process.argv.includes("--create");
const regenerate = process.argv.includes("--regenerate");

function run(command, args) {
  return execFileSync(command, args, { cwd: root, encoding: "utf8" }).trim();
}

function sourceFiles() {
  return run("rg", ["--files", "-g", "*.rs", "-g", "!target/**"])
    .split("\n")
    .filter(Boolean)
    .sort();
}

function coverageAudits(files) {
  return files
    .filter((file) => !file.includes("/tests/") && !file.includes("/examples/"))
    .filter((file) => !/(^|\/)(test|tests|.*_tests?)\.rs$/.test(file))
    .map((file) => ({ file, source: readFileSync(resolve(root, file), "utf8") }))
    .map(({ file, source }) => ({
      file,
      source,
      lines: source.split("\n").length - (source.endsWith("\n") ? 1 : 0),
    }))
    .filter(({ source, lines }) => lines >= 70 && !source.includes("#[cfg(test)]"))
    .sort((a, b) => b.lines - a.lines || a.file.localeCompare(b.file))
    .slice(0, 86)
    .map(({ file, lines }, index) => {
      const id = `FCP-${String(index + 1).padStart(3, "0")}`;
      return {
        id,
        category: "test-coverage-audit",
        evidence_path: file,
        evidence_line: 1,
        title: `[Pilot backlog] Audit focused test coverage for ${file}`,
        body: [
          `<!-- fleet-pilot-backlog-id: ${id} -->`,
          "## Evidence",
          "",
          `\`${file}\` currently has ${lines} lines and no colocated \`#[cfg(test)]\` module. This is a static, repository-verifiable coverage signal; it does not claim the file has zero integration coverage.`,
          "",
          "## Scope",
          "",
          "Map the module's existing unit/integration coverage, identify risk-bearing branches that are not directly exercised, and add deterministic tests where a real gap exists.",
          "",
          "## Acceptance",
          "",
          `- Existing coverage for \`${file}\` is documented with exact test names.`,
          "- Missing success, failure, and boundary cases receive deterministic tests, or the issue is closed with concrete evidence that existing integration tests are sufficient.",
          "- The relevant package tests pass without enabling network-dependent fixtures.",
        ].join("\n"),
        labels: ["fleet-backlog", "quality", "test-coverage"],
        issue_number: null,
        issue_url: null,
      };
    });
}

function ignoredTests(files, offset) {
  const found = [];
  for (const file of files) {
    const lines = readFileSync(resolve(root, file), "utf8").split("\n");
    for (let index = 0; index < lines.length; index += 1) {
      if (!/^\s*#\[ignore(?:\s*=.*)?\]/.test(lines[index])) continue;
      const functionLine = lines.slice(index + 1, index + 8).find((line) => /\bfn\s+[A-Za-z0-9_]+/.test(line));
      const name = functionLine?.match(/\bfn\s+([A-Za-z0-9_]+)/)?.[1] ?? `ignored_test_at_line_${index + 1}`;
      const reason = lines[index].match(/#\[ignore\s*=\s*"([^"]+)"\]/)?.[1] ?? "no reason recorded";
      const id = `FCP-${String(offset + found.length + 1).padStart(3, "0")}`;
      found.push({
        id,
        category: "ignored-test-automation",
        evidence_path: file,
        evidence_line: index + 1,
        title: `[Pilot backlog] Automate ignored test ${name}`,
        body: [
          `<!-- fleet-pilot-backlog-id: ${id} -->`,
          "## Evidence",
          "",
          `\`${file}:${index + 1}\` marks \`${name}\` ignored: ${reason}.`,
          "",
          "## Scope",
          "",
          "Replace the manual-only dependency with a deterministic fixture or add a documented, scheduled integration lane that executes the test and retains its result.",
          "",
          "## Acceptance",
          "",
          "- The test runs in an identified CI or scheduled validation lane, or is replaced by an equivalent deterministic test.",
          "- Required credentials, network access, timing, and cleanup behavior are documented without committing secrets.",
          "- A failing assertion makes the validation lane fail visibly.",
        ].join("\n"),
        labels: ["fleet-backlog", "quality", "test-coverage"],
        issue_number: null,
        issue_url: null,
      });
    }
  }
  return found;
}

function generate() {
  const files = sourceFiles();
  const audits = coverageAudits(files);
  const ignored = ignoredTests(files, audits.length);
  const items = [...audits, ...ignored];
  if (audits.length !== 86 || ignored.length !== 14 || items.length !== 100) {
    throw new Error(`expected 86 coverage audits + 14 ignored tests = 100; got ${audits.length} + ${ignored.length}`);
  }
  return {
    generated_at: new Date().toISOString(),
    source_commit: run("git", ["rev-parse", "HEAD"]),
    repository: "hoveychen/claw-fleet",
    methodology: "86 production Rust modules >=70 lines without colocated #[cfg(test)] plus all 14 executable #[ignore] tests",
    items,
  };
}

function csv(value) {
  const text = value == null ? "" : String(value);
  return `"${text.replaceAll('"', '""')}"`;
}

function write(dataset) {
  writeFileSync(jsonPath, `${JSON.stringify(dataset, null, 2)}\n`);
  const header = ["id", "category", "evidence_path", "evidence_line", "issue_number", "issue_url", "title"];
  const rows = dataset.items.map((item) => header.map((key) => csv(item[key])).join(","));
  writeFileSync(csvPath, `${header.map(csv).join(",")}\n${rows.join("\n")}\n`);
}

function createIssues(dataset) {
  const existing = JSON.parse(run("gh", ["issue", "list", "--state", "all", "--limit", "200", "--json", "number,url,body,title"]));
  const byId = new Map();
  for (const issue of existing) {
    const id = issue.body?.match(/fleet-pilot-backlog-id:\s*(FCP-\d{3})/)?.[1];
    if (id) byId.set(id, issue);
  }
  for (const item of dataset.items) {
    const matched = byId.get(item.id);
    if (matched) {
      item.issue_number = matched.number;
      item.issue_url = matched.url;
      continue;
    }
    const url = run("gh", [
      "issue",
      "create",
      "--title",
      item.title,
      "--body",
      item.body,
      ...item.labels.flatMap((label) => ["--label", label]),
    ]);
    item.issue_url = url;
    item.issue_number = Number(url.match(/\/(\d+)$/)?.[1]);
    if (!Number.isInteger(item.issue_number)) throw new Error(`cannot parse issue number from ${url}`);
    write(dataset);
  }
}

let dataset;
try {
  if (regenerate) throw new Error("regenerate requested");
  dataset = JSON.parse(readFileSync(jsonPath, "utf8"));
} catch {
  dataset = generate();
}

if (create) createIssues(dataset);
write(dataset);
console.log(`${relative(root, jsonPath)}: ${dataset.items.length} items, ${dataset.items.filter((item) => item.issue_url).length} reconciled`);
