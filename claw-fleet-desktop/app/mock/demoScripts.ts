/**
 * Relay screenplays for the promo screencast (plan `promo-mock-demo`).
 *
 * One RawMessage[] per hop of the `chain-grpc` handoff relay. The streaming
 * engine in tauri-mock.ts replays a screenplay when its session's detail view
 * is opened: it streams each assistant turn's `thinking` through
 * read_live_thinking, then appends the full message via a `session-tail` event.
 *
 * Voice is lifted from the real claude-fleet corpus — the model reasons in
 * English even on Chinese prompts — so the thinking reads like an actual
 * session, just on a fresh project (`aurora-platform`, a REST→gRPC migration).
 */
import type { ContentBlock, RawMessage } from "../types";

let seq = 0;
const uid = (h: string) => `grpc-${h}-${(seq += 1)}`;

const think = (s: string): ContentBlock => ({ type: "thinking", thinking: s });
const text = (s: string): ContentBlock => ({ type: "text", text: s });
const use = (id: string, name: string, input: Record<string, unknown>): ContentBlock => ({
  type: "tool_use", id, name, input,
});

/** assistant turn */
function A(hop: string, blocks: ContentBlock[], out = 400): RawMessage {
  return {
    type: "assistant",
    uuid: uid(hop),
    message: {
      role: "assistant",
      model: hop === "5" ? "claude-opus-4-8" : "claude-fable-5",
      content: blocks,
      stop_reason: blocks.some((b) => b.type === "tool_use") ? "tool_use" : "end_turn",
      usage: { input_tokens: 3200, output_tokens: out },
    },
  };
}
/** user / injected prompt */
function U(hop: string, content: string): RawMessage {
  return { type: "user", uuid: uid(hop), message: { role: "user", content } };
}
/** tool_result carrier (real transcripts use type:"user" for these) */
function R(hop: string, toolUseId: string, content: string, isError = false): RawMessage {
  return {
    type: "user",
    uuid: uid(hop),
    message: { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content, is_error: isError }] },
  };
}

