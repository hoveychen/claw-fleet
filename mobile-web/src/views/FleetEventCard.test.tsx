import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { FleetEventCard } from "./FleetEventCard";

describe("FleetEventCard", () => {
  it("keeps the injected prompt collapsed behind a passive watch header", () => {
    const html = renderToStaticMarkup(
      <FleetEventCard
        event={{ kind: "watch", status: "fired", id: "w7" }}
        text="large captured output"
      />,
    );
    expect(html).toContain("fleet-event-card");
    expect(html).toContain("Fleet watch");
    expect(html).toContain("w7");
    expect(html).not.toContain("large captured output");
  });
});
