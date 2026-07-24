export interface OutlineEntryUI {
  label: string;
  status: "covered" | "current" | "intent_untouched";
}

const STATUS_CLASSES = ["outline-covered", "outline-current", "outline-intent"] as const;

function statusClass(status: OutlineEntryUI["status"]): string {
  if (status === "covered") return "outline-covered";
  if (status === "current") return "outline-current";
  return "outline-intent";
}

/**
 * Incrementally reconciles the outline container against `entries`, keyed
 * by label (stable thanks to the Rust-side label damper). Persisting lines
 * keep their DOM nodes so the paper feels stable; new lines carry
 * `outline-arriving` (CSS fade+shimmer; removed on animationend, with a
 * reconciliation-time fallback for environments where animations never
 * run). Status changes swap classes in place. Children without data-label
 * (placeholder lines) are removed. Returns the current-topic element for
 * the shine underline, or null.
 *
 * textContent only — labels are LLM-derived and must never parse as markup.
 */
export function updateOutline(
  container: HTMLElement,
  entries: OutlineEntryUI[],
): HTMLElement | null {
  const byLabel = new Map<string, HTMLElement>();
  for (const child of Array.from(container.children) as HTMLElement[]) {
    if (child.dataset.label !== undefined) byLabel.set(child.dataset.label, child);
    else child.remove();
  }

  let currentEl: HTMLElement | null = null;
  let anchor: HTMLElement | null = null;
  for (const entry of entries) {
    let el = byLabel.get(entry.label);
    if (el !== undefined) {
      byLabel.delete(entry.label);
      el.classList.remove("outline-arriving"); // fallback when animationend never fired
      const cls = statusClass(entry.status);
      if (!el.classList.contains(cls)) {
        el.classList.remove(...STATUS_CLASSES);
        el.classList.add(cls);
      }
    } else {
      el = document.createElement("p");
      el.dataset.label = entry.label;
      el.textContent = entry.label;
      el.classList.add(statusClass(entry.status), "outline-arriving");
      const created = el;
      created.addEventListener(
        "animationend",
        () => created.classList.remove("outline-arriving"),
        { once: true },
      );
    }
    if (anchor === null) {
      if (container.firstElementChild !== el) container.prepend(el);
    } else if (anchor.nextElementSibling !== el) {
      anchor.after(el);
    }
    anchor = el;
    if (entry.status === "current") currentEl = el;
  }
  for (const stale of byLabel.values()) stale.remove();
  return currentEl;
}
