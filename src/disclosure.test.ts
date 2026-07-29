import { describe, expect, it } from "vitest";
import { createDisclosure } from "./disclosure";

describe("createDisclosure", () => {
  it("starts collapsed with the body hidden and aria-expanded=false", () => {
    const d = createDisclosure({ label: "Settings" });
    const header = d.el.querySelector<HTMLButtonElement>(".disc-header")!;
    expect(d.isOpen()).toBe(false);
    expect(header.getAttribute("aria-expanded")).toBe("false");
    expect(d.body.hidden).toBe(true);
    expect(d.el.querySelector(".disc-label")?.textContent).toBe("Settings");
  });

  it("toggles open/closed on header click", () => {
    const d = createDisclosure({ label: "Moments" });
    const header = d.el.querySelector<HTMLButtonElement>(".disc-header")!;
    header.click();
    expect(d.isOpen()).toBe(true);
    expect(header.getAttribute("aria-expanded")).toBe("true");
    expect(d.body.hidden).toBe(false);
    header.click();
    expect(d.isOpen()).toBe(false);
    expect(d.body.hidden).toBe(true);
  });

  it("honors open:true and renders an optional count + gear", () => {
    const d = createDisclosure({ label: "Moments", count: "4", gear: true, open: true });
    expect(d.isOpen()).toBe(true);
    expect(d.body.hidden).toBe(false);
    expect(d.el.querySelector(".disc-count")?.textContent).toBe("4");
    expect(d.el.querySelector(".disc-gear")?.textContent).toBe("⚙");
  });

  it("setOpen drives the state directly", () => {
    const d = createDisclosure({ label: "X" });
    d.setOpen(true);
    expect(d.isOpen()).toBe(true);
    d.setOpen(false);
    expect(d.isOpen()).toBe(false);
  });
});
