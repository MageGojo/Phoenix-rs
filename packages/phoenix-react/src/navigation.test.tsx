// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  getPhoenixNavigator,
  Link,
  startPhoenix,
  stopPhoenix,
  type PageEnvelope,
} from "./index.js";
import {
  installPage,
  nextNavigation,
  pageEnvelope,
  pageResponse,
  readInstalledPage,
} from "./test-utils.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

afterEach(async () => {
  await act(async () => stopPhoenix());
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("Phoenix navigation", () => {
  it("intercepts same-origin Link navigation, replaces the page, and updates History and Head", async () => {
    function HomePage() {
      return (
        <main id="home-main">
          <h1>Home</h1>
          <Link id="members-link" href="/members">Members</Link>
          <Link id="external-link" href="https://outside.example.test/members">External</Link>
          <Link id="reload-link" href="/reload" reloadDocument>Reload</Link>
        </main>
      );
    }
    function MembersPage({ name }: { name: string }) {
      return <main id="members-main"><h1>{name}</h1></main>;
    }
    function SettingsPage() {
      return <main id="settings-main"><h1>Settings</h1></main>;
    }

    const members = pageEnvelope("members/index", { name: "Ada" }, {
      title: "Members",
      description: "Member directory",
      canonical: "https://example.test/members",
      robots: "index,follow",
      open_graph: {
        title: "Member graph",
        description: "Member graph description",
        image: "https://example.test/member.png",
        kind: "website",
      },
    });
    const settings = pageEnvelope("settings/index", {}, { title: "Settings" });
    const fetcher = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      expect(new Headers(init?.headers).get("x-phoenix-page")).toBe("1");
      const pathname = new URL(url.toString()).pathname;
      return pageResponse(pathname === "/members" ? members : settings);
    });
    window.history.replaceState(null, "", "/home");
    installPage(pageEnvelope("home/index", {}, { title: "Home" }));
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const pushState = vi.spyOn(window.history, "pushState");
    const replaceState = vi.spyOn(window.history, "replaceState");

    await act(async () => {
      await startPhoenix({
        pages: {
          "home/index": HomePage,
          "members/index": MembersPage,
          "settings/index": SettingsPage,
        },
        fetcher: fetcher as typeof fetch,
      });
    });

    document.addEventListener("click", (event) => event.preventDefault(), { once: true });
    document.getElementById("external-link")?.dispatchEvent(new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      button: 0,
    }));
    document.addEventListener("click", (event) => event.preventDefault(), { once: true });
    document.getElementById("reload-link")?.dispatchEvent(new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      button: 0,
    }));
    expect(fetcher).not.toHaveBeenCalled();

    const finished = nextNavigation("phoenix:navigation-success");
    await act(async () => {
      document.getElementById("members-link")?.dispatchEvent(new MouseEvent("click", {
        bubbles: true,
        cancelable: true,
        button: 0,
      }));
      await finished;
    });

    expect(fetcher).toHaveBeenCalledOnce();
    expect(window.location.pathname).toBe("/members");
    expect(pushState).toHaveBeenCalled();
    expect(document.getElementById("phoenix-root")?.textContent).toContain("Ada");
    expect(document.title).toBe("Members");
    expect(document.querySelector('meta[name="description"]')?.getAttribute("content"))
      .toBe("Member directory");
    expect(document.querySelector('link[rel="canonical"]')?.getAttribute("href"))
      .toBe("https://example.test/members");
    expect(document.querySelector('meta[property="og:type"]')?.getAttribute("content"))
      .toBe("website");
    expect(readInstalledPage().page).toBe("members/index");
    expect(scrollTo).toHaveBeenCalledWith(0, 0);
    expect(document.activeElement?.id).toBe("members-main");

    pushState.mockClear();
    replaceState.mockClear();
    await act(async () => {
      await getPhoenixNavigator()?.visit("/settings", { replace: true });
    });

    expect(window.location.pathname).toBe("/settings");
    expect(pushState).not.toHaveBeenCalled();
    expect(replaceState).toHaveBeenCalled();
    expect(document.title).toBe("Settings");
    expect(document.querySelector('meta[name="description"]')).toBeNull();
    expect(document.querySelector('link[rel="canonical"]')).toBeNull();
    expect(document.querySelector('meta[property="og:title"]')).toBeNull();
  });

  it("preserves unmarked document chrome while removing stale managed Head tags", async () => {
    function HomePage() {
      return <main>Home</main>;
    }
    document.head.innerHTML = [
      '<title>Custom shell</title>',
      '<meta name="description" content="Custom description">',
      '<title data-phoenix-head>Stale page title</title>',
      '<meta data-phoenix-head name="description" content="Stale page description">',
    ].join("");
    installPage(pageEnvelope("home", {}));

    await act(async () => {
      await startPhoenix({ pages: { home: HomePage } });
    });

    expect(document.querySelector("title:not([data-phoenix-head])")?.textContent)
      .toBe("Custom shell");
    expect(document.querySelector('meta:not([data-phoenix-head])[name="description"]')
      ?.getAttribute("content")).toBe("Custom description");
    expect(document.querySelector("title[data-phoenix-head]")).toBeNull();
    expect(document.querySelector('meta[data-phoenix-head][name="description"]')).toBeNull();
  });

  it("aborts the previous visit and ignores a stale response when navigation races", async () => {
    function NamedPage({ name }: { name: string }) {
      return <main><h1>{name}</h1></main>;
    }
    window.history.replaceState(null, "", "/home");
    installPage({ ...pageEnvelope("home", { name: "Home" }), asset_version: "assets-current" });
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});

    let resolveSlow: ((response: Response) => void) | undefined;
    let slowSignal: AbortSignal | null = null;
    const hardNavigate = vi.fn();
    const fetcher = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const pathname = new URL(url.toString()).pathname;
      if (pathname === "/slow") {
        slowSignal = init?.signal ?? null;
        return new Promise<Response>((resolve) => {
          resolveSlow = resolve;
        });
      }
      return Promise.resolve(pageResponse({
        ...pageEnvelope("fast", { name: "Fast" }),
        asset_version: "assets-current",
      }));
    });
    await act(async () => {
      await startPhoenix({
        pages: { home: NamedPage, slow: NamedPage, fast: NamedPage },
        fetcher: fetcher as typeof fetch,
        hardNavigate,
      });
    });
    const navigator = getPhoenixNavigator();
    expect(navigator).not.toBeNull();

    const slowResult = navigator!.visit("/slow").catch((error: unknown) => error);
    await act(async () => {
      await navigator!.visit("/fast");
    });
    expect((slowSignal as AbortSignal | null)?.aborted).toBe(true);
    resolveSlow?.(pageResponse({
      ...pageEnvelope("slow", { name: "Slow" }),
      asset_version: "assets-stale",
    }));
    const staleError = await slowResult;

    expect(staleError).toMatchObject({ name: "AbortError" });
    expect(window.location.pathname).toBe("/fast");
    expect(hardNavigate).not.toHaveBeenCalled();
    expect(document.getElementById("phoenix-root")?.textContent).toContain("Fast");
    expect(document.getElementById("phoenix-root")?.textContent).not.toContain("Slow");
  });

  it("moves an Islands document to a managed full-page root on navigation", async () => {
    function Counter({ count }: { count: number }) {
      return <button id="counter">{count}</button>;
    }
    function NextPage() {
      return <main id="next-main"><h1>Managed page</h1></main>;
    }

    const initial: PageEnvelope = {
      ...pageEnvelope("article", {}),
      render_mode: "islands",
      islands: [{ id: "counter-island", component: "counter", props: { count: 3 } }],
    };
    installPage(
      initial,
      '<article>Server article<div data-phoenix-island="counter-island"><button id="counter">3</button></div></article>',
    );
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const fetcher = vi.fn(async () => pageResponse(pageEnvelope("next", {})));
    await act(async () => {
      await startPhoenix({
        pages: { next: NextPage },
        islands: { counter: Counter },
        fetcher: fetcher as typeof fetch,
      });
    });
    expect(document.getElementById("counter")?.textContent).toBe("3");

    await act(async () => {
      await getPhoenixNavigator()?.visit("/next");
    });

    expect(document.getElementById("counter")).toBeNull();
    expect(document.getElementById("phoenix-root")?.textContent).toBe("Managed page");
    expect(document.activeElement?.id).toBe("next-main");
  });

  it("restores scroll and focus after popstate navigation", async () => {
    function FirstPage() {
      return (
        <main id="first-main">
          <h1>First</h1>
          <button id="first-focus">Edit</button>
        </main>
      );
    }
    function SecondPage() {
      return <main id="second-main"><h1>Second</h1></main>;
    }

    let scrollX = 0;
    let scrollY = 0;
    vi.spyOn(window, "scrollX", "get").mockImplementation(() => scrollX);
    vi.spyOn(window, "scrollY", "get").mockImplementation(() => scrollY);
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation((x, y) => {
      scrollX = Number(x);
      scrollY = Number(y);
    });
    const fetcher = vi.fn(async (url: string | URL | Request) => {
      const pathname = new URL(url.toString()).pathname;
      return pageResponse(pathname === "/first"
        ? pageEnvelope("first", {})
        : pageEnvelope("second", {}));
    });
    window.history.replaceState(null, "", "/first");
    installPage(pageEnvelope("first", {}));
    await act(async () => {
      await startPhoenix({
        pages: { first: FirstPage, second: SecondPage },
        fetcher: fetcher as typeof fetch,
      });
    });

    scrollX = 12;
    scrollY = 160;
    document.getElementById("first-focus")?.focus();
    await act(async () => {
      await getPhoenixNavigator()?.visit("/second");
    });
    expect(document.activeElement?.id).toBe("second-main");

    const restored = nextNavigation("phoenix:navigation-success");
    await act(async () => {
      window.history.back();
      await restored;
    });

    expect(window.location.pathname).toBe("/first");
    expect(scrollTo).toHaveBeenLastCalledWith(12, 160);
    expect(document.activeElement?.id).toBe("first-focus");
  });

  it.each([
    "protocol",
    "asset_version",
    "asset_version_missing",
    "contract_hash",
    "contract_hash_missing",
  ] as const)(
    "uses a hard navigation before loading a component when %s is incompatible",
    async (identity) => {
      function HomePage() {
        return <main>Home</main>;
      }
      function NextPage() {
        return <main>Next</main>;
      }

      const initial = {
        ...pageEnvelope("home", {}),
        asset_version: "assets-a",
        contract_hash: "contract-a",
      };
      const next = {
        ...pageEnvelope("next", {}),
        asset_version: identity === "asset_version"
          ? "assets-b"
          : identity === "asset_version_missing" ? null : "assets-a",
        contract_hash: identity === "contract_hash"
          ? "contract-b"
          : identity === "contract_hash_missing" ? null : "contract-a",
        protocol: identity === "protocol" ? 2 : 1,
      } as unknown as PageEnvelope;
      const loadNext = vi.fn(async () => ({ default: NextPage }));
      const hardNavigate = vi.fn();
      const pushState = vi.spyOn(window.history, "pushState");
      window.history.replaceState(null, "", "/home");
      installPage(initial);

      await act(async () => {
        await startPhoenix({
          pages: { home: HomePage, next: { load: loadNext } },
          fetcher: vi.fn(async () => pageResponse(next)) as typeof fetch,
          hardNavigate,
        });
      });
      await act(async () => {
        await getPhoenixNavigator()?.visit("/next");
      });

      expect(hardNavigate).toHaveBeenCalledOnce();
      expect(hardNavigate).toHaveBeenCalledWith("http://localhost:3000/next");
      expect(loadNext).not.toHaveBeenCalled();
      expect(pushState).not.toHaveBeenCalled();
      expect(window.location.pathname).toBe("/home");
      expect(readInstalledPage().page).toBe("home");
      expect(document.getElementById("phoenix-root")?.textContent).toBe("Home");
    },
  );

  it("keeps local navigation when a previously unknown asset and contract identity appears", async () => {
    function HomePage() {
      return <main>Home</main>;
    }
    function NextPage() {
      return <main>Next</main>;
    }
    const next = {
      ...pageEnvelope("next", {}),
      asset_version: "assets-a",
      contract_hash: "contract-a",
    };
    const hardNavigate = vi.fn();
    window.history.replaceState(null, "", "/home");
    installPage(pageEnvelope("home", {}));
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});

    await act(async () => {
      await startPhoenix({
        pages: { home: HomePage, next: NextPage },
        fetcher: vi.fn(async () => pageResponse(next)) as typeof fetch,
        hardNavigate,
      });
      await getPhoenixNavigator()?.visit("/next");
    });

    expect(hardNavigate).not.toHaveBeenCalled();
    expect(readInstalledPage()).toMatchObject({
      page: "next",
      asset_version: "assets-a",
      contract_hash: "contract-a",
    });
    expect(document.getElementById("phoenix-root")?.textContent).toBe("Next");
  });
});
