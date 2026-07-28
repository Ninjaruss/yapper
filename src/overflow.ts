// A "⋯" overflow menu — tucks a row's secondary actions behind one control so
// the row reads clean (used by Setup's past-talk rows for Export / Show file /
// Forget). Keyboard-accessible (arrow keys move between items; Escape closes and
// returns focus to the trigger); closes on outside click. Labels are trusted
// static strings.

export interface OverflowItem {
  label: string;
  onSelect: () => void;
}

export function createOverflowMenu(items: OverflowItem[], ariaLabel = "More actions"): HTMLElement {
  const root = document.createElement("div");
  root.className = "overflow";

  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "overflow-btn";
  btn.textContent = "⋯";
  btn.setAttribute("aria-haspopup", "menu");
  btn.setAttribute("aria-expanded", "false");
  btn.setAttribute("aria-label", ariaLabel);

  const menu = document.createElement("div");
  menu.className = "overflow-menu";
  menu.setAttribute("role", "menu");
  menu.hidden = true;

  const itemEls = items.map((item) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "overflow-item";
    b.setAttribute("role", "menuitem");
    b.tabIndex = -1;
    b.textContent = item.label;
    b.addEventListener("click", () => {
      close();
      item.onSelect();
    });
    menu.appendChild(b);
    return b;
  });

  let open = false;
  const onDocClick = (e: MouseEvent) => {
    if (!root.contains(e.target as Node)) close();
  };

  function openMenu() {
    if (open) return;
    open = true;
    menu.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    document.addEventListener("click", onDocClick, true);
    itemEls[0]?.focus();
  }
  function close() {
    if (!open) return;
    open = false;
    menu.hidden = true;
    btn.setAttribute("aria-expanded", "false");
    document.removeEventListener("click", onDocClick, true);
  }

  btn.addEventListener("click", () => (open ? close() : openMenu()));

  root.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (open) {
        e.preventDefault();
        close();
        btn.focus();
      }
      return;
    }
    if (!open) return;
    const i = itemEls.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown") {
      e.preventDefault();
      itemEls[(i + 1 + itemEls.length) % itemEls.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      itemEls[(i - 1 + itemEls.length) % itemEls.length]?.focus();
    }
  });

  root.append(btn, menu);
  return root;
}
