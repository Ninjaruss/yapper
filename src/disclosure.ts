// A small collapsible section — the redesign's progressive-disclosure primitive.
// Header is a real <button aria-expanded> (keyboard + screen-reader friendly);
// the caller fills `.body` with content and it shows/hides on toggle. Used for
// Setup's "Settings" and Recap's "Moments". Labels are trusted static strings
// (textContent only). Height is not animated when the user prefers reduced motion.

export interface Disclosure {
  /** The whole component: header button + body. Append this to the page. */
  el: HTMLElement;
  /** The content container — append your rows here. */
  body: HTMLElement;
  setOpen(open: boolean): void;
  isOpen(): boolean;
}

export function createDisclosure(opts: {
  label: string;
  /** Optional right-aligned count, e.g. "18 lines" or "4". */
  count?: string;
  /** Show a ⚙ before the chevron (used by Setup's Settings). */
  gear?: boolean;
  /** Start expanded. Defaults to collapsed. */
  open?: boolean;
}): Disclosure {
  const el = document.createElement("div");
  el.className = "disclosure";

  const header = document.createElement("button");
  header.type = "button";
  header.className = "disc-header";

  if (opts.gear) {
    const g = document.createElement("span");
    g.className = "disc-gear";
    g.textContent = "⚙";
    header.appendChild(g);
  }
  const chev = document.createElement("span");
  chev.className = "disc-chev";
  chev.textContent = "›";
  chev.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.className = "disc-label";
  label.textContent = opts.label;
  header.append(chev, label);
  if (opts.count != null) {
    const c = document.createElement("span");
    c.className = "disc-count";
    c.textContent = opts.count;
    header.appendChild(c);
  }

  const body = document.createElement("div");
  body.className = "disc-body";

  el.append(header, body);

  let open = false;
  const setOpen = (v: boolean) => {
    open = v;
    header.setAttribute("aria-expanded", String(v));
    header.classList.toggle("open", v);
    body.hidden = !v;
  };
  setOpen(opts.open ?? false);
  header.addEventListener("click", () => setOpen(!open));

  return { el, body, setOpen, isOpen: () => open };
}
