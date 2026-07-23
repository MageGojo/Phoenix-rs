// @vitest-environment jsdom
import { act, createElement, type ReactElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PhoenixNavigator } from "./navigation.js";
import { PhoenixPageProvider } from "./page-state.js";
import type { PageEnvelope } from "./protocol.js";
import { pageEnvelope } from "./test-utils.js";
import { WhenVisible } from "./when-visible.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let container: HTMLDivElement | null = null;

type ObserverCallback = IntersectionObserverCallback;

let observerInstances: MockIntersectionObserver[] = [];

class MockIntersectionObserver implements IntersectionObserver {
  readonly root: Element | Document | null = null;
  readonly rootMargin: string;
  readonly thresholds: ReadonlyArray<number>;
  private readonly callback: ObserverCallback;
  private targets = new Set<Element>();

  constructor(callback: ObserverCallback, options?: IntersectionObserverInit) {
    this.callback = callback;
    this.rootMargin = options?.rootMargin ?? "0px";
    this.thresholds = Array.isArray(options?.threshold)
      ? options.threshold
      : [options?.threshold ?? 0];
    observerInstances.push(this);
  }

  observe(target: Element): void {
    this.targets.add(target);
  }

  unobserve(target: Element): void {
    this.targets.delete(target);
  }

  disconnect(): void {
    this.targets.clear();
  }

  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }

  trigger(isIntersecting = true): void {
    const entries = Array.from(this.targets).map((target) => ({
      isIntersecting,
      target,
      intersectionRatio: isIntersecting ? 1 : 0,
      time: 0,
      boundingClientRect: target.getBoundingClientRect(),
      intersectionRect: target.getBoundingClientRect(),
      rootBounds: null,
    })) as IntersectionObserverEntry[];
    this.callback(entries, this);
  }
}

function render(ui: ReactElement): void {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root?.render(ui);
  });
}

function stubNavigator(
  envelope: PageEnvelope,
  reload: PhoenixNavigator["reload"] = async () => envelope,
): PhoenixNavigator {
  return {
    get page() {
      return envelope;
    },
    visit: async () => envelope,
    reload,
    dispose() {},
  };
}

function Provider({
  envelope,
  navigator,
  children,
}: {
  envelope: PageEnvelope;
  navigator: PhoenixNavigator;
  children?: ReactNode;
}): ReactElement {
  return createElement(
    PhoenixPageProvider,
    { envelope, navigator },
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
  observerInstances = [];
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("WhenVisible", () => {
  it("reloads only the listed prop key on first intersection", async () => {
    vi.stubGlobal("IntersectionObserver", MockIntersectionObserver);

    const initial = pageEnvelope("posts/show", {
      title: "Post",
      comments: undefined,
    });
    const afterReload = pageEnvelope("posts/show", {
      title: "Post",
      comments: [{ id: 1, body: "hi" }],
    });

    const reload = vi.fn(async () => afterReload);
    const navigator = stubNavigator(initial, reload);

    function App() {
      return createElement(
        Provider,
        { envelope: initial, navigator },
        createElement(
          WhenVisible,
          {
            data: "comments",
            rootMargin: "100px",
            fallback: createElement("span", { id: "fallback" }, "loading"),
            children: (value: unknown) => createElement(
              "ul",
              { id: "comments" },
              Array.isArray(value)
                ? value.map((item: { id: number }) =>
                  createElement("li", { key: item.id }, String(item.id)))
                : null,
            ),
          },
        ),
      );
    }

    render(createElement(App));

    expect(document.getElementById("fallback")?.textContent).toBe("loading");
    expect(reload).not.toHaveBeenCalled();
    expect(observerInstances).toHaveLength(1);
    expect(observerInstances[0]?.rootMargin).toBe("100px");

    let resolveReload!: (envelope: typeof afterReload) => void;
    reload.mockImplementationOnce(() => new Promise<typeof afterReload>((resolve) => {
      resolveReload = resolve;
    }));

    await act(async () => {
      observerInstances[0]?.trigger(true);
    });

    expect(reload).toHaveBeenCalledWith({
      only: ["comments"],
      preserveScroll: true,
      preserveFocus: true,
    });
    expect(document.getElementById("fallback")?.textContent).toBe("loading");
    expect(
      container?.querySelector("[data-phoenix-when-visible-status]")
        ?.getAttribute("data-phoenix-when-visible-status"),
    ).toBe("loading");

    await act(async () => {
      resolveReload(afterReload);
    });

    // Provider still has initial envelope in this unit test; children still render
    // after status flips to loaded (value read from current provider props).
    expect(
      container?.querySelector("[data-phoenix-when-visible-status]")
        ?.getAttribute("data-phoenix-when-visible-status"),
    ).toBe("loaded");
    expect(document.getElementById("fallback")).toBeNull();
    expect(document.getElementById("comments")).not.toBeNull();
  });

  it("records error status when reload fails", async () => {
    vi.stubGlobal("IntersectionObserver", MockIntersectionObserver);

    const envelope = pageEnvelope("posts/show", { comments: undefined });
    const reload = vi.fn(async () => {
      throw new Error("network down");
    });

    render(createElement(
      Provider,
      { envelope, navigator: stubNavigator(envelope, reload) },
      createElement(
        WhenVisible,
        {
          data: "comments",
          fallback: createElement("span", { id: "fallback" }, "wait"),
          children: () => createElement("div", { id: "ok" }, "ok"),
        },
      ),
    ));

    await act(async () => {
      observerInstances[0]?.trigger(true);
    });

    expect(
      container?.querySelector("[data-phoenix-when-visible-status]")
        ?.getAttribute("data-phoenix-when-visible-status"),
    ).toBe("error");
    expect(container?.querySelector("[data-phoenix-when-visible-error]")).not.toBeNull();
    expect(document.getElementById("ok")?.textContent).toBe("ok");
  });
});
