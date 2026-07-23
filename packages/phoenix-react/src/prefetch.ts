import { fetchPage } from "./page-client.js";
import type { DecryptPage, PageEnvelope } from "./protocol.js";

const DEFAULT_TTL_MS = 30_000;

interface PrefetchEntry {
  envelope: PageEnvelope;
  expiresAt: number;
}

export interface PrefetchOptions {
  fetcher?: typeof fetch;
  decrypt?: DecryptPage;
  signal?: AbortSignal;
  ttlMs?: number;
}

const cache = new Map<string, PrefetchEntry>();
const inflight = new Map<string, Promise<PageEnvelope>>();
const controllers = new Map<string, AbortController>();

export async function prefetchPage(
  url: string,
  options: PrefetchOptions = {},
): Promise<PageEnvelope> {
  const key = prefetchCacheKey(url);
  const cached = getPrefetched(key);
  if (cached) return cached;

  const pending = inflight.get(key);
  if (pending) return pending;

  const ttlMs = options.ttlMs ?? DEFAULT_TTL_MS;
  const controller = new AbortController();
  controllers.set(key, controller);
  const abortFromCaller = () => controller.abort();
  if (options.signal?.aborted) controller.abort();
  options.signal?.addEventListener("abort", abortFromCaller, { once: true });

  const promise = fetchPage(key, options.decrypt, options.fetcher ?? fetch, {
    signal: controller.signal,
  })
    .then((envelope) => {
      cache.set(key, {
        envelope,
        expiresAt: Date.now() + ttlMs,
      });
      return envelope;
    })
    .finally(() => {
      options.signal?.removeEventListener("abort", abortFromCaller);
      if (inflight.get(key) === promise) inflight.delete(key);
      if (controllers.get(key) === controller) controllers.delete(key);
    });

  inflight.set(key, promise);
  return promise;
}

export function getPrefetched(url: string): PageEnvelope | undefined {
  const key = prefetchCacheKey(url);
  const entry = cache.get(key);
  if (!entry) return undefined;
  if (Date.now() >= entry.expiresAt) {
    cache.delete(key);
    return undefined;
  }
  return entry.envelope;
}

export function invalidatePrefetch(url?: string): void {
  if (url === undefined) {
    clearPrefetchCache();
    return;
  }
  const key = prefetchCacheKey(url);
  cache.delete(key);
  abortInflight(key);
}

export function clearPrefetchCache(): void {
  cache.clear();
  for (const key of [...controllers.keys()]) abortInflight(key);
}

function abortInflight(key: string): void {
  const controller = controllers.get(key);
  if (controller) {
    controller.abort();
    controllers.delete(key);
  }
  inflight.delete(key);
}

function prefetchCacheKey(url: string): string {
  if (typeof window !== "undefined" && window.location?.href) {
    try {
      return new URL(url, window.location.href).href;
    } catch {
      return url;
    }
  }
  return url;
}
