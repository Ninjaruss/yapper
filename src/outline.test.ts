import { beforeEach, describe, expect, it } from "vitest";
import { sinkGhosts, updateOutline, type OutlineEntryUI } from "./outline";

const e = (label: string, status: OutlineEntryUI["status"]): OutlineEntryUI => ({ label, status });

describe("updateOutline", () => {
  let container: HTMLElement;
  beforeEach(() => {
    container = document.createElement("div");
  });

  it("creates lines with status classes and the arriving marker", () => {
    updateOutline(container, [e("the drive out", "covered"), e("the empty apartment", "current")]);
    const [a, b] = Array.from(container.children) as HTMLElement[];
    expect(a.textContent).toBe("the drive out");
    expect(a.classList.contains("outline-covered")).toBe(true);
    expect(a.classList.contains("outline-arriving")).toBe(true);
    expect(b.classList.contains("outline-current")).toBe(true);
  });

  it("returns the current-topic element, or null", () => {
    const current = updateOutline(container, [e("a", "covered"), e("b", "current")]);
    expect(current?.textContent).toBe("b");
    expect(updateOutline(container, [e("a", "covered"), e("b", "covered")])).toBeNull();
  });

  it("reuses DOM nodes for persisting labels (stable paper)", () => {
    updateOutline(container, [e("the drive out", "current")]);
    const before = container.children[0];
    updateOutline(container, [e("the drive out", "covered"), e("calling mom", "current")]);
    expect(container.children[0]).toBe(before); // same node, not recreated
    expect(before.classList.contains("outline-covered")).toBe(true);
    expect(before.classList.contains("outline-arriving")).toBe(false); // only NEW lines arrive
  });

  it("removes lines the model dropped and placeholder lines", () => {
    const placeholder = document.createElement("p");
    placeholder.textContent = "listening for the shape of it…";
    container.appendChild(placeholder); // no data-label => placeholder
    updateOutline(container, [e("a", "current")]);
    expect(container.children.length).toBe(1);
    updateOutline(container, [e("b", "current")]);
    expect(container.children.length).toBe(1);
    expect((container.children[0] as HTMLElement).textContent).toBe("b");
  });

  it("keeps document order matching entries order", () => {
    updateOutline(container, [e("a", "covered"), e("b", "current")]);
    updateOutline(container, [e("b", "covered"), e("a", "covered"), e("c", "current")]);
    const texts = Array.from(container.children).map((c) => c.textContent);
    expect(texts).toEqual(["b", "a", "c"]);
  });

  it("uses textContent only — labels are never parsed as markup", () => {
    updateOutline(container, [e("<img src=x onerror=alert(1)>", "current")]);
    expect(container.querySelector("img")).toBeNull();
  });
});

describe("sinkGhosts", () => {
  it("moves intent-untouched entries to the end, preserving both orders", () => {
    const sorted = sinkGhosts([
      e("a", "covered"),
      e("ghost one", "intent_untouched"),
      e("b", "current"),
      e("ghost two", "intent_untouched"),
    ]);
    expect(sorted.map((x) => x.label)).toEqual(["a", "b", "ghost one", "ghost two"]);
  });

  it("is a no-op without ghosts", () => {
    const entries = [e("a", "covered"), e("b", "current")];
    expect(sinkGhosts(entries)).toEqual(entries);
  });
});
