/** Website-only fixtures; ordinary mock/QA scenes retain their original data. */
import { useComposerDraftStore } from '../composerDraft';
import assets from '../../../scripts/site/fixtures/assets.json';
import reviewEn from '../../../scripts/site/fixtures/en/review.html?raw';
import reviewZh from '../../../scripts/site/fixtures/zh/review.html?raw';
import scenes from '../../../scripts/site/fixtures/scenes.json';
import { MOCK_SESSIONS, MOCK_MESSAGES, MOCK_ARTIFACTS, MOCK_ARTIFACT_USAGE } from './data';
export const websiteLang = new URLSearchParams(location.search).get('website');
export const websiteScene = websiteLang === 'en' || websiteLang === 'zh' ? scenes[websiteLang] : null;
export const websiteReview = websiteLang === "zh" ? reviewZh : reviewEn;
const projects = ["ember-coffee","customer-research","home-studio","balcony-garden","city-walks","app-workshop"];
export function installWebsiteFixtures() {
  const c = websiteScene;
  if (!c) return;
  const seed = MOCK_SESSIONS[0];
  MOCK_SESSIONS.splice(0, MOCK_SESSIONS.length, ...c.tasks.map(([title, preview], i) => ({
    ...structuredClone(seed), id: `website-${i}`, aiTitle: title, slug: null,
    workspaceName: c.projects[i % 6], workspacePath: `/Users/demo/workspace/${projects[i % 6]}`,
    status: (['waitingInput', 'thinking', 'executing', 'idle'] as const)[i % 4],
    isSubagent: false, parentSessionId: null, runningSubagentCount: 0, watches: [],
    agentSource: (['claude-code', 'codex', 'dsh'] as const)[i % 3], model: ['claude-opus-4-8', 'gpt-5.6-sol', 'deepseek/deepseek-v4-pro'][i % 3],
    entrypoint: 'fleet', fleetSpawned: true, procAlive: i % 4 !== 3,
    lastMessagePreview: preview, lastActivityMs: Date.now() - (i + 1) * 60000,
    jsonlPath: `/Users/demo/website-${i}.jsonl`, tokenSpeed: 24 + i * 3, agentTokenSpeed: 0,
    totalOutputTokens: 12800 + i * 7350, totalCostUsd: 0.62 + i * 0.37, agentTotalCostUsd: 0, contextPercent: 0.18 + (i % 5) * 0.13,
  })));
  for (const [i, session] of MOCK_SESSIONS.entries()) {
    MOCK_MESSAGES[session.id] = [
      {type: 'user', uuid: `site-user-${i}`, timestamp: new Date().toISOString(), message: {role: 'user', content: i === 0 ? c.brief : c.tasks[i][0]}},
      {type: 'assistant', uuid: `site-answer-${i}`, timestamp: new Date().toISOString(), message: {role: 'assistant', content: [{type: 'text', text: i === 0 ? c.reply : c.tasks[i][1]}]}},
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
    tool: 'claude', model: 'claude-opus-4-8', effort: 'high', permissionMode: 'acceptEdits',
    attachments: [
      {path:'/Users/demo/workspace/ember-coffee/brand-brief.pdf',name:websiteLang==='zh'?'品牌简报.pdf':'Brand brief.pdf'},
      {path:'/Users/demo/workspace/ember-coffee/budget.xlsx',name:websiteLang==='zh'?'开店预算.xlsx':'Launch budget.xlsx'},
      {path:'/Users/demo/workspace/ember-coffee/references.png',name:websiteLang==='zh'?'包装参考.png':'Packaging reference.png',previewUrl:`/artifact_blob?id=website-${websiteLang}-4`},
    ],
  });
  Object.assign(MOCK_ARTIFACT_USAGE, {count: MOCK_ARTIFACTS.length, totalBytes: MOCK_ARTIFACTS.reduce((sum, a) => sum + a.sizeBytes, 0)});
}
