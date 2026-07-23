import {
  type AnchorHTMLAttributes,
  createContext,
  createElement,
  type ComponentType,
  type FocusEvent as ReactFocusEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
  useCallback,
  useContext,
  useEffect,
  useRef,
} from "react";
import { flushSync } from "react-dom";
import { createRoot, hydrateRoot, type Root } from "react-dom/client";

import { confirmAction } from "./confirm.js";
import {
  PhoenixErrorBoundary,
  type ErrorFallbackProps,
} from "./error-boundary.js";
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
import { fetchPage, submitPage } from "./page-client.js";
import {
  DEFAULT_PAGE_LOAD_TIMEOUT_MS,
  DefaultPageLoadFallback,
  loadPageComponent,
  toPageLoadError,
  type PageLoadFallbackProps,
} from "./page-load.js";
import { mergePageEnvelope } from "./partial.js";
import { invalidatePrefetch, prefetchPage } from "./prefetch.js";
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
import { pathMatches, PhoenixPageProvider } from "./page-state.js";
import {
  componentRegistry,
  pageProps,
  PhoenixRenderProvider,
  requiredComponent,
} from "./rendering.js";

export type VisitMethod = "get" | "post" | "put" | "patch" | "delete";

export interface VisitOptions {
  replace?: boolean;
  preserveScroll?: boolean;
  preserveFocus?: boolean;
  signal?: AbortSignal;
  method?: VisitMethod;
  data?: Record<string, unknown>;
  only?: string[];
  except?: string[];
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
  errorFallback?: ComponentType<ErrorFallbackProps>;
  pageLoadTimeoutMs?: number;
  pageLoadFallback?: ComponentType<PageLoadFallbackProps>;
}

