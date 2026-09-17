#!/usr/bin/env python3
"""Aggregate the ablation runs and attach exact p-values, so a difference is
only ever reported as real when the sample actually supports it."""
import json, math
from collections import defaultdict
from pathlib import Path

RUNS = Path.home() / ".guidance-probe" / "runs"
RULE = {
    "A": "Rule 3 — 生产代码改动走 worktree",
    "B": "Rule 2 — 多步任务先写 TASKS.md 计划",
    "C": "Rule 1 — 计划中途不在 main 上提交",
    "D": "周期任务用 fleet loop，不用 CronCreate",
    "E": "跨回合等待用 fleet watch，不空转",
    "F": "终局回合出一张格式正确的决策卡",
}
# Measured with the delta method (see REPORT.md §2). `full` reproduced within
# 0.02% across two days (24,381 then 24,377), so these are stable.
# `minprd` / `minmode` cross the two files: which one's bulk actually costs
# compliance? Re-measuring `min` and `shipped` alongside them reproduced within
# ~1% (8,086 vs 7,993; 19,649 vs 19,558), so the delta method is stable.
TOKENS = {"none": 0, "full": 24377, "shipped": 19558, "minmode": 19052,
          "lite": 14607, "newship": 10932, "minprd": 8690, "min": 7993}


def fisher(a, b, c, d):
    """Two-sided Fisher exact test on [[a,b],[c,d]]."""
    def p(a, b, c, d):
        n = a + b + c + d
        return (math.comb(a + b, a) * math.comb(c + d, c)) / math.comb(n, a + c)
    obs = p(a, b, c, d)
    tot = 0.0
    for i in range(0, a + b + 1):
        j, k, l = a + b - i, a + c - i, d - a + i
        if k < 0 or l < 0:
            continue
        pr = p(i, j, k, l)
        if pr <= obs + 1e-12:
            tot += pr
    return min(1.0, tot)


cells = defaultdict(lambda: [0, 0])  # (scen, model, cond) -> [pass, total]
for f in sorted(RUNS.glob("*/record.json")):
    r = json.loads(f.read_text())
    if r.get("api_error"):
        continue
    k = (r["scenario"], r["model"], r["condition"])
    cells[k][1] += 1
    cells[k][0] += bool(r["score"].get("compliant"))

for model in sorted({k[1] for k in cells}):
    conds = [c for c in ("none", "full", "shipped", "minmode", "lite", "newship", "minprd", "min")
             if any(k[2] == c and k[1] == model for k in cells)]
    print(f"\n### model = {model}\n")
    print("| 场景 | 规则 | " + " | ".join(conds) + " |")
    print("|---|---|" + "---|" * len(conds))
    tot = {c: [0, 0] for c in conds}
    for s in sorted({k[0] for k in cells if k[1] == model}):
        row = []
        for c in conds:
            p, n = cells.get((s, model, c), [0, 0])
            row.append(f"{p}/{n}" if n else "—")
            tot[c][0] += p
            tot[c][1] += n
        print(f"| {s} | {RULE[s]} | " + " | ".join(row) + " |")
    print("| **合计** | | " + " | ".join(
        f"**{tot[c][0]}/{tot[c][1]}**" for c in conds) + " |")

    print("\n各条件对 `full` 的差异（Fisher 精确检验，双侧）：")
    for c in conds:
        if c == "full":
            continue
        for s in sorted({k[0] for k in cells if k[1] == model}):
            fp, fn = cells.get((s, model, "full"), [0, 0])
            xp, xn = cells.get((s, model, c), [0, 0])
            if not fn or not xn or fp == xp and fn == xn:
                continue
            pv = fisher(xp, xn - xp, fp, fn - fp)
            mark = "**显著**" if pv < 0.05 else "不显著"
            print(f"- {s} {c} {xp}/{xn} vs full {fp}/{fn} → p={pv:.3f} ({mark})")

print("\n### 条件成本（实测 token 增量，对空 CLAUDE.md）\n")
print("| 条件 | token | 相对 full |")
print("|---|---|---|")
for c, t in TOKENS.items():
    rel = "—" if c == "none" else f"{(t / TOKENS['full'] - 1) * 100:+.1f}%"
    print(f"| {c} | {t:,} | {rel} |")

# ---- pooled test over the non-saturated scenarios -------------------------
# A, C and D sit at 100% (or 0%) under every condition that carries the rule,
# so they can only dilute a comparison. Pool the three that actually vary.
POOL = ("B", "E", "F")
print("\n### 非饱和场景 (B,E,F) 合并对比\n")
# Only conditions measured on ALL of B, E and F may be pooled. `shipped`,
# `minmode` and `minprd` were run on F alone, so pooling them would compare
# "F only" against "B+E+F" and read as a collapse (or a win) that is really
# just a different scenario mix.
pooled, partial = {}, []
for c in ("none", "full", "shipped", "minmode", "lite", "newship", "minprd", "min"):
    p = sum(cells.get((s, "sonnet", c), [0, 0])[0] for s in POOL)
    n = sum(cells.get((s, "sonnet", c), [0, 0])[1] for s in POOL)
    if not all(cells.get((s, "sonnet", c), [0, 0])[1] for s in POOL):
        if n:
            partial.append(c)
        continue
    pooled[c] = (p, n)
    print(f"- {c}: {p}/{n} = {p / n * 100:.0f}%")
if partial:
    print(f"- 未纳入合并（只跑了 F，场景组成不可比）: {', '.join(partial)}")
fp, fn = pooled["full"]
for c in ("none", "shipped", "minmode", "lite", "newship", "minprd", "min"):
    if c not in pooled:
        continue
    xp, xn = pooled[c]
    pv = fisher(xp, xn - xp, fp, fn - fp)
    print(f"- {c} vs full → p={pv:.3f} " + ("(**显著**)" if pv < 0.05 else "(不显著)"))
