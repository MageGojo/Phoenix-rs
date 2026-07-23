import { createElement, type ComponentType } from "react";

import type { ComponentSource } from "./protocol.js";
import { requiredComponent } from "./rendering.js";

export const DEFAULT_PAGE_LOAD_TIMEOUT_MS = 15_000;

export interface PageLoadFallbackProps {
  error: Error;
  retry: () => void;
}

export interface LoadPageComponentOptions {
  timeoutMs?: number;
  kind?: string;
  /** Reserved for callers; load itself only applies timeout. */
  fallback?: ComponentType<PageLoadFallbackProps>;
}

export class PageLoadTimeoutError extends Error {
  readonly name = "PageLoadTimeoutError";
  readonly pageName: string;
  readonly timeoutMs: number;

  constructor(pageName: string, timeoutMs: number, kind = "page") {
    super(`Phoenix ${kind} load timed out after ${timeoutMs}ms: ${pageName}`);
    this.pageName = pageName;
    this.timeoutMs = timeoutMs;
  }
}

export function DefaultPageLoadFallback({ error, retry }: PageLoadFallbackProps) {
  return createElement(
    "div",
    { "data-phoenix-page-load-error": "" },
    createElement("p", null, error.message),
    createElement(
      "button",
      { type: "button", onClick: () => retry() },
      "Retry",
    ),
  );
}

export async function loadPageComponent(
  registry: ComponentSource,
  name: string,
  options: LoadPageComponentOptions = {},
): Promise<ComponentType<any>> {
  const timeoutMs = options.timeoutMs ?? DEFAULT_PAGE_LOAD_TIMEOUT_MS;
  const kind = options.kind ?? "page";
  const loadPromise = requiredComponent(registry, name, kind);

  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    return loadPromise;
  }

  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      loadPromise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          reject(new PageLoadTimeoutError(name, timeoutMs, kind));
        }, timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

export function toPageLoadError(error: unknown): Error {
  if (error instanceof Error) return error;
  return new Error(String(error));
}