interface PageLoadRetryContext {
  envelope: PageEnvelope;
  target?: URL;
  options?: VisitOptions;
  historyMode?: HistoryMode;
  restoration?: HistorySnapshot;
  initial?: boolean;
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

export type LinkPrefetchMode = false | "hover" | "mount" | "viewport";

export interface LinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> {
  href: string;
  replace?: boolean;
  preserveScroll?: boolean;
  preserveFocus?: boolean;
  reloadDocument?: boolean;
  match?: "exact" | "prefix";
  activeClassName?: string;
  active?: boolean;
  confirm?: string;
  prefetch?: LinkPrefetchMode;
}

export function Link({
  href,
  replace = false,
  preserveScroll = false,
  preserveFocus = false,
  reloadDocument = false,
  match = "exact",
  activeClassName,
  active,
  confirm,
  prefetch = false,
  className,
  onClick,
  onPointerEnter,
  onFocus,
  "aria-current": ariaCurrent,
  ...props
}: LinkProps): ReactElement {
  const navigator = useContext(navigationContext);
  const anchorRef = useRef<HTMLAnchorElement>(null);
  const location = currentLocation();
  const hrefPath = location
    ? new URL(href, location.href).pathname
    : href;
  const currentPath = location?.pathname ?? "/";
  const isActive = active ?? pathMatches(currentPath, hrefPath, match);
  const mergedClassName = [className, isActive ? activeClassName : undefined]
    .filter(Boolean)
    .join(" ") || undefined;
  const resolvedAriaCurrent = ariaCurrent !== undefined
    ? ariaCurrent
    : (isActive ? "page" : undefined);

  const runPrefetch = useCallback(() => {
    if (prefetch === false) return;
    void prefetchPage(href).catch(() => {});
  }, [href, prefetch]);

  useEffect(() => {
    if (prefetch !== "mount") return;
    runPrefetch();
  }, [prefetch, runPrefetch]);

  useEffect(() => {
    if (prefetch !== "viewport") return;
    const element = anchorRef.current;
    if (!element) return;
    if (typeof IntersectionObserver === "undefined") {
      runPrefetch();
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        runPrefetch();
        observer.disconnect();
      },
      { rootMargin: "200px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [prefetch, runPrefetch]);

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
    if (confirm && !confirmAction(confirm)) return;
    void navigator.visit(href, { replace, preserveScroll, preserveFocus }).catch(() => {});
  };
  const handlePointerEnter = (event: ReactPointerEvent<HTMLAnchorElement>) => {
    onPointerEnter?.(event);
    if (prefetch === "hover") runPrefetch();
  };
  const handleFocus = (event: ReactFocusEvent<HTMLAnchorElement>) => {
    onFocus?.(event);
    if (prefetch === "hover") runPrefetch();
  };
  return createElement("a", {
    ...props,
    ref: anchorRef,
    href,
    className: mergedClassName,
    "aria-current": resolvedAriaCurrent === false ? undefined : resolvedAriaCurrent,
    onClick: handleClick,
    onPointerEnter: handlePointerEnter,
    onFocus: handleFocus,
    ...(reloadDocument ? { "data-phoenix-reload": "" } : {}),
  });
}

function currentLocation(): Location | null {
  if (typeof window !== "undefined") return window.location;
  return null;
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
  private readonly errorFallback?: ComponentType<ErrorFallbackProps>;
  private readonly pageLoadTimeoutMs: number;
  private readonly pageLoadFallback: ComponentType<PageLoadFallbackProps>;
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
    this.errorFallback = options.errorFallback;
    this.pageLoadTimeoutMs = options.pageLoadTimeoutMs ?? DEFAULT_PAGE_LOAD_TIMEOUT_MS;
    this.pageLoadFallback = options.pageLoadFallback ?? DefaultPageLoadFallback;
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
      try {
        const Page = await loadPageComponent(this.pages, this.currentPage.page, {
          timeoutMs: this.pageLoadTimeoutMs,
          fallback: this.pageLoadFallback,
        });
        const element = this.pageElement(Page, this.currentPage);
        if (this.currentPage.render_mode === "ssr") {
          this.fullRoot = hydrateRoot(this.rootElement, element);
        } else {
          this.fullRoot = createRoot(this.rootElement);
          this.fullRoot.render(element);
        }
      } catch (error) {
        if (this.disposed) throw error;
        this.renderPageLoadFallback(error, {
          envelope: this.currentPage,
          initial: true,
        });
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

    const method = (options.method ?? "get").toLowerCase() as VisitMethod;
    if (
      historyMode !== "none" &&
      method === "get" &&
      isHashOnlyVisit(target, this.windowRef.location)
    ) {
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
      let envelope = method === "get"
        ? await fetchPage(target.href, this.decrypt, this.fetcher, {
          signal: controller.signal,
          only: options.only,
          except: options.except,
        })
        : await submitPage(target.href, {
          method,
          data: options.data,
          signal: controller.signal,
          decrypt: this.decrypt,
          fetcher: this.fetcher,
          headers: csrfHeaders(this.currentPage),
          only: options.only,
          except: options.except,
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
      if (options.only || options.except) {
        envelope = mergePageEnvelope(this.currentPage, envelope, options);
      }
      let Page: ComponentType<any>;
      try {
        Page = await loadPageComponent(this.pages, envelope.page, {
          timeoutMs: this.pageLoadTimeoutMs,
          fallback: this.pageLoadFallback,
        });
      } catch (loadError) {
        if (!isAbortError(loadError) && !this.disposed) {
          this.renderPageLoadFallback(loadError, {
            envelope,
            target,
            options,
            historyMode,
            restoration,
          });
        }
        throw loadError;
      }
      if (navigation !== this.navigationSequence || controller.signal.aborted) {
        throw abortError();
      }

      this.commitVisit(Page, envelope, target, options, historyMode, restoration);
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
      PhoenixPageProvider,
      { envelope, navigator: this },
      createElement(
        navigationContext.Provider,
        { value: this },
        createElement(
          PhoenixRenderProvider,
          { mode: envelope.render_mode },
          createElement(
            PhoenixErrorBoundary,
            { fallback: this.errorFallback },
            createElement(Page, pageProps(envelope)),
          ),
        ),
      ),
    );
  }

  private commitVisit(
    Page: ComponentType<any>,
    envelope: PageEnvelope,
    target: URL,
    options: VisitOptions,
    historyMode: HistoryMode,
    restoration?: HistorySnapshot,
  ): void {
    const previousSnapshot = this.history.capture();
    if (historyMode !== "none") {
      this.history.save();
      this.history.write(historyMode, target, nextHistorySnapshot(previousSnapshot, options));
    }
    this.renderPage(Page, envelope);
    this.history.restore(target, options, restoration, previousSnapshot);
    this.history.save();
    invalidatePrefetch(target.href);
    this.dispatch("phoenix:navigation-success", { url: target.href, page: envelope });
  }

  private renderPage(Page: ComponentType<any>, envelope: PageEnvelope): void {
    this.currentPage = envelope;
    writePage(this.documentRef, envelope);
    updatePageHead(this.documentRef, envelope.head);
    this.ensureFullRoot();
    const element = this.pageElement(Page, envelope);
    flushSync(() => this.fullRoot?.render(element));
  }

  private renderPageLoadFallback(error: unknown, context: PageLoadRetryContext): void {
    const loadError = toPageLoadError(error);
    const Fallback = this.pageLoadFallback;
    const retry = () => {
      void (async () => {
        if (this.disposed) return;
        try {
          const Page = await loadPageComponent(this.pages, context.envelope.page, {
            timeoutMs: this.pageLoadTimeoutMs,
            fallback: this.pageLoadFallback,
          });
          if (context.initial || !context.target || !context.options || !context.historyMode) {
            if (context.initial && this.currentPage.render_mode === "ssr" && !this.fullRoot) {
              this.fullRoot = hydrateRoot(
                this.rootElement,
                this.pageElement(Page, context.envelope),
              );
            } else {
              this.renderPage(Page, context.envelope);
            }
            return;
          }
          this.commitVisit(
            Page,
            context.envelope,
            context.target,
            context.options,
            context.historyMode,
            context.restoration,
          );
        } catch (retryError) {
          if (!this.disposed) this.renderPageLoadFallback(retryError, context);
        }
      })();
    };
    this.ensureFullRoot();
    flushSync(() => {
      this.fullRoot?.render(createElement(Fallback, { error: loadError, retry }));
    });
  }

  private ensureFullRoot(): Root {
    if (!this.fullRoot) {
      for (const root of this.islandRoots) root.unmount();
      this.islandRoots = [];
      this.rootElement.replaceChildren();
      this.fullRoot = createRoot(this.rootElement);
    }
    return this.fullRoot;
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
        PhoenixPageProvider,
        { envelope, navigator },
        createElement(
          navigationContext.Provider,
          { value: navigator },
          createElement(Component, descriptor.props),
        ),
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

function csrfHeaders(page: PageEnvelope): Record<string, string> | undefined {
  if (!page.csrf_token) return undefined;
  return { "X-CSRF-Token": page.csrf_token };
}
