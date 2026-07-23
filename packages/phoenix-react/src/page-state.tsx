import {
  createContext,
  createElement,
  type ReactElement,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import type { PhoenixNavigator } from "./navigation.js";
import type { PageEnvelope, PageHead, RenderMode } from "./protocol.js";

export interface PhoenixPageState {
  envelope: PageEnvelope;
  navigator: PhoenixNavigator;
}

export interface PageHookValue<TProps = unknown> {
  page: string;
  props: TProps;
  shared: Record<string, unknown>;
  flash: Record<string, unknown>;
  errors: Record<string, unknown>;
  routes: Record<string, string>;
  envelope: PageEnvelope;
  head: PageHead | undefined;
  csrf_token: string | null;
  contract_hash: string | null;
  asset_version: string | null;
  request_id: string | null;
  render_mode: RenderMode;
  islands: PageEnvelope["islands"];
  protocol: 1;
}

export interface FlashHookValue<T = Record<string, unknown>> {
  flash: T;
  consume: (...keys: string[]) => void;
}

export interface NavigatingState {
  processing: boolean;
  progress: number;
  url: string | null;
}

const pageContext = createContext<PageEnvelope | null>(null);
const navigatorContext = createContext<PhoenixNavigator | null>(null);

const PROGRESS_INTERVAL_MS = 100;
const PROGRESS_CEILING = 0.9;
const PROGRESS_RESET_MS = 200;

export function PhoenixPageProvider({
  envelope,
  navigator,
  children,
}: {
  envelope: PageEnvelope;
  navigator: PhoenixNavigator;
  children?: ReactNode;
}): ReactElement {
  return createElement(
    pageContext.Provider,
    { value: envelope },
    createElement(navigatorContext.Provider, { value: navigator }, children),
  );
}

export function NavigatorProvider({
  navigator,
  children,
}: {
  navigator: PhoenixNavigator;
  children?: ReactNode;
}): ReactElement {
  return createElement(navigatorContext.Provider, { value: navigator }, children);
}

export function usePage<TProps = unknown>(): PageHookValue<TProps> {
  const envelope = useContext(pageContext);
  if (!envelope) {
    throw new Error("Phoenix page context is not available");
  }
  return {
    page: envelope.page,
    props: envelope.props as TProps,
    shared: envelope.shared,
    flash: envelope.flash,
    errors: envelope.errors,
    routes: envelope.routes,
    envelope,
    head: envelope.head,
    csrf_token: envelope.csrf_token ?? null,
    contract_hash: envelope.contract_hash,
    asset_version: envelope.asset_version,
    request_id: envelope.request_id,
    render_mode: envelope.render_mode,
    islands: envelope.islands,
    protocol: envelope.protocol,
  };
}

export function useShared<T = Record<string, unknown>>(): T {
  const envelope = useContext(pageContext);
  if (!envelope) {
    throw new Error("Phoenix page context is not available");
  }
  return envelope.shared as T;
}

export function useFlash<T = Record<string, unknown>>(): FlashHookValue<T> {
  const envelope = useContext(pageContext);
  const [consumed, setConsumed] = useState(() => new Set<string>());

  useEffect(() => {
    setConsumed(new Set());
  }, [envelope]);

  const flash = useMemo(() => {
    if (!envelope) return {} as T;
    const next = { ...envelope.flash } as T & Record<string, unknown>;
    for (const key of consumed) {
      delete next[key];
    }
    return next as T;
  }, [envelope, consumed]);

  if (!envelope) {
    throw new Error("Phoenix page context is not available");
  }

  const consume = (...keys: string[]) => {
    setConsumed((previous) => {
      const next = new Set(previous);
      for (const key of keys) next.add(key);
      return next;
    });
  };

  return { flash, consume };
}

export function useCsrfToken(): string | null {
  const envelope = useContext(pageContext);
  if (!envelope) {
    throw new Error("Phoenix page context is not available");
  }
  return envelope.csrf_token ?? null;
}

export function useNavigator(): PhoenixNavigator {
  const navigator = useContext(navigatorContext);
  if (!navigator) {
    throw new Error("Phoenix navigator is not available");
  }
  return navigator;
}

export function useNavigating(documentRef: Document = document): NavigatingState {
  const [state, setState] = useState<NavigatingState>({
    processing: false,
    progress: 0,
    url: null,
  });

  useEffect(() => {
    let progressTimer: ReturnType<typeof setInterval> | null = null;
    let resetTimer: ReturnType<typeof setTimeout> | null = null;

    const clearProgressTimer = () => {
      if (progressTimer != null) {
        clearInterval(progressTimer);
        progressTimer = null;
      }
    };

    const clearResetTimer = () => {
      if (resetTimer != null) {
        clearTimeout(resetTimer);
        resetTimer = null;
      }
    };

    const onStart = (event: Event) => {
      clearResetTimer();
      clearProgressTimer();
      const detail = (event as CustomEvent<{ url?: string }>).detail;
      const url = detail?.url ?? null;
      setState({ processing: true, progress: 0.1, url });
      progressTimer = setInterval(() => {
        setState((previous) => ({
          ...previous,
          progress: previous.progress + (PROGRESS_CEILING - previous.progress) * 0.15,
        }));
      }, PROGRESS_INTERVAL_MS);
    };

    const onFinish = (event: Event) => {
      clearProgressTimer();
      clearResetTimer();
      const detail = (event as CustomEvent<{ url?: string }>).detail;
      const url = detail?.url ?? null;
      setState((previous) => ({
        processing: true,
        progress: 1,
        url: url ?? previous.url,
      }));
      resetTimer = setTimeout(() => {
        setState({ processing: false, progress: 0, url: null });
        resetTimer = null;
      }, PROGRESS_RESET_MS);
    };

    documentRef.addEventListener("phoenix:navigation-start", onStart);
    documentRef.addEventListener("phoenix:navigation-success", onFinish);
    documentRef.addEventListener("phoenix:navigation-hard", onFinish);
    documentRef.addEventListener("phoenix:navigation-error", onFinish);
    documentRef.addEventListener("phoenix:navigation-finish", onFinish);

    return () => {
      clearProgressTimer();
      clearResetTimer();
      documentRef.removeEventListener("phoenix:navigation-start", onStart);
      documentRef.removeEventListener("phoenix:navigation-success", onFinish);
      documentRef.removeEventListener("phoenix:navigation-hard", onFinish);
      documentRef.removeEventListener("phoenix:navigation-error", onFinish);
      documentRef.removeEventListener("phoenix:navigation-finish", onFinish);
    };
  }, [documentRef]);

  return state;
}

export function normalizePathname(path: string): string {
  let pathname = path;
  if (/^https?:\/\//i.test(path) || path.startsWith("//")) {
    try {
      pathname = new URL(path, "http://phoenix.invalid").pathname;
    } catch {
      pathname = path.split(/[?#]/, 1)[0] || "/";
    }
  } else {
    pathname = path.split(/[?#]/, 1)[0] || "/";
  }
  if (pathname.length > 1 && pathname.endsWith("/")) {
    pathname = pathname.slice(0, -1);
  }
  return pathname || "/";
}

export function pathMatches(
  currentPath: string,
  hrefPath: string,
  match: "exact" | "prefix",
): boolean {
  const current = normalizePathname(currentPath);
  const href = normalizePathname(hrefPath);
  if (match === "exact") return current === href;
  if (href === "/") return true;
  return current === href || current.startsWith(`${href}/`);
}