// ── P1 — audit ──────────────────────────────────────────────────────────────
const P1: RawMessage[] = [
  U("1", "New project: aurora-platform. I want to migrate our whole API layer from REST to gRPC — 43-ish endpoints — but we can't break existing clients, so keep a REST shim in front. This is big; take it as far as one session can, then hand off."),
  A("1", [
    think(
      "This is a large migration, definitely multi-session. Before writing any proto or touching a handler I need ground truth: exactly which REST endpoints exist, who calls them, and which ones have shapes that don't map cleanly to unary RPCs (streaming, file upload, long-poll). Guessing the endpoint count would be the classic mistake here — I'll enumerate them from the router, not from memory.\n\nLet me start by finding every route registration across the codebase.",
    ),
    text("Big migration — this is P1 of a relay. I'll start with an honest inventory of every REST endpoint and its callers before anyone designs a proto. Enumerating from the router, not guessing."),
    use("t-1a", "Bash", { command: "rg -n \"router\\.(Get|Post|Put|Delete|Patch)\\(\" src/api --no-heading | wc -l && rg -n \"router\\.(Get|Post|Put|Delete|Patch)\\(\" src/api -o | sort -u | head", description: "Count and sample REST route registrations" }),
  ], 520),
  R("1", "t-1a", "43\nrouter.Get(\nrouter.Post(\nrouter.Put(\nrouter.Delete(\nrouter.Patch("),
  A("1", [
    think("43 endpoints — matches the ask. Now which of these are not plain request/response? The reports endpoint streams NDJSON, and there's a file upload. Those are the ones that'll bite a naive unary mapping. Let me look for streaming responses and multipart handlers specifically."),
    use("t-1b", "Grep", { pattern: "Flush\\(\\)|multipart|Transfer-Encoding|text/event-stream|application/x-ndjson", path: "src/api", output_mode: "files_with_matches" }),
  ], 300),
  R("1", "t-1b", "src/api/reports.go\nsrc/api/exports.go\nsrc/api/uploads.go"),
  A("1", [
    think("Three special cases: reports.go (NDJSON stream → must be a server-streaming RPC, not unary), exports.go (also streams), uploads.go (multipart → client-streaming or a signed-URL side-channel). I also need the client side — who consumes /v1/reports as a stream, and does the mobile client pin TLS? If it does, the shim must preserve the exact cert chain or mobile breaks. Let me grep the clients."),
    use("t-1c", "Grep", { pattern: "/v1/reports|tls\\.Config|PinnedCertificates|certPin", path: "clients", output_mode: "content", "-n": true }),
  ], 300),
  R("1", "t-1c", "clients/mobile/api.ts:88: // NOTE: cert-pinned — see PinnedCertificates below\nclients/mobile/api.ts:141: streamReports(): AsyncIterable<Row> { /* consumes NDJSON */ }\nclients/web/api.ts:52: streamReports()\nclients/cli/main.go:210: pinnedCerts := x509..."),
  A("1", [
    text("Inventory complete. 43 endpoints, 6 clients. The three that need care: `reports` and `exports` stream (→ server-streaming RPCs) and `uploads` is multipart. The mobile + CLI clients pin TLS, so the shim must keep the same cert chain. I'll publish the full audit to the wiki and hand off — my context is nearly full."),
    use("t-1d", "mcp__fleet__fleet__wiki", { action: "publish", slug: "grpc/audit", title: "REST→gRPC migration — endpoint audit" }),
  ], 340),
  R("1", "t-1d", "published wiki: grpc/audit (v1)"),
  A("1", [
    think("Context is at ~96%. The right move is a clean handoff with the load-bearing gotchas front and center, not to grind out P2 in a nearly-full window. I'll register the handoff with the streaming + cert-pinning traps called out explicitly, and point the successor at P2 (proto design)."),
    use("t-1e", "mcp__fleet__fleet__handoff", {
      note: "P1 done — 43 REST endpoints inventoried across 6 clients (wiki: grpc/audit). Gotcha: /v1/reports streams NDJSON, so it must map to a server-streaming RPC, not unary. The mobile client pins TLS — the shim has to keep the same cert chain.",
      plan: "grpc-migration", next: "P2",
    }),
  ], 260),
  R("1", "t-1e", "ok: handoff registered → successor spawning"),
];

