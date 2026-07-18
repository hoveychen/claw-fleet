import { describe, expect, it } from "vitest";
import {
  MOCK_QA_DELAY_MS,
  MOCK_QA_MARKETPLACES,
  MOCK_QA_PLUGINS,
  mockQaDecisionHistory,
  mockQaElicitationRequest,
  shouldDelayMockQaCommand,
} from "./qa";

describe("mock visual-QA scenarios", () => {
  it("provides realistic decision history only for the seeded session", () => {
    const records = mockQaDecisionHistory("sess-fleet-main");

    expect(records).toHaveLength(2);
    expect(records.map((record) => record.kind)).toEqual([
      "elicitation",
      "fleet-ask",
    ]);
    expect(mockQaDecisionHistory("sess-unknown")).toEqual([]);
  });

  it("provides a three-step elicitation so dot navigation is visible", () => {
    const request = mockQaElicitationRequest();

    expect(request.questions).toHaveLength(3);
    expect(request.questions.map((question) => question.header)).toEqual([
      "发布窗口",
      "回滚策略",
      "通知范围",
    ]);
  });

  it("delays only commands whose pending state drives disabled controls", () => {
    expect(MOCK_QA_DELAY_MS).toBeGreaterThanOrEqual(1_000);
    expect(shouldDelayMockQaCommand("set_mobile_relay_config")).toBe(true);
    expect(shouldDelayMockQaCommand("rotate_mobile_relay_secret")).toBe(true);
    expect(shouldDelayMockQaCommand("promote_memory")).toBe(true);
    expect(shouldDelayMockQaCommand("remove_managed_lesson")).toBe(true);
    expect(shouldDelayMockQaCommand("install_plugin")).toBe(true);
    expect(shouldDelayMockQaCommand("add_marketplace")).toBe(true);
    expect(shouldDelayMockQaCommand("export_wiki_doc")).toBe(true);
    expect(shouldDelayMockQaCommand("plugin:dialog|save")).toBe(true);
    expect(shouldDelayMockQaCommand("list_sessions")).toBe(false);
    expect(shouldDelayMockQaCommand("get_messages")).toBe(false);
  });

  it("provides one catalog plugin and marketplace for pending-state QA", () => {
    expect(MOCK_QA_PLUGINS).toHaveLength(1);
    expect(MOCK_QA_PLUGINS[0]).toMatchObject({
      enabled: false,
      isDownloaded: false,
      sourceKind: "catalog",
    });
    expect(MOCK_QA_MARKETPLACES).toHaveLength(1);
  });
});
