import {
  type AnchorHTMLAttributes,
  createContext,
  createElement,
  type ComponentType,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  useContext,
} from "react";
import { flushSync } from "react-dom";
import { createRoot, hydrateRoot, type Root } from "react-dom/client";

import { abortError, isAbortError } from "./errors.js";
import { updatePageHead } from "./head.js";
import {
  historySnapshot,
  isHashOnlyVisit,
  NavigationHistory,
  nextHistorySnapshot,
  shouldHandleLink,
  type HistoryMode,
  type HistorySnapshot,
} from "./history.js";
import { fetchPage } from "./page-client.js";
import {
  assertPageEnvelope,
  readPage,
  requiredElement,
  writePage,
  type ComponentList,
  type ComponentSource,
  type DecryptPage,
  type PageEnvelope,
} from "./protocol.js";
import {
  componentRegistry,
  pageProps,
  PhoenixRenderProvider,
  requiredComponent,
} from "./rendering.js";

export interface VisitOptions {
  replace?: boolean;
  preserveScroll?: boolean;
  preserveFocus?: boolean;
  signal?: AbortSignal;
}

export interface PhoenixNavigator {
  readonly page: PageEnvelope;
  visit(url: string | URL, options?: VisitOptions): Promise<PageEnvelope>;
  reload(options?: Omit<VisitOptions, "replace">): Promise<PageEnvelope>;
  dispose(): void;
}

export interface PhoenixOptions {
  pages?: ComponentSource;
  islands?: ComponentSource | ComponentList;
  document?: Document;
  fetcher?: typeof fetch;
  decrypt?: DecryptPage;
  onNavigationError?: (error: unknown) => void;
  hardNavigate?: (url: string) => void;
}

const navigationContext = createContext<PhoenixNavigator | null>(null);
const navigators = new WeakMap<Document, BrowserNavigator>();

export async function startPhoenix(options: PhoenixOptions = {}): Promise<PageEnvelope> {
  const documentRef = options.document ?? document;
  const envelope = readPage(documentRef);
  navigators.get(documentRef)?.dispose();
  const navigator = new BrowserNavigator(documentRef, envelope, options);
  navigators.set(documentRef, navigator);
  try {
    await navigator.start();
  } catch (error) {
    navigator.dispose();
    throw error;
  }
  return envelope;
}

export function getPhoenixNavigator(
  documentRef: Document = document,
): PhoenixNavigator | null {
  return navigators.get(documentRef) ?? null;
}

export function stopPhoenix(documentRef: Document = document): void {
  navigators.get(documentRef)?.dispose();
}

export function navigate(
  url: string | URL,
  options: VisitOptions = {},
  documentRef: Document = document,
): Promise<PageEnvelope> {
  const navigator = navigators.get(documentRef);
  if (!navigator) throw new Error("Phoenix has not been started for this document");
  return navigator.visit(url, options);
}

export interface LinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> {
  href: string;
  replace?: boolean;
  preserveScroll?: boolean;
  preserveFocus?: boolean;
  reloadDocument?: boolean;
}

export function Link({
  href,
  replace = false,
  preserveScroll = false,
  preserveFocus = false,
  reloadDocument = false,
  onClick,
  ...props
}: LinkProps): ReactElement {
  const navigator = useContext(navigationContext);
  const handleClick = (event: ReactMouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);
    if (
      event.defaultPrevented ||
      !navigator ||
      !shouldHandleLink(event, event.currentTarget)
    ) {
      return;
    }
    event.preventDefault();
    void navigator.visit(href, { replace, preserveScroll, preserveFocus }).catch(() => {});
  };
  return createElement("a", {
    ...props,
    href,
    onClick: handleClick,
    ...(reloadDocument ? { "data-phoenix-reload": "" } : {}),
  });
}

class BrowserNavigator implements PhoenixNavigator {
  private readonly windowRef: Window;
  private readonly rootElement: HTMLElement;
  private readonly pages: ComponentSource;
  private readonly islands: ComponentSource;
  private readonly fetcher: typeof fetch;
  private readonly decrypt?: DecryptPage;
  private readonly onNavigationError?: (error: unknown) => void;
  private readonly hardNavigate: (url: string) => void;
  private readonly history: NavigationHistory;
  private currentPage: PageEnvelope;
  private fullRoot: Root | null = null;
  private islandRoots: Root[] = [];
  private activeController: AbortController | null = null;
  private navigationSequence = 0;
  private disposed = false;