// ── P2 — proto design & plan ─────────────────────────────────────────────────
const P2: RawMessage[] = [
  U("2", "[handoff] P1 done — 43 REST endpoints inventoried across 6 clients (wiki: grpc/audit). Gotcha: /v1/reports streams NDJSON → server-streaming RPC. Mobile client pins TLS — shim must keep the cert chain. Next: P2 — design the proto + write the plan."),
  A("2", [
    think("Picking up P2. I have the audit; now I design aurora.proto and lock the plan. Two real decisions: (1) the reports/exports RPCs must be `stream` on the server side — the predecessor flagged this, I'll honor it. (2) The REST shim: hand-write 43 reverse handlers, or put grpc-gateway in front and get them generated from proto annotations? Hand-writing 43 is error-prone and exactly the kind of parallel surface that drifts. grpc-gateway is the standard answer, but it's a real architectural choice with a cost (an extra proxy hop), so this is 老板's call, not mine to assume."),
    text("I have the audit. Before I write the plan I need one decision from you — it shapes the whole shim layer."),
    use("t-2a", "mcp__fleet__fleet__ask", { question: "How should the REST compatibility shim work?" }),
  ], 480),
  R("2", "t-2a", "answered: \"Use grpc-gateway as a reverse proxy — don't hand-write the REST handlers.\""),
  A("2", [
    think("grpc-gateway it is — generated from proto annotations, so the REST surface can't drift from the gRPC one. That means every message needs `google.api.http` annotations. Let me author aurora.proto: unary RPCs for the 40 plain endpoints, server-streaming for reports + exports, and route uploads through a signed-URL RPC + direct object-store PUT rather than streaming bytes through gRPC."),
    use("t-2b", "Write", { file_path: "proto/aurora/v1/aurora.proto", content: "syntax = \"proto3\";\npackage aurora.v1;\nimport \"google/api/annotations.proto\";\n\nservice Reports {\n  rpc Stream(StreamReportsRequest) returns (stream ReportRow) {\n    option (google.api.http) = { get: \"/v1/reports\" };\n  }\n}\n// … 43 endpoints across 7 services …" }),
  ], 360),
  R("2", "t-2b", "File written: proto/aurora/v1/aurora.proto (487 lines)"),
  A("2", [
    text("Proto drafted. Linting it before I commit to the shape."),
    use("t-2c", "Bash", { command: "buf lint && buf breaking --against '.git#branch=main'", description: "Lint the proto and check for breaking changes" }),
  ], 200),
  R("2", "t-2c", "proto/aurora/v1/aurora.proto — lint OK (0 issues)\nno breaking changes against main"),
  A("2", [
    think("Clean. Now the plan: P3 core server impl, P4 clients + shim, P5 tests/docs/merge. I'll write TASKS.md so the macro plan survives the handoff, then hand off. I'll tell the successor to start P3 with reports + accounts, because those two unblock the mobile client (the one with the cert-pinning + streaming constraints)."),
    use("t-2d", "mcp__fleet__fleet__plan", { action: "create", plan_id: "grpc-migration", title: "REST→gRPC migration" }),
  ], 260),
  R("2", "t-2d", "created plan 'grpc-migration' + P3..P5 in TASKS.md"),
  A("2", [
    use("t-2e", "mcp__fleet__fleet__handoff", {
      note: "P2 green — aurora.proto (v1) + buf lint clean, TASKS.md has P3..P5. Decision from 老板: keep the REST shim as a grpc-gateway reverse proxy (not hand-written). Start P3 with the reports + accounts services; they unblock the mobile client.",
      plan: "grpc-migration", next: "P3",
    }),
  ], 240),
  R("2", "t-2e", "ok: handoff registered → successor spawning"),
];

// ── P3 — core server impl ────────────────────────────────────────────────────
const P3: RawMessage[] = [
  U("3", "[handoff] P2 green — aurora.proto + buf lint clean, plan grpc-migration P3..P5 in TASKS.md. Decision: REST shim = grpc-gateway reverse proxy. Start P3 with reports + accounts (they unblock mobile). Next: P3 — core server impl."),
  A("3", [
    think("P3: implement the actual gRPC server handlers. Codegen first — regenerate Go stubs + the gateway from the proto, then implement the services. Starting with reports (server-streaming, the tricky one) and accounts (plain unary), per the handoff note."),
    text("Starting P3. Generating stubs from the proto, then implementing the reports (streaming) and accounts services first."),
    use("t-3a", "Bash", { command: "buf generate && ls gen/go/aurora/v1", description: "Generate Go gRPC stubs + gateway" }),
  ], 420),
  R("3", "t-3a", "gen/go/aurora/v1/aurora.pb.go\naurora_grpc.pb.go\naurora.pb.gw.go   # grpc-gateway shim\nreports_grpc.pb.go\naccounts_grpc.pb.go"),
  A("3", [
    think("Stubs are there, including the gateway shim aurora.pb.gw.go — that's the REST compat layer generated for free, exactly what the grpc-gateway decision buys us. Now implement ReportsServer.Stream. It reads rows and calls stream.Send per row. This is where the predecessor's backpressure warning will matter, but the send buffer is a client-side concern — I'll note it for P4 and keep the server correct for now."),
    use("t-3b", "Write", { file_path: "src/grpc/reports.go", content: "func (s *ReportsServer) Stream(req *pb.StreamReportsRequest, stream pb.Reports_StreamServer) error {\n  rows, err := s.store.Query(stream.Context(), req.GetFilter())\n  if err != nil { return status.Errorf(codes.Internal, \"query: %v\", err) }\n  for r := range rows {\n    if err := stream.Send(toProto(r)); err != nil { return err }\n  }\n  return nil\n}" }),
  ], 360),
  R("3", "t-3b", "File written: src/grpc/reports.go"),
  A("3", [
    text("Reports + accounts + billing implemented. Running the service unit tests."),
    use("t-3c", "Bash", { command: "go test ./src/grpc/... -run 'Reports|Accounts|Billing' -count=1", description: "Unit-test the new gRPC service handlers" }),
  ], 200),
  R("3", "t-3c", "ok  aurora/src/grpc  1.84s\n--- PASS: TestReportsStream (0.21s)\n--- PASS: TestAccountsGet (0.03s)\n--- PASS: TestBillingList (0.05s)\nPASS"),
  A("3", [
    think("Three core services green. Context is climbing (~97%). Correct move: hand off before I start the client migration in a full window. The one thing the successor must not miss is backpressure — the streaming Send has no bound, so a slow reader will grow the server's memory unboundedly. That has to be fixed on the client side in P4."),
    use("t-3d", "mcp__fleet__fleet__handoff", {
      note: "P3 done — reports/accounts/billing on gRPC, unit tests green. Watch the reports stream: backpressure isn't wired, a slow client will OOM the server. P4 must add a bounded send buffer when you touch the client side.",
      plan: "grpc-migration", next: "P4",
    }),
  ], 280),
  R("3", "t-3d", "ok: handoff registered → successor spawning"),
];

