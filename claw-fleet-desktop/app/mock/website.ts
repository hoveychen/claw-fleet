/** Website-only fixtures; ordinary mock/QA scenes retain their original data. */
import scenes from '../../../scripts/site/fixtures/scenes.json';
import { MOCK_SESSIONS, MOCK_MESSAGES, MOCK_ARTIFACTS, MOCK_ARTIFACT_USAGE } from './data';
export const websiteLang = new URLSearchParams(location.search).get('website');
export const websiteScene = websiteLang === 'en' || websiteLang === 'zh' ? scenes[websiteLang] : null;
export function installWebsiteFixtures() {
  const c = websiteScene;
  if (!c) return;
  const seed = MOCK_SESSIONS[0];
  MOCK_SESSIONS.splice(0, MOCK_SESSIONS.length, ...c.tasks.map(([title, preview], i) => ({
    ...structuredClone(seed), id: `website-${i}`, aiTitle: title, slug: null,
    workspaceName: c.projects[i % 4], workspacePath: `/Users/demo/workspace/${['launch','research','revenue','brand'][i % 4]}`,
    status: (['waitingInput', 'thinking', 'executing', 'idle'] as const)[i % 4],
    isSubagent: false, parentSessionId: null, runningSubagentCount: 0, watches: [],
    agentSource: (['claude-code', 'codex', 'dsh'] as const)[i % 3], model: ['claude-opus-4-8', 'gpt-5.6-sol', 'gpt-5.6-sol'][i % 3],
    entrypoint: 'fleet', fleetSpawned: true, procAlive: i % 4 !== 3,
    lastMessagePreview: preview, lastActivityMs: Date.now() - (i + 1) * 60000,
    jsonlPath: `/Users/demo/website-${i}.jsonl`, tokenSpeed: 24 + i * 3, agentTokenSpeed: 0,
  })));
  for (const [i, session] of MOCK_SESSIONS.entries()) {
    MOCK_MESSAGES[session.id] = [
      {type: 'user', uuid: `site-user-${i}`, timestamp: new Date().toISOString(), message: {role: 'user', content: i === 0 ? c.brief : c.tasks[i][0]}},
      {type: 'assistant', uuid: `site-answer-${i}`, timestamp: new Date().toISOString(), message: {role: 'assistant', content: [{type: 'text', text: i === 0 ? c.reply : c.tasks[i][1]}]}},
    ];
  }
  const artifactSeed = MOCK_ARTIFACTS[0];
  MOCK_ARTIFACTS.splice(0, MOCK_ARTIFACTS.length, ...c.artifacts.map((title, i) => ({
    ...artifactSeed, id: `website-${websiteLang}-${i}`, name: `deliverable-${i}.${i === 3 || i === 4 ? 'md' : 'html'}`,
    title, note: c.tasks[i][1], kind: i === 3 || i === 4 ? 'text' : 'html', mime: i === 3 || i === 4 ? 'text/markdown' : 'text/html',
    sizeBytes: 4200 + i * 731, workspaceName: c.projects[i % 4], workspacePath: MOCK_SESSIONS[i].workspacePath,
    starred: i < 2, drifted: false, createdMs: Date.now() - i * 3600000,
  })));
  Object.assign(MOCK_ARTIFACT_USAGE, {count: MOCK_ARTIFACTS.length, totalBytes: MOCK_ARTIFACTS.reduce((sum, a) => sum + a.sizeBytes, 0)});
}