  constructor(
    private readonly documentRef: Document,
    envelope: PageEnvelope,
    options: PhoenixOptions,
  ) {
    const windowRef = documentRef.defaultView;
    if (!windowRef) throw new Error("Phoenix navigation requires a browser window");
    this.windowRef = windowRef;
    this.history = new NavigationHistory(documentRef);
    this.rootElement = requiredElement(documentRef, "phoenix-root");
    this.pages = options.pages ?? {};
    this.islands = componentRegistry(options.islands);
    this.fetcher = options.fetcher ?? fetch;
    this.decrypt = options.decrypt;
    this.onNavigationError = options.onNavigationError;
    this.hardNavigate = options.hardNavigate ?? ((url) => this.windowRef.location.assign(url));
    this.currentPage = envelope;
  }

  get page(): PageEnvelope {
    return this.currentPage;
  }

  async start(): Promise<void> {
    updatePageHead(this.documentRef, this.currentPage.head);
    if (this.currentPage.render_mode === "islands") {
      this.islandRoots = await hydrateIslands(
        this.documentRef,
        this.currentPage,
        this.islands,
        this,
      );
    } else {
      const Page = await requiredComponent(this.pages, this.currentPage.page, "page");
      const element = this.pageElement(Page, this.currentPage);
      if (this.currentPage.render_mode === "ssr") {
        this.fullRoot = hydrateRoot(this.rootElement, element);
      } else {
        this.fullRoot = createRoot(this.rootElement);
        this.fullRoot.render(element);
      }
    }
    this.installNavigation();
  }

  visit(url: string | URL, options: VisitOptions = {}): Promise<PageEnvelope> {
    return this.performVisit(url, options, options.replace ? "replace" : "push");
  }

  reload(options: Omit<VisitOptions, "replace"> = {}): Promise<PageEnvelope> {
    return this.performVisit(this.windowRef.location.href, {
      preserveScroll: true,
      preserveFocus: true,
      ...options,
    }, "none");
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.navigationSequence += 1;
    this.activeController?.abort();
    this.activeController = null;
    this.documentRef.removeEventListener("click", this.handleDocumentClick);
    this.windowRef.removeEventListener("popstate", this.handlePopState);
    this.history.dispose();
    this.fullRoot?.unmount();
    this.fullRoot = null;
    for (const root of this.islandRoots) root.unmount();
    this.islandRoots = [];
    if (navigators.get(this.documentRef) === this) navigators.delete(this.documentRef);
  }

  private async performVisit(
    url: string | URL,
    options: VisitOptions,
    historyMode: HistoryMode,
    restoration?: HistorySnapshot,
  ): Promise<PageEnvelope> {
    if (this.disposed) throw new Error("Phoenix navigation has been disposed");
    const target = new URL(url.toString(), this.windowRef.location.href);
    if (target.origin !== this.windowRef.location.origin) {
      throw new Error("Phoenix navigation only supports same-origin URLs");
    }

    if (historyMode !== "none" && isHashOnlyVisit(target, this.windowRef.location)) {
      this.navigationSequence += 1;
      this.activeController?.abort();
      this.history.save();
      this.history.write(historyMode, target, this.history.capture());
      this.history.restore(target, options);
      this.history.save();
      return this.currentPage;
    }

    const navigation = this.navigationSequence + 1;
    this.navigationSequence = navigation;
    this.activeController?.abort();
    const controller = new AbortController();
    this.activeController = controller;
    const abortFromCaller = () => controller.abort();
    if (options.signal?.aborted) controller.abort();
    options.signal?.addEventListener("abort", abortFromCaller, { once: true });
    this.dispatch("phoenix:navigation-start", { url: target.href });

    try {
      const envelope = await fetchPage(target.href, this.decrypt, this.fetcher, {
        signal: controller.signal,
      });
      if (navigation !== this.navigationSequence || controller.signal.aborted) {
        throw abortError();
      }
      if (requiresHardNavigation(this.currentPage, envelope)) {
        this.dispatch("phoenix:navigation-hard", { url: target.href, page: envelope });
        this.hardNavigate(target.href);
        return this.currentPage;
      }
      assertPageEnvelope(envelope);
      const Page = await requiredComponent(this.pages, envelope.page, "page");
      if (navigation !== this.navigationSequence || controller.signal.aborted) {
        throw abortError();
      }

      const previousSnapshot = this.history.capture();
      if (historyMode !== "none") {
        this.history.save();
        this.history.write(historyMode, target, nextHistorySnapshot(previousSnapshot, options));
      }
      this.renderPage(Page, envelope);
      this.history.restore(target, options, restoration, previousSnapshot);
      this.history.save();
      this.dispatch("phoenix:navigation-success", { url: target.href, page: envelope });
      return envelope;
    } catch (error) {
      if (!isAbortError(error) && !this.disposed) {
        this.onNavigationError?.(error);
        this.dispatch("phoenix:navigation-error", { url: target.href, error });
      }
      throw error;
    } finally {
      options.signal?.removeEventListener("abort", abortFromCaller);
      if (this.activeController === controller) this.activeController = null;
      this.dispatch("phoenix:navigation-finish", { url: target.href });
    }
  }