// ── P4 — clients & REST shim ─────────────────────────────────────────────────
const P4: RawMessage[] = [
  U("4", "[handoff] P3 done — reports/accounts/billing on gRPC, unit tests green. WARNING: reports stream has no backpressure, slow client → server OOM; add a bounded send buffer. Next: P4 — migrate the 6 clients + stand up the grpc-gateway REST shim."),
  A("4", [
    think("P4: migrate clients and stand up the shim. First the backpressure fix the predecessor flagged — that's a correctness bug, it goes first. A bounded buffered channel between the store iterator and stream.Send, so a slow client blocks the producer instead of growing memory. Then migrate the 6 clients to the generated gRPC stubs, and bring up grpc-gateway so the old REST paths keep working 1:1."),
    text("P4. Fixing the backpressure bug first (it's a real OOM), then migrating the clients and bringing up the REST shim."),
    use("t-4a", "Edit", { file_path: "src/grpc/reports.go", old_string: "for r := range rows {", new_string: "buf := make(chan *pb.ReportRow, 256) // bounded: slow client blocks producer\n  go func(){ defer close(buf); for r := range rows { buf <- toProto(r) } }()\n  for r := range buf {" }),
  ], 440),
  R("4", "t-4a", "Edited src/grpc/reports.go (bounded send buffer added)"),
  A("4", [
    think("Backpressure handled. Now the mobile client — the sensitive one, cert-pinned per P1. The gRPC channel must reuse the exact same pinned cert chain or mobile TLS handshakes fail. Let me migrate it and keep the PinnedCertificates config intact."),
    use("t-4b", "Bash", { command: "for c in mobile web cli admin worker cron; do echo \"== $c ==\"; done && go build ./clients/... && echo BUILD_OK", description: "Migrate + build all 6 clients against the gRPC stubs" }),
  ], 320),
  R("4", "t-4b", "== mobile ==\n== web ==\n== cli ==\n== admin ==\n== worker ==\n== cron ==\nBUILD_OK"),
  A("4", [
    text("Clients migrated. Standing up grpc-gateway and running the contract tests that assert the old REST paths still respond identically."),
    use("t-4c", "Bash", { command: "go test ./test/contract/... -run RESTShim -count=1", description: "Verify the REST shim matches the old contract 1:1" }),
  ], 220),
  R("4", "t-4c", "--- PASS: TestRESTShim/reports_ndjson (0.30s)\n--- PASS: TestRESTShim/accounts_get (0.02s)\n... 41 passed\n--- FAIL: TestIntegration/error_envelope (0.04s)\n    want REST body {\"error\":{\"code\":\"not_found\"}}, got gRPC status 5\n--- FAIL: TestIntegration/error_envelope_validation\nFAIL"),
  A("4", [
    think("41 of 43 pass. The two failures are the same root cause: the old REST error envelope isn't being reconstructed from the gRPC status code — grpc-gateway returns its default error shape, not our `{\"error\":{\"code\":...}}` contract. That's a custom error handler on the gateway. It's a contained, well-understood fix, but my context is ~94% and this is a natural seam. I'll hand off with the exact failing assertion and the fix direction."),
    use("t-4d", "mcp__fleet__fleet__handoff", {
      note: "P4 done — 6 clients migrated, grpc-gateway shim serving the old REST paths 1:1 (contract tests pass). Bounded buffer added. Two integration tests still red — they assert the old REST error envelope; P5 needs to map gRPC status codes back through the shim.",
      plan: "grpc-migration", next: "P5",
    }),
  ], 300),
  R("4", "t-4d", "ok: handoff registered → successor spawning"),
];

