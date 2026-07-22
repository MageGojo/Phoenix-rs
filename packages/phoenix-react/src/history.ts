import { isRecord } from "./protocol.js";

const HISTORY_STATE_KEY = "__phoenix";
let historyEntrySequence = 0;

export interface HistorySnapshot {
  version: 1;
  key: string;
  scroll: [number, number];
  focus: string | null;
}

export type HistoryMode = "push" | "replace" | "none";

export interface LocationRestoreOptions {
  preserveScroll?: boolean;
  preserveFocus?: boolean;
}

export interface LinkClickIntent {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export class NavigationHistory {
  private readonly windowRef: Window;
  private scrollFrame: number | null = null;
  private originalScrollRestoration: ScrollRestoration | undefined;
  private installed = false;

  constructor(private readonly documentRef: Document) {
    const windowRef = documentRef.defaultView;
    if (!windowRef) throw new Error("Phoenix navigation requires a browser window");
    this.windowRef = windowRef;
  }

  install(): void {
    if (this.installed) return;
    this.installed = true;
    this.originalScrollRestoration = this.windowRef.history.scrollRestoration;
    this.windowRef.history.scrollRestoration = "manual";
    this.documentRef.addEventListener("focusin", this.handleFocusChange);
    this.windowRef.addEventListener("scroll", this.handleScroll, { passive: true });
    this.save();
  }

  dispose(): void {
    if (!this.installed) return;
    this.installed = false;
    this.documentRef.removeEventListener("focusin", this.handleFocusChange);
    this.windowRef.removeEventListener("scroll", this.handleScroll);
    if (this.scrollFrame !== null) {
      this.windowRef.cancelAnimationFrame(this.scrollFrame);
      this.scrollFrame = null;
    }
    if (this.originalScrollRestoration !== undefined) {
      this.windowRef.history.scrollRestoration = this.originalScrollRestoration;
    }
  }

  capture(): HistorySnapshot {
    const scrollX = Number.isFinite(this.windowRef.scrollX) ? this.windowRef.scrollX : 0;
    const scrollY = Number.isFinite(this.windowRef.scrollY) ? this.windowRef.scrollY : 0;
    return {
      version: 1,
      key: historySnapshot(this.windowRef.history.state)?.key ?? nextHistoryKey(),
      scroll: [scrollX, scrollY],
      focus: focusSelector(this.documentRef),
    };
  }

  save(): void {
    if (!this.installed) return;
    this.windowRef.history.replaceState(
      historyState(this.windowRef.history.state, this.capture()),
      "",
      this.windowRef.location.href,
    );
  }

  write(
    mode: Exclude<HistoryMode, "none">,
    target: URL,
    snapshot: HistorySnapshot,
  ): void {
    const base = mode === "replace" ? this.windowRef.history.state : null;
    this.windowRef.history[mode === "push" ? "pushState" : "replaceState"](
      historyState(base, snapshot),
      "",
      target.href,
    );
  }

  restore(
    target: URL,
    options: LocationRestoreOptions,
    restoration?: HistorySnapshot,
    previous?: HistorySnapshot,
  ): void {
    const scroll = restoration?.scroll ?? (options.preserveScroll ? previous?.scroll : undefined);
    if (scroll) {
      this.windowRef.scrollTo(scroll[0], scroll[1]);
    } else if (!scrollToHash(this.documentRef, target.hash)) {
      this.windowRef.scrollTo(0, 0);
    }

    const focus = restoration?.focus ?? (options.preserveFocus ? previous?.focus : null);
    if (focus && focusSelectorTarget(this.documentRef, focus)) return;
    if (!target.hash) focusPage(this.documentRef);
  }

  private readonly handleScroll = () => {
    if (this.scrollFrame !== null) return;
    this.scrollFrame = this.windowRef.requestAnimationFrame(() => {
      this.scrollFrame = null;
      this.save();
    });
  };

