import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { splitRefsBlock } from "../../../shared-ts/sessionMentions";
import { MentionText, RefChips, RefsNote, SessionMentionNavProvider } from "./SessionMention";

const nav = { open: () => {}, has: (id: string) => id === "known" };

const render = (text: string) =>
  renderToStaticMarkup(
    <SessionMentionNavProvider nav={nav}>
      <MentionText text={text} />
    </SessionMentionNavProvider>,
  );

describe("MentionText", () => {
  it("renders a mention of a session in the snapshot as a tappable chip", () => {
    const html = render("see [@Known one](fleet-session:known) please");
    expect(html).toContain("<button");
    expect(html).toContain("@Known one");
    expect(html).not.toContain("fleet-session:");
  });

  it("renders a mention of an unknown session as an inert chip", () => {
    const html = render("see [@Gone](fleet-session:gone)");
    expect(html).not.toContain("<button");
    expect(html).toContain('data-inert="true"');
    expect(html).toContain("@Gone");
  });

  it("passes plain text through untouched", () => {
    expect(render("no mentions here")).toBe("no mentions here");
  });
});

describe("referenced-sessions block collapse", () => {
  it("hides the raw block and shows one line with the session count", () => {
    const sent =
      "use [@A](fleet-session:a) and [@B](fleet-session:b)\n\n<fleet-referenced-sessions>\n## A\nx\n## B\ny\n</fleet-referenced-sessions>";
    const { text, refs } = splitRefsBlock(sent);
    expect(text).not.toContain("fleet-referenced-sessions");
    expect(refs).toBe(2);
    const html = renderToStaticMarkup(<RefsNote count={refs} />);
    expect(html).toMatch(/2/);
    expect(html).not.toContain("## A");
  });

  it("renders nothing when no block was attached", () => {
    expect(renderToStaticMarkup(<RefsNote count={0} />)).toBe("");
  });
});

describe("RefChips", () => {
  it("makes known sessions and wiki docs tappable, unknown sessions inert", () => {
    const html = renderToStaticMarkup(
      <SessionMentionNavProvider nav={{ ...nav, openWiki: () => {} }}>
        <RefChips
          refs={[
            { kind: "session", id: "known", title: "Known one" },
            { kind: "session", id: "gone", title: "Gone" },
            { kind: "wiki", slug: "arch/x" },
          ]}
          onRemove={() => {}}
        />
      </SessionMentionNavProvider>,
    );
    expect(html).toContain("Known one");
    expect(html).toContain("arch/x");
    expect(html.match(/data-inert="true"/g)?.length).toBe(1);
    expect(html.match(/移除引用|Remove reference/g)?.length).toBe(3);
  });
});
