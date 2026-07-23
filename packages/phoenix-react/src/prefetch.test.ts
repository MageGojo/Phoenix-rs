// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  clearPrefetchCache,
  getPrefetched,
  invalidatePrefetch,
  prefetchPage,
} from "./prefetch.js";
import type { PageEnvelope } from "./protocol.js";
import { pageEnvelope, pageResponse } from "./test-utils.js";

afterEach(() => {
  clearPrefetchCache();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("prefetchPage", () => {
  it("caches successful GET envelopes and does not write the document page", async () => {
    const envelope = pageEnvelope("posts/show", { id: 1 });
    const fetcher = vi.fn(async () => pageResponse(envelope));
    document.body.innerHTML = '<script id="phoenix-page" type="application/json">{"keep":true}</script>';

    const first = await prefetchPage("/posts/1", { fetcher: fetcher as typeof fetch });
    const second = await prefetchPage("/posts/1", { fetcher: fetcher as typeof fetch });

    expect(first).toEqual(envelope);
    expect(second).toEqual(envelope);
    expect(fetcher).toHaveBeenCalledOnce();
    expect(getPrefetched("/posts/1")).toEqual(envelope);
    expect(document.getElementById("phoenix-page")?.textContent).toBe('{"keep":true}');
  });

  it("dedupes in-flight requests for the same URL", async () => {
    let resolveFetch!: (value: Response) => void;
    const fetcher = vi.fn(() => new Promise<Response>((resolve) => {
      resolveFetch = resolve;
    }));

    const first = prefetchPage("/posts/2", { fetcher: fetcher as typeof fetch });
    const second = prefetchPage("/posts/2", { fetcher: fetcher as typeof fetch });
    expect(fetcher).toHaveBeenCalledOnce();

    resolveFetch(pageResponse(pageEnvelope("posts/show", { id: 2 })));
    const [a, b] = await Promise.all([first, second]);
    expect(a.props).toEqual({ id: 2 });
    expect(b.props).toEqual({ id: 2 });
  });

  it("expires cache entries after TTL and supports invalidate/clear", async () => {
    vi.useFakeTimers();
    const envelope = pageEnvelope("posts/show", { id: 3 });
    const fetcher = vi.fn(async () => pageResponse(envelope));

    await prefetchPage("/posts/3", {
      fetcher: fetcher as typeof fetch,
      ttlMs: 1_000,
    });
    expect(getPrefetched("/posts/3")).toEqual(envelope);

    await vi.advanceTimersByTimeAsync(1_001);
    expect(getPrefetched("/posts/3")).toBeUndefined();

    await prefetchPage("/posts/3", {
      fetcher: fetcher as typeof fetch,
      ttlMs: 5_000,
    });
    expect(getPrefetched("/posts/3")).toEqual(envelope);
    invalidatePrefetch("/posts/3");
    expect(getPrefetched("/posts/3")).toBeUndefined();

    await prefetchPage("/posts/3", { fetcher: fetcher as typeof fetch });
    clearPrefetchCache();
    expect(getPrefetched("/posts/3")).toBeUndefined();
  });

  it("does not apply prefetched csrf to the current document", async () => {
    const envelope = {
      ...pageEnvelope("secure/index", {}),
      csrf_token: "prefetch-csrf",
    } satisfies PageEnvelope;
    const fetcher = vi.fn(async () => pageResponse(envelope));
    document.body.innerHTML =
      '<script id="phoenix-page" type="application/json">{"csrf_token":"page-csrf"}</script>';

    await prefetchPage("/secure", { fetcher: fetcher as typeof fetch });

    expect(getPrefetched("/secure")?.csrf_token).toBe("prefetch-csrf");
    expect(document.getElementById("phoenix-page")?.textContent).toContain("page-csrf");
  });
});
