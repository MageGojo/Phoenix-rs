// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  getPhoenixNavigator,
  Link,
  clearPrefetchCache,
  getPrefetched,
  resetConfirmImplementation,
  setConfirmImplementation,
  startPhoenix,
  stopPhoenix,
  useCsrfToken,
  useNavigator,
  usePage,
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
  clearPrefetchCache();
  resetConfirmImplementation();
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

  it("marks active Links with exact/prefix matching, activeClassName, and aria-current", async () => {
    function NavPage() {
      return (
        <nav>
          <Link id="exact-members" href="/members" activeClassName="is-active">
            Exact
          </Link>
          <Link
            id="prefix-members"
            href="/members"
            match="prefix"
            activeClassName="is-active"
            className="nav-link"
          >
            Prefix
          </Link>
          <Link
            id="forced-active"
            href="/other"
            active
            activeClassName="forced"
            aria-current="true"
          >
            Forced
          </Link>
          <Link
            id="forced-inactive"
            href="/members"
            active={false}
            activeClassName="is-active"
          >
            Forced off
          </Link>
          <Link
            id="aria-off"
            href="/members"
            match="prefix"
            activeClassName="is-active"
            aria-current={false}
          >
            Aria off
          </Link>
        </nav>
      );
    }

    window.history.replaceState(null, "", "/members/42");
    installPage(pageEnvelope("nav", {}));
    await act(async () => {
      await startPhoenix({ pages: { nav: NavPage } });
    });

    const exact = document.getElementById("exact-members");
    const prefix = document.getElementById("prefix-members");
    const forced = document.getElementById("forced-active");
    const forcedOff = document.getElementById("forced-inactive");
    const ariaOff = document.getElementById("aria-off");

    expect(exact?.className).toBe("");
    expect(exact?.getAttribute("aria-current")).toBeNull();
    expect(prefix?.className).toBe("nav-link is-active");
    expect(prefix?.getAttribute("aria-current")).toBe("page");
    expect(forced?.className).toBe("forced");
    expect(forced?.getAttribute("aria-current")).toBe("true");
    expect(forcedOff?.className).toBe("");
    expect(forcedOff?.getAttribute("aria-current")).toBeNull();
    expect(ariaOff?.className).toBe("is-active");
    expect(ariaOff?.getAttribute("aria-current")).toBeNull();
  });

  it("activates exact Links when the pathname matches", async () => {
    function NavPage() {
      return (
        <Link id="home" href="/" activeClassName="here">
          Home
        </Link>
      );
    }

    window.history.replaceState(null, "", "/");
    installPage(pageEnvelope("nav", {}));
    await act(async () => {
      await startPhoenix({ pages: { nav: NavPage } });
    });

    expect(document.getElementById("home")?.className).toBe("here");
    expect(document.getElementById("home")?.getAttribute("aria-current")).toBe("page");
  });

  it("provides page hooks through PhoenixPageProvider after startPhoenix", async () => {
    function HookPage() {
      const { page, props } = usePage<{ title: string }>();
      const csrf = useCsrfToken();
      const navigator = useNavigator();
      return (
        <main
          id="hook-page"
          data-page={page}
          data-title={props.title}
          data-csrf={csrf ?? ""}
          data-has-navigator={navigator ? "1" : "0"}
        />
      );
    }

    const envelope = pageEnvelope("hooks/index", { title: "Hello" });
    envelope.csrf_token = "csrf-from-envelope";
    installPage(envelope);
    await act(async () => {
      await startPhoenix({ pages: { "hooks/index": HookPage } });
    });

    const root = document.getElementById("hook-page");
    expect(root?.dataset.page).toBe("hooks/index");
    expect(root?.dataset.title).toBe("Hello");
    expect(root?.dataset.csrf).toBe("csrf-from-envelope");
    expect(root?.dataset.hasNavigator).toBe("1");
  });

  it("skips Link navigation when confirm is cancelled", async () => {
    const confirmFn = vi.fn(() => false);
    setConfirmImplementation(confirmFn);
    const fetcher = vi.fn(async () => pageResponse(pageEnvelope("members", {})));

    function HomePage() {
      return (
        <main>
          <Link id="members-link" href="/members" confirm="Leave this page?">
            Members
          </Link>
        </main>
      );
    }

    window.history.replaceState(null, "", "/home");
    installPage(pageEnvelope("home", {}));
    await act(async () => {
      await startPhoenix({
        pages: { home: HomePage, members: () => <main>Members</main> },
        fetcher: fetcher as typeof fetch,
      });
    });

    document.getElementById("members-link")?.dispatchEvent(new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      button: 0,
    }));

    expect(confirmFn).toHaveBeenCalledWith("Leave this page?");
    expect(fetcher).not.toHaveBeenCalled();
    expect(window.location.pathname).toBe("/home");
  });

  it("does not preventDefault when Link has no navigation context", async () => {
    const { createRoot } = await import("react-dom/client");
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<Link id="bare-link" href="/x">Bare</Link>);
    });

    const anchor = document.getElementById("bare-link");
    expect(anchor).not.toBeNull();
    const event = new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
      button: 0,
    });
    const preventDefault = vi.spyOn(event, "preventDefault");
    anchor!.dispatchEvent(event);

    expect(preventDefault).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);

    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  it("prefetches on Link hover without writing the current page", async () => {
    const prefetched = pageEnvelope("members/index", { name: "Prefetch" });
    prefetched.csrf_token = "prefetch-csrf";
    const fetchMock = vi.fn(async () => pageResponse(prefetched));
    vi.stubGlobal("fetch", fetchMock);

    function HomePage() {
      return (
        <main>
          <Link id="members-link" href="/members" prefetch="hover">
            Members
          </Link>
        </main>
      );
    }

    window.history.replaceState(null, "", "/home");
    const initial = pageEnvelope("home/index", {});
    initial.csrf_token = "page-csrf";
    installPage(initial);
    await act(async () => {
      await startPhoenix({
        pages: {
          "home/index": HomePage,
          "members/index": () => <main>Members</main>,
        },
      });
    });

    await act(async () => {
      document.getElementById("members-link")?.focus();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(fetchMock).toHaveBeenCalledOnce();
    expect(getPrefetched("/members")?.csrf_token).toBe("prefetch-csrf");
    expect(readInstalledPage().csrf_token).toBe("page-csrf");
  });

  it("prefetches on Link mount", async () => {
    const prefetched = pageEnvelope("posts/show", { id: 1 });
    const fetchMock = vi.fn(async () => pageResponse(prefetched));
    vi.stubGlobal("fetch", fetchMock);

    function HomePage() {
      return (
        <main>
          <Link id="post-link" href="/posts/1" prefetch="mount">
            Post
          </Link>
        </main>
      );
    }

    window.history.replaceState(null, "", "/home");
    installPage(pageEnvelope("home/index", {}));
    await act(async () => {
      await startPhoenix({
        pages: {
          "home/index": HomePage,
          "posts/show": () => <main>Post</main>,
        },
      });
    });

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(fetchMock).toHaveBeenCalled();
    expect(getPrefetched("/posts/1")?.page).toBe("posts/show");
  });

  it("reload with only sends X-Phoenix-Only and merges props", async () => {
    function PostPage({
      title,
      comments,
      sidebar,
    }: {
      title: string;
      comments: Array<{ id: number }>;
      sidebar: string;
    }) {
      return (
        <main
          id="post-page"
          data-title={title}
          data-comments={comments.map((item) => item.id).join(",")}
          data-sidebar={sidebar}
        />
      );
    }

    const initial = pageEnvelope("posts/show", {
      title: "Keep title",
      comments: [] as Array<{ id: number }>,
      sidebar: "keep sidebar",
    });
    const partial = pageEnvelope("posts/show", {
      title: "Should ignore",
      comments: [{ id: 7 }],
      sidebar: "Should ignore",
    });
    partial.shared = { user: "ada" };
    partial.csrf_token = "csrf-partial";

    const fetcher = vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.get("x-phoenix-page")).toBe("1");
      expect(headers.get("x-phoenix-only")).toBe("comments");
      expect(headers.get("x-phoenix-except")).toBeNull();
      return pageResponse(partial);
    });

    window.history.replaceState(null, "", "/posts/1");
    installPage(initial);
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    await act(async () => {
      await startPhoenix({
        pages: { "posts/show": PostPage },
        fetcher: fetcher as typeof fetch,
      });
    });

    await act(async () => {
      await getPhoenixNavigator()?.reload({ only: ["comments"] });
    });

    expect(fetcher).toHaveBeenCalledOnce();
    const page = document.getElementById("post-page");
    expect(page?.dataset.title).toBe("Keep title");
    expect(page?.dataset.comments).toBe("7");
    expect(page?.dataset.sidebar).toBe("keep sidebar");
    expect(readInstalledPage().props).toEqual({
      title: "Keep title",
      comments: [{ id: 7 }],
      sidebar: "keep sidebar",
    });
    expect(readInstalledPage().shared).toEqual({ user: "ada" });
    expect(readInstalledPage().csrf_token).toBe("csrf-partial");
  });
});
