import { describe, beforeAll, beforeEach, afterEach, afterAll, it, expect } from '@ohos/hypium';
import { waitForSessionId } from '../main/ets/common/SpawnConfirm';
import { SessionInfo, isFleetOwnedEntrypoint, canResumeSession, CODEX_FLEET_ORIGINATOR }
  from '../main/ets/models/Types';

// 只需要 id 参与匹配,其余字段用 as 补齐,避免造整个 SessionInfo。
function sess(id: string): SessionInfo {
  return { id } as SessionInfo;
}

export default function localUnitTest() {
  describe('localUnitTest', () => {
    // ── waitForSessionId:ack/reply 都丢时的宽限期兜底(对齐 spawnConfirm.test.ts)──
    it('waitForSessionId_present_immediately', 0, async (done: () => void) => {
      let slept = 0;
      const ok = await waitForSessionId('abc', () => [sess('x'), sess('abc')], {
        sleep: async () => { slept++; },
      });
      expect(ok).assertTrue();
      expect(slept).assertEqual(0);
      done();
    });

    it('waitForSessionId_appears_after_polls', 0, async (done: () => void) => {
      let snapshot: SessionInfo[] = [];
      let ticks = 0;
      const ok = await waitForSessionId('late', () => snapshot, {
        attempts: 10,
        sleep: async () => {
          ticks++;
          if (ticks === 3) { snapshot = [sess('late')]; }
        },
      });
      expect(ok).assertTrue();
      expect(ticks).assertEqual(3);
      done();
    });

    it('waitForSessionId_never_appears', 0, async (done: () => void) => {
      let ticks = 0;
      const ok = await waitForSessionId('never', () => [sess('other')], {
        attempts: 5,
        sleep: async () => { ticks++; },
      });
      expect(ok).assertFalse();
      expect(ticks).assertEqual(5);
      done();
    });

    // ── isFleetOwnedEntrypoint / canResumeSession:Fleet 起的 codex 会话应被认作
    //    自有会话且可续接(对齐 mobile-web types.test.ts)──────────────────────
    it('isFleetOwnedEntrypoint_claude_and_codex', 0, () => {
      expect(isFleetOwnedEntrypoint('claw-fleet-newsession')).assertTrue();
      expect(isFleetOwnedEntrypoint('claw-fleet-handoff')).assertTrue();
      expect(isFleetOwnedEntrypoint(CODEX_FLEET_ORIGINATOR)).assertTrue();
      expect(isFleetOwnedEntrypoint('fleet')).assertTrue();
    });

    it('isFleetOwnedEntrypoint_rejects_external', 0, () => {
      expect(isFleetOwnedEntrypoint('cli')).assertFalse();
      expect(isFleetOwnedEntrypoint('codex_exec')).assertFalse();
      expect(isFleetOwnedEntrypoint(null)).assertFalse();
      expect(isFleetOwnedEntrypoint(undefined)).assertFalse();
    });

    it('canResumeSession_fleet_codex_resumable', 0, () => {
      const codex: SessionInfo = {
        id: 's1', entrypoint: 'fleet', agentSource: 'codex',
        isSubagent: false, procAlive: false, status: 'waitingInput',
      } as SessionInfo;
      expect(canResumeSession(codex)).assertTrue();
      const external: SessionInfo = {
        id: 's2', entrypoint: 'codex_exec', agentSource: 'codex',
        isSubagent: false, procAlive: false, status: 'waitingInput',
      } as SessionInfo;
      expect(canResumeSession(external)).assertFalse();
    });

    // Defines a test suite. Two parameters are supported: test suite name and test suite function.
    beforeAll(() => {
      // Presets an action, which is performed only once before all test cases of the test suite start.
      // This API supports only one parameter: preset action function.
    });
    beforeEach(() => {
      // Presets an action, which is performed before each unit test case starts.
      // The number of execution times is the same as the number of test cases defined by **it**.
      // This API supports only one parameter: preset action function.
    });
    afterEach(() => {
      // Presets a clear action, which is performed after each unit test case ends.
      // The number of execution times is the same as the number of test cases defined by **it**.
      // This API supports only one parameter: clear action function.
    });
    afterAll(() => {
      // Presets a clear action, which is performed after all test cases of the test suite end.
      // This API supports only one parameter: clear action function.
    });
    it('assertContain', 0, () => {
      // Defines a test case. This API supports three parameters: test case name, filter parameter, and test case function.
      let a = 'abc';
      let b = 'b';
      // Defines a variety of assertion methods, which are used to declare expected boolean conditions.
      expect(a).assertContain(b);
      expect(a).assertEqual(a);
    });
  });
}