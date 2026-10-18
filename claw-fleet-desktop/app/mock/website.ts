/** Website-only fixtures; ordinary mock/QA scenes retain their original data. */
import { useComposerDraftStore } from '../composerDraft';
import assets from '../../../scripts/site/fixtures/assets.json';
import reviewEn from '../../../scripts/site/fixtures/en/review.html?raw';
import reviewZh from '../../../scripts/site/fixtures/zh/review.html?raw';
import scenes from '../../../scripts/site/fixtures/scenes.json';
import { MOCK_SESSIONS, MOCK_MESSAGES, MOCK_ARTIFACTS, MOCK_ARTIFACT_USAGE, MOCK_TASK_PLANS } from './data';
export const websiteLang = new URLSearchParams(location.search).get('website');
export const websiteScene = websiteLang === 'en' || websiteLang === 'zh' ? scenes[websiteLang] : null;
export const websiteReview = websiteLang === "zh" ? reviewZh : reviewEn;
const projects = ["ember-coffee","customer-research","home-studio","balcony-garden","city-walks","app-workshop"];
export function installWebsiteFixtures() {
  const c = websiteScene;
  if (!c) return;
  const seed = MOCK_SESSIONS[0];
  const now = Date.now();
  const co = c.coordinator;
  const sources = ['claude-code', 'codex', 'dsh'] as const;
  const models = ['claude-opus-5-5', 'gpt-5.6-sol', 'deepseek-official/deepseek-flash'];
  // Every home section gets rows: task 0 (Ember) needs you through a sub-task's
  // card, then running, waiting, to-accept and done tasks across the projects.
  type Shape = { status: string; alive: boolean; outcome?: 'completed'; plan?: [number, number]; watch?: boolean };
  const shapeOf = (i: number): Shape =>
    i === 0 ? { status: 'watching', alive: false, plan: [co.stepsDone, co.steps.length] }
    : i <= 2 ? { status: i === 1 ? 'thinking' : 'executing', alive: true, plan: [i + 1, 5] }
    : i === 3 ? { status: 'watching', alive: false, watch: true }
    : i <= 5 ? { status: 'idle', alive: false, plan: [4, 4] }
    : { status: 'idle', alive: false, outcome: 'completed' };
  const session = (id: string, i: number, title: string, preview: string, extra: Record<string, unknown>) => ({
    ...structuredClone(seed), id, aiTitle: title, slug: null,
    workspaceName: c.projects[i % 6], workspacePath: `/Users/demo/workspace/${projects[i % 6]}`,
    isSubagent: false, parentSessionId: null, runningSubagentCount: 0, watches: [],
    agentSource: sources[i % 3], model: models[i % 3],
    entrypoint: 'fleet', fleetSpawned: true,
    lastMessagePreview: preview, lastActivityMs: now - (i + 1) * 60000, createdAtMs: now - (i + 2) * 47 * 60000,
    jsonlPath: `/Users/demo/${id}.jsonl`, tokenSpeed: 24 + i * 3, agentTokenSpeed: 0,
    totalOutputTokens: 12800 + i * 7350, totalCostUsd: 0.62 + i * 0.37, agentTotalCostUsd: 0, contextPercent: 0.18 + (i % 5) * 0.13,
    handoff: null, taskId: id, parentTaskId: null, taskOutcome: null, taskPlan: null,
    ...extra,
  });
  MOCK_SESSIONS.splice(0, MOCK_SESSIONS.length, ...c.tasks.map(([title, preview], i) => {
    const sh = shapeOf(i);
    return session(`website-${i}`, i, title, preview, {
      status: sh.status, procAlive: sh.alive, taskOutcome: sh.outcome ?? null,
      taskPlan: sh.plan ? { done: sh.plan[0], total: sh.plan[1], currentPlan: null, planId: 'ember-launch', currentTask: null } : null,
      watches: sh.watch ? [{ id: `w-${i}`, note: co.waitNote, created: now - 20 * 60000, pollSecs: 60, deadlineAt: now + 3600000, pollCount: 20, structuralFailStreak: 0 }] : [],
      ...(i === 0 ? { agentSource: 'claude-code', model: models[0] } : {}),
    });
  }));
  // Ember's three sub-tasks, one per agent tool: packaging (its card is the
  // pinned decision), the website still building, the deck delivered.
  const subStatus = [['waitingInput', true], ['executing', true], ['idle', false]] as const;
  MOCK_SESSIONS.push(...co.subtasks.map(([title, preview]: string[], k: number) =>
    session(`website-sub-${k}`, 0, title, preview, {
      agentSource: (['dsh', 'codex', 'claude-code'] as const)[k], model: [models[2], models[1], models[0]][k],
      status: subStatus[k][0], procAlive: subStatus[k][1], taskId: `website-sub-${k}`, parentTaskId: 'website-0',
      fleetSpawned: true, lastActivityMs: now - (k + 1) * 4 * 60000,
    })));
  MOCK_TASK_PLANS.splice(0, MOCK_TASK_PLANS.length, {
    id: 'ember-launch', title: c.tasks[0][0], source: null, kind: 'exec',
    items: co.steps.map((text: string, k: number) => ({ text, done: k < co.stepsDone })),
  });
  for (const [i, session] of MOCK_SESSIONS.entries()) {
    const [ask, answer] = i === 0 ? [c.brief, c.reply] : [session.aiTitle, session.lastMessagePreview];
    MOCK_MESSAGES[session.id] = [
      {type: 'user', uuid: `site-user-${i}`, timestamp: new Date().toISOString(), message: {role: 'user', content: ask}},
      {type: 'assistant', uuid: `site-answer-${i}`, timestamp: new Date().toISOString(), message: {role: 'assistant', content: [{type: 'text', text: answer}]}},
    ];
  }
  const artifactSeed = MOCK_ARTIFACTS[0];
  const files = assets[websiteLang as 'en' | 'zh'];
  MOCK_ARTIFACTS.splice(0, MOCK_ARTIFACTS.length, ...c.artifacts.map((title, i) => ({
    ...artifactSeed, ...files[i], title, note: '',
    workspaceName: c.projects[[0,0,0,4,0,3,0,5][i]], workspacePath: `/Users/demo/workspace/${projects[[0,0,0,4,0,3,0,5][i]]}`,
    starred: i < 2, drifted: false, createdMs: Date.now() - i * 3600000,
  })));
  useComposerDraftStore.getState().patchDraft('new', {
    workspace: '/Users/demo/workspace/ember-coffee', prompt: c.brief,
    tool: 'claude', model: 'claude-opus-5-5', effort: 'high', permissionMode: 'acceptEdits',
    attachments: [
      {path:'/Users/demo/workspace/ember-coffee/brand-brief.pdf',name:websiteLang==='zh'?'品牌简报.pdf':'Brand brief.pdf'},
      {path:'/Users/demo/workspace/ember-coffee/budget.xlsx',name:websiteLang==='zh'?'开店预算.xlsx':'Launch budget.xlsx'},
      {path:'/Users/demo/workspace/ember-coffee/references.png',name:websiteLang==='zh'?'包装参考.png':'Packaging reference.png',previewUrl:`/artifact_blob?id=website-${websiteLang}-4`},
    ],
  });
  Object.assign(MOCK_ARTIFACT_USAGE, {count: MOCK_ARTIFACTS.length, totalBytes: MOCK_ARTIFACTS.reduce((sum, a) => sum + a.sizeBytes, 0)});
}
