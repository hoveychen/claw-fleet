import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { TextBlock } from "./TextBlock";

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
