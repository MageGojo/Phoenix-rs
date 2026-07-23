// @vitest-environment jsdom
import { act, createElement, useEffect, type ReactElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PhoenixNavigator } from "./navigation.js";
import {
  normalizePathname,
  pathMatches,
  PhoenixPageProvider,
  useCsrfToken,
  useFlash,
  useNavigating,
  useNavigator,
  usePage,
  useShared,
  type PageHookValue,
} from "./page-state.js";
import type { PageEnvelope } from "./protocol.js";
import { pageEnvelope } from "./test-utils.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let container: HTMLDivElement | null = null;

function render(ui: ReactElement): void {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root?.render(ui);
  });
}

function stubNavigator(envelope: PageEnvelope): PhoenixNavigator {
  return {
    get page() {
      return envelope;
    },
    visit: async () => envelope,
    reload: async () => envelope,
    dispose() {},
  };
}

function Provider({
  envelope,
  children,
}: {
  envelope: PageEnvelope;
  children?: ReactNode;
}): ReactElement {
  return createElement(
    PhoenixPageProvider,
    { envelope, navigator: stubNavigator(envelope) },
    children,
  );
}

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  root = null;
  container?.remove();
  container = null;
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("page state hooks", () => {
  it("reads page, shared, and csrf values inside PhoenixPageProvider", () => {
    const envelope = pageEnvelope("members/index", { name: "Ada" });
    envelope.shared = { user: "ada" };
    envelope.csrf_token = "csrf-token";
    envelope.errors = { name: ["required"] };
    envelope.routes = { members: "/members" };
    envelope.flash = { notice: "Saved" };

    let snapshot: PageHookValue<{ name: string }> | undefined;
    let shared: { user: string } | undefined;
    let csrf: string | null = "unset";
    let navigator: PhoenixNavigator | undefined;

    function Probe() {
      snapshot = usePage<{ name: string }>();
      shared = useShared<{ user: string }>();
      csrf = useCsrfToken();
      navigator = useNavigator();
      return createElement(
        "div",
        null,
        createElement("span", { id: "page" }, snapshot.page),
        createElement("span", { id: "props" }, snapshot.props.name),
        createElement("span", { id: "shared" }, shared.user),
        createElement("span", { id: "csrf" }, csrf ?? ""),
      );
    }

    render(createElement(Provider, { envelope }, createElement(Probe)));

    expect(document.getElementById("page")?.textContent).toBe("members/index");
    expect(document.getElementById("props")?.textContent).toBe("Ada");
    expect(document.getElementById("shared")?.textContent).toBe("ada");
    expect(document.getElementById("csrf")?.textContent).toBe("csrf-token");
    expect(snapshot?.errors).toEqual({ name: ["required"] });
    expect(snapshot?.routes).toEqual({ members: "/members" });
    expect(snapshot?.envelope).toBe(envelope);
    expect(navigator?.page).toBe(envelope);
  });

  it("throws outside PhoenixPageProvider", () => {
    let pageError: unknown;
    let sharedError: unknown;
    let csrfError: unknown;
    let navigatorError: unknown;
    let flashError: unknown;

    function Probe() {
      try {
        usePage();
      } catch (error) {
        pageError = error;
      }
      try {
        useShared();
      } catch (error) {
        sharedError = error;
      }
      try {
        useCsrfToken();
      } catch (error) {
        csrfError = error;
      }
      try {
        useNavigator();
      } catch (error) {
        navigatorError = error;
      }
      try {
        useFlash();
      } catch (error) {
        flashError = error;
      }
      return null;
    }

    render(createElement(Probe));

    expect(pageError).toEqual(new Error("Phoenix page context is not available"));
    expect(sharedError).toEqual(new Error("Phoenix page context is not available"));
    expect(csrfError).toEqual(new Error("Phoenix page context is not available"));
    expect(flashError).toEqual(new Error("Phoenix page context is not available"));
    expect(navigatorError).toEqual(new Error("Phoenix navigator is not available"));
  });

  it("masks flash keys with consume and restores them after a new envelope", () => {
    const first = pageEnvelope("home", {});
    first.flash = { notice: "Hello", alert: "Careful" };
    const second = pageEnvelope("home", {});
    second.flash = { notice: "Again", alert: "Careful" };

    let consumeFlash: ((...keys: string[]) => void) | null = null;
    let visible: Record<string, unknown> = {};

    function Probe() {
      const { flash, consume } = useFlash<{ notice?: string; alert?: string }>();
      consumeFlash = consume;
      visible = flash as Record<string, unknown>;
      return createElement(
        "div",
        null,
        createElement("span", { id: "notice" }, String(flash.notice ?? "")),
        createElement("span", { id: "alert" }, String(flash.alert ?? "")),
      );
    }

    render(createElement(Provider, { envelope: first }, createElement(Probe)));
    expect(document.getElementById("notice")?.textContent).toBe("Hello");
    expect(document.getElementById("alert")?.textContent).toBe("Careful");

    act(() => {
      consumeFlash?.("notice");
    });
    expect(document.getElementById("notice")?.textContent).toBe("");
    expect(document.getElementById("alert")?.textContent).toBe("Careful");
    expect(visible).toEqual({ alert: "Careful" });

    act(() => {
      root?.render(createElement(Provider, { envelope: second }, createElement(Probe)));
    });
    expect(document.getElementById("notice")?.textContent).toBe("Again");
    expect(document.getElementById("alert")?.textContent).toBe("Careful");
  });

  it("tracks navigating progress from custom document events", () => {
    vi.useFakeTimers();
    let state = { processing: false, progress: 0, url: null as string | null };

    function Probe() {
      state = useNavigating();
      useEffect(() => {}, [state.processing, state.progress, state.url]);
      return createElement(
        "div",
        null,
        createElement("span", { id: "processing" }, String(state.processing)),
        createElement("span", { id: "progress" }, String(state.progress)),
        createElement("span", { id: "url" }, state.url ?? ""),
      );
    }

    render(createElement(Probe));
    expect(state).toEqual({ processing: false, progress: 0, url: null });

    act(() => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-start", {
        detail: { url: "https://example.test/members" },
      }));
    });
    expect(state.processing).toBe(true);
    expect(state.url).toBe("https://example.test/members");
    expect(state.progress).toBeGreaterThan(0);
    expect(state.progress).toBeLessThan(0.9);

    const beforeTick = state.progress;
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(state.progress).toBeGreaterThan(beforeTick);
    expect(state.progress).toBeLessThan(0.9);

    act(() => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-finish", {
        detail: { url: "https://example.test/members" },
      }));
    });
    expect(state.processing).toBe(true);
    expect(state.progress).toBe(1);

    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(state).toEqual({ processing: false, progress: 0, url: null });
  });
});

describe("path matching helpers", () => {
  it("normalizes trailing slashes", () => {
    expect(normalizePathname("/")).toBe("/");
    expect(normalizePathname("/members/")).toBe("/members");
    expect(normalizePathname("/members/?q=1")).toBe("/members");
    expect(normalizePathname("https://example.test/members/")).toBe("/members");
  });

  it("matches exact and prefix paths", () => {
    expect(pathMatches("/members", "/members", "exact")).toBe(true);
    expect(pathMatches("/members/", "/members", "exact")).toBe(true);
    expect(pathMatches("/members/1", "/members", "exact")).toBe(false);

    expect(pathMatches("/members/1", "/members", "prefix")).toBe(true);
    expect(pathMatches("/members", "/members", "prefix")).toBe(true);
    expect(pathMatches("/membership", "/members", "prefix")).toBe(false);
    expect(pathMatches("/members", "/", "prefix")).toBe(true);
    expect(pathMatches("/", "/", "prefix")).toBe(true);
    expect(pathMatches("/", "/", "exact")).toBe(true);
  });
});