  private pageElement(Page: ComponentType<any>, envelope: PageEnvelope): ReactElement {
    return createElement(
      navigationContext.Provider,
      { value: this },
      createElement(
        PhoenixRenderProvider,
        { mode: envelope.render_mode },
        createElement(Page, pageProps(envelope)),
      ),
    );
  }

  private renderPage(Page: ComponentType<any>, envelope: PageEnvelope): void {
    this.currentPage = envelope;
    writePage(this.documentRef, envelope);
    updatePageHead(this.documentRef, envelope.head);
    if (!this.fullRoot) {
      for (const root of this.islandRoots) root.unmount();
      this.islandRoots = [];
      this.rootElement.replaceChildren();
      this.fullRoot = createRoot(this.rootElement);
    }
    const element = this.pageElement(Page, envelope);
    flushSync(() => this.fullRoot?.render(element));
  }

  private installNavigation(): void {
    this.documentRef.addEventListener("click", this.handleDocumentClick);
    this.windowRef.addEventListener("popstate", this.handlePopState);
    this.history.install();
  }

  private readonly handleDocumentClick = (event: MouseEvent) => {
    if (event.defaultPrevented) return;
    const target = event.target as Element | null;
    const anchor = target?.closest?.("a[href]") as HTMLAnchorElement | null;
    if (!anchor || !shouldHandleLink(event, anchor)) return;
    event.preventDefault();
    void this.visit(anchor.href).catch(() => {});
  };

  private readonly handlePopState = (event: PopStateEvent) => {
    void this.performVisit(
      this.windowRef.location.href,
      {},
      "none",
      historySnapshot(event.state),
    ).catch(() => {});
  };

  private dispatch(name: string, detail: Record<string, unknown>): void {
    const event = this.documentRef.createEvent("CustomEvent");
    event.initCustomEvent(name, false, false, detail);
    this.documentRef.dispatchEvent(event);
  }
}

async function hydrateIslands(
  documentRef: Document,
  envelope: PageEnvelope,
  registry: ComponentSource,
  navigator: PhoenixNavigator,
): Promise<Root[]> {
  return Promise.all(envelope.islands.map(async (descriptor) => {
    const root = Array.from(documentRef.querySelectorAll("[data-phoenix-island]"))
      .find((element) => element.getAttribute("data-phoenix-island") === descriptor.id);
    if (!root) throw new Error(`Phoenix island root not found: ${descriptor.id}`);
    const Component = await requiredComponent(registry, descriptor.component, "island");
    return hydrateRoot(
      root,
      createElement(
        navigationContext.Provider,
        { value: navigator },
        createElement(Component, descriptor.props),
      ),
    );
  }));
}

function requiresHardNavigation(current: PageEnvelope, next: PageEnvelope): boolean {
  return (
    current.protocol !== next.protocol ||
    !compatibleIdentity(current.asset_version, next.asset_version) ||
    !compatibleIdentity(current.contract_hash, next.contract_hash)
  );
}

function compatibleIdentity(
  current: string | null | undefined,
  next: string | null | undefined,
): boolean {
  return current == null || current === next;
}
