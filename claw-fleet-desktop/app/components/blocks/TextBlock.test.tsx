import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TextBlock } from "./TextBlock";
import { WikiLinksProvider } from "../../markdown/wikiLinksContext";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

function render(markdown: string): string {
  return renderToStaticMarkup(<TextBlock text={markdown} />);
}

describe("TextBlock fenced code spacing", () => {
  it("reserves a header row above highlighted code when a language label is present", () => {
    const html = render("```bash\necho ok\n```");

    expect(html).toContain(">bash</span>");
    expect(html).toContain("padding-top:28px");
  });

  it("keeps the compact default padding for an unlabeled fence", () => {
    const html = render("```\necho first\necho second\n```");

    expect(html).not.toContain("padding-top:28px");
    expect(html).toContain("padding:1em");
  });

  it("reserves the same header row when a large labeled block skips highlighting", () => {
    const html = render(`\`\`\`text\n${"x".repeat(8001)}\n\`\`\``);

    expect(html).toContain(">text</span>");
    expect(html).toContain("padding-top:28px");
  });
});

describe("TextBlock wiki refs", () => {
  const ctx = {
    hasSlug: (slug: string) => slug === "arch/overview",
    openSlug: () => {},
  };

  it("leaves [[slug]] as plain text when nothing offers a wiki context", () => {
    // A chat bubble or any other context-free caller: the ref is prose, and it
    // must not render as a link that goes nowhere.
    const html = renderToStaticMarkup(<TextBlock text="see [[arch/overview]]" />);

    expect(html).toContain("[[arch/overview]]");
    expect(html).not.toContain("<a");
  });

  it("takes the wiki context from the surrounding provider", () => {
    // How agent prose gets clickable refs: SessionDetail provides the context
    // once for the whole transcript instead of threading a prop through every
    // block renderer between here and there.
    const html = renderToStaticMarkup(
      <WikiLinksProvider value={ctx}>
        <TextBlock text="see [[arch/overview]]" />
      </WikiLinksProvider>,
    );

    expect(html).toContain('href="#wiki=arch/overview"');
    expect(html).toContain(">arch/overview</a>");
  });

  it("grays out a ref to a doc that was never published", () => {
    const html = renderToStaticMarkup(
      <WikiLinksProvider value={ctx}>
        <TextBlock text="see [[notes/ghost]]" />
      </WikiLinksProvider>,
    );

    expect(html).toContain("not published");
  });

  it("prefers an explicit wiki prop over the provider", () => {
    // The 知识库 page passes its own context down; a provider higher up (or a
    // future one) must not silently replace it.
    const explicit = { hasSlug: () => false, openSlug: () => {} };
    const html = renderToStaticMarkup(
      <WikiLinksProvider value={ctx}>
        <TextBlock text="see [[arch/overview]]" wiki={explicit} />
      </WikiLinksProvider>,
    );

    expect(html).toContain("not published");
  });
});
