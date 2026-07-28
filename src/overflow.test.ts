import { describe, expect, it, vi } from "vitest";
import { createOverflowMenu } from "./overflow";

function mount(menu: HTMLElement) {
  document.body.appendChild(menu);
  return () => menu.remove();
}

describe("createOverflowMenu", () => {
  it("starts closed and opens on the ⋯ button", () => {
    const menu = createOverflowMenu([{ label: "Export", onSelect: () => {} }]);
    const cleanup = mount(menu);
    const btn = menu.querySelector<HTMLButtonElement>(".overflow-btn")!;
    const popover = menu.querySelector<HTMLElement>(".overflow-menu")!;
    expect(popover.hidden).toBe(true);
    expect(btn.getAttribute("aria-expanded")).toBe("false");
    btn.click();
    expect(popover.hidden).toBe(false);
    expect(btn.getAttribute("aria-expanded")).toBe("true");
    cleanup();
  });

  it("fires the item's onSelect and closes", () => {
    const onSelect = vi.fn();
    const menu = createOverflowMenu([{ label: "Forget", onSelect }]);
    const cleanup = mount(menu);
    menu.querySelector<HTMLButtonElement>(".overflow-btn")!.click();
    menu.querySelector<HTMLButtonElement>(".overflow-item")!.click();
    expect(onSelect).toHaveBeenCalledOnce();
    expect(menu.querySelector<HTMLElement>(".overflow-menu")!.hidden).toBe(true);
    cleanup();
  });

  it("closes on Escape and on an outside click", () => {
    const menu = createOverflowMenu([{ label: "Show file", onSelect: () => {} }]);
    const cleanup = mount(menu);
    const btn = menu.querySelector<HTMLButtonElement>(".overflow-btn")!;
    const popover = menu.querySelector<HTMLElement>(".overflow-menu")!;

    btn.click();
    menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(popover.hidden).toBe(true);

    btn.click();
    expect(popover.hidden).toBe(false);
    document.body.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(popover.hidden).toBe(true);

    cleanup();
  });

  it("renders one menuitem per action", () => {
    const menu = createOverflowMenu([
      { label: "Export transcript", onSelect: () => {} },
      { label: "Show file", onSelect: () => {} },
      { label: "Forget", onSelect: () => {} },
    ]);
    expect(menu.querySelectorAll('[role="menuitem"]').length).toBe(3);
  });
});