// ── P5 — tests, docs & merge (the live one) ──────────────────────────────────
const P5: RawMessage[] = [
  U("5", "[handoff] P4 done — 6 clients migrated, grpc-gateway shim serving old REST paths 1:1. 2 integration tests red: they assert the old REST error envelope; map gRPC status codes back through the shim. Next: P5 — go green, docs, merge."),
  A("5", [
    think("Final hop. Two red tests, one cause: the gateway isn't translating gRPC status codes into our REST error envelope. grpc-gateway lets you register a custom error handler via runtime.WithErrorHandler — I map codes.NotFound → {\"error\":{\"code\":\"not_found\"}}, etc. Then run the full suite, write the migration doc, and merge. Let me implement the error handler."),
    text("Home stretch. The two failures share one cause — the shim isn't rebuilding our REST error envelope from gRPC status codes. Registering a custom gateway error handler."),
    use("t-5a", "Write", { file_path: "src/gateway/errors.go", content: "func restErrorHandler(ctx context.Context, mux *runtime.ServeMux, m runtime.Marshaler, w http.ResponseWriter, r *http.Request, err error) {\n  st := status.Convert(err)\n  w.WriteHeader(httpStatusFromCode(st.Code()))\n  _ = m.NewEncoder(w).Encode(restError{Error: restErrBody{Code: codeString(st.Code())}})\n}" }),
  ], 460),
  R("5", "t-5a", "File written: src/gateway/errors.go"),
  A("5", [
    text("Handler registered on the gateway mux. Running the full integration suite."),
    use("t-5b", "Bash", { command: "go test ./... -count=1 2>&1 | tail -6", description: "Run the entire test suite" }),
  ], 220),
  R("5", "t-5b", "ok  aurora/src/grpc       2.01s\nok  aurora/src/gateway    0.44s\nok  aurora/test/contract   3.12s\nok  aurora/test/integration 4.88s\nPASS\nok  aurora  18.303s"),
  A("5", [
    think("All green — 43/43 endpoints on gRPC, REST shim serving the old contract 1:1 including the error envelope. Last two steps: a short migration doc so the team knows the shim exists and how to add a new endpoint, then the merge. Per the workflow this is the one merge that lands on main, via --no-ff from the relay branch — but I'll report the summary and ask 老板 to greenlight the merge rather than merging silently."),
    use("t-5c", "Write", { file_path: "docs/grpc-migration.md", content: "# REST → gRPC migration\n\nAll 43 endpoints now serve gRPC; grpc-gateway fronts them with the original REST paths (1:1). To add an endpoint: edit `aurora.proto`, `buf generate`, implement the handler. The REST path + error envelope come for free.\n" }),
  ], 360),
  R("5", "t-5c", "File written: docs/grpc-migration.md"),
  A("5", [
    text("**Relay complete — all 5 hops.** 43/43 endpoints migrated to gRPC, the grpc-gateway shim serves every old REST path (and error envelope) 1:1, backpressure is bounded, and the full suite is green (18.3s). The migration doc is written.\n\nThe only thing left is the `git merge --no-ff` onto main. That's the one commit that lands on the default branch — want me to run it?"),
  ], 420),
];

export const RELAY_SCRIPTS: Record<string, RawMessage[]> = {
  "sess-grpc-1": P1,
  "sess-grpc-2": P2,
  "sess-grpc-3": P3,
  "sess-grpc-4": P4,
  "sess-grpc-5": P5,
};
