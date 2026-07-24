import { describe, expect, it } from "vitest";
import { escapeHtml } from "./escape";

describe("escapeHtml", () => {
  it("escapes &, <, >, \", and ' in one pass", () => {
    expect(escapeHtml(`&<>"'`)).toBe("&amp;&lt;&gt;&quot;&#39;");
  });

  it("leaves ordinary text untouched", () => {
    expect(escapeHtml("hello world")).toBe("hello world");
  });
});