  private readonly handleFocusChange = () => this.save();
}

export function shouldHandleLink(event: LinkClickIntent, anchor: HTMLAnchorElement): boolean {
  if (
    event.button !== 0 ||
    event.metaKey ||
    event.ctrlKey ||
    event.shiftKey ||
    event.altKey ||
    anchor.hasAttribute("download") ||
    anchor.hasAttribute("data-phoenix-reload") ||
    (anchor.target !== "" && anchor.target.toLowerCase() !== "_self") ||
    anchor.relList.contains("external") ||
    anchor.closest('[contenteditable="true"]')
  ) {
    return false;
  }
  const windowRef = anchor.ownerDocument.defaultView;
  if (!windowRef) return false;
  const target = new URL(anchor.href, windowRef.location.href);
  return (
    target.origin === windowRef.location.origin &&
    (target.protocol === "http:" || target.protocol === "https:")
  );
}

export function isHashOnlyVisit(target: URL, current: Location): boolean {
  return (
    target.origin === current.origin &&
    target.pathname === current.pathname &&
    target.search === current.search &&
    target.hash !== current.hash
  );
}

export function nextHistorySnapshot(
  previous: HistorySnapshot,
  options: LocationRestoreOptions,
): HistorySnapshot {
  return {
    version: 1,
    key: nextHistoryKey(),
    scroll: options.preserveScroll ? previous.scroll : [0, 0],
    focus: options.preserveFocus ? previous.focus : null,
  };
}

export function historySnapshot(value: unknown): HistorySnapshot | undefined {
  if (!isRecord(value)) return undefined;
  const candidate = value[HISTORY_STATE_KEY];
  if (
    !isRecord(candidate) ||
    candidate.version !== 1 ||
    typeof candidate.key !== "string" ||
    !Array.isArray(candidate.scroll) ||
    candidate.scroll.length !== 2 ||
    typeof candidate.scroll[0] !== "number" ||
    typeof candidate.scroll[1] !== "number" ||
    (candidate.focus !== null && typeof candidate.focus !== "string")
  ) {
    return undefined;
  }
  return candidate as unknown as HistorySnapshot;
}

function nextHistoryKey(): string {
  historyEntrySequence += 1;
  return `phoenix-${historyEntrySequence}`;
}

function historyState(value: unknown, snapshot: HistorySnapshot): Record<string, unknown> {
  return {
    ...(isRecord(value) ? value : {}),
    [HISTORY_STATE_KEY]: snapshot,
  };
}

function focusSelector(documentRef: Document): string | null {
  const active = documentRef.activeElement;
  const HTMLElementClass = documentRef.defaultView?.HTMLElement;
  if (!HTMLElementClass || !(active instanceof HTMLElementClass) || active === documentRef.body) {
    return null;
  }
  const focusKey = active.getAttribute("data-phoenix-focus-key");
  if (focusKey) {
    return `[data-phoenix-focus-key="${escapeAttributeSelector(focusKey)}"]`;
  }
  if (!active.id) return null;
  const escape = documentRef.defaultView?.CSS?.escape;
  if (escape) return `#${escape(active.id)}`;
  return /^[A-Za-z][A-Za-z0-9_-]*$/.test(active.id) ? `#${active.id}` : null;
}

function escapeAttributeSelector(value: string): string {
  return value.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
}

function focusSelectorTarget(documentRef: Document, selector: string): boolean {
  let target: Element | null = null;
  try {
    target = documentRef.querySelector(selector);
  } catch {
    return false;
  }
  return target ? focusElement(documentRef, target) : false;
}

function focusPage(documentRef: Document): void {
  const target = ["[autofocus]", "main", "[role=main]", "h1", "#phoenix-root"]
    .map((selector) => documentRef.querySelector(selector))
    .find((element) => element !== null);
  if (target) focusElement(documentRef, target);
}

function focusElement(documentRef: Document, target: Element): boolean {
  const HTMLElementClass = documentRef.defaultView?.HTMLElement;
  if (!HTMLElementClass || !(target instanceof HTMLElementClass)) return false;
  if (!isNaturallyFocusable(target) && !target.hasAttribute("tabindex")) {
    target.setAttribute("tabindex", "-1");
  }
  target.focus({ preventScroll: true });
  return documentRef.activeElement === target;
}

function isNaturallyFocusable(element: HTMLElement): boolean {
  return (
    element.matches("a[href], button, input, select, textarea, summary") ||
    element.hasAttribute("tabindex")
  ) && !element.hasAttribute("disabled");
}

function scrollToHash(documentRef: Document, hash: string): boolean {
  if (!hash) return false;
  let id: string;
  try {
    id = decodeURIComponent(hash.slice(1));
  } catch {
    return false;
  }
  const target = documentRef.getElementById(id);
  if (!target) return false;
  target.scrollIntoView?.({ block: "start" });
  focusElement(documentRef, target);
  return true;
}
