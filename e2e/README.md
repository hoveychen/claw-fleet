# E2E (Task-as-Unit V1)

`task-as-unit.spec.ts` captures the V1 smoke flow. Playwright runner is not yet wired into CI (see the comment block at the top of the spec for the activation steps).

Until the runner lands, treat this folder as executable documentation: the spec describes the exact UI surface the master/worker cluster must produce.

## Future: enabling the smoke

1. `cd claw-fleet-desktop && pnpm add -D @playwright/test`
2. Create `claw-fleet-desktop/playwright.config.ts`:
   ```ts
   import { defineConfig } from '@playwright/test';
   export default defineConfig({
     testDir: '../e2e',
     timeout: 60_000,
     use: { baseURL: 'http://localhost:1420/' },
     webServer: { command: 'pnpm vite', url: 'http://localhost:1420/', reuseExistingServer: true },
   });
   ```
3. Add `fixtures/mock-agent.ts` — a stub `claude` binary that replays a canned dispatch / mark-done event stream when invoked with `--print --append-system-prompt`. Inject via `FLEET_AGENT_BINARY_OVERRIDE` env in the test setup.
4. Drop the `test.skip(...)` to `test(...)` in the spec.
5. Add a CI job that runs `pnpm playwright test`.
