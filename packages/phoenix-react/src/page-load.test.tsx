// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  getPhoenixNavigator,
  loadPageComponent,
  PageLoadTimeoutError,
  startPhoenix,
  stopPhoenix,
} from "./index.js";
import {
  installPage,
  pageEnvelope,
  pageResponse,
} from "./test-utils.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

afterEach(async () => {
  await act(async () => stopPhoenix());
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("loadPageComponent", () => {
  it("times out slow loaders", async () => {
    vi.useFakeTimers();
    const pending = loadPageComponent(
      {
        slow: {
          load: () => new Promise(() => {}),
        },
      },
      "slow",
      { timeoutMs: 50 },
    );

    const assertion = expect(pending).rejects.toBeInstanceOf(PageLoadTimeoutError);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    await assertion;
  });
});

describe("page load fallback", () => {
  it("renders retry UI on start timeout and recovers after retry", async () => {
    vi.useFakeTimers();
    let attempts = 0;
    function HomePage() {
      return <main id="home">Home</main>;
    }

    installPage(pageEnvelope("home/index", {}));
    const startPromise = startPhoenix({
      pageLoadTimeoutMs: 40,
      pages: {
        "home/index": {
          load: async () => {
            attempts += 1;
            if (attempts === 1) {
              await new Promise(() => {});
            }
            return { default: HomePage };
          },
        },
      },
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(40);
      await startPromise;
    });

    expect(document.querySelector("[data-phoenix-page-load-error]")).not.toBeNull();
    expect(document.querySelector("#home")).toBeNull();

    await act(async () => {
      document.querySelector("[data-phoenix-page-load-error] button")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });

    expect(document.querySelector("#home")?.textContent).toBe("Home");
    expect(document.querySelector("[data-phoenix-page-load-error]")).toBeNull();
    expect(attempts).toBe(2);
  });

  it("renders retry UI when visit page load fails", async () => {
    function HomePage() {
      return <main id="home">Home</main>;
    }
    function MembersPage() {
      return <main id="members">Members</main>;
    }

    let membersLoads = 0;
    const members = pageEnvelope("members/index", {});
    const fetcher = vi.fn(async () => pageResponse(members));
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});

    installPage(pageEnvelope("home/index", {}));
    await act(async () => {
      await startPhoenix({
        pages: {
          "home/index": HomePage,
          "members/index": {
            load: async () => {
              membersLoads += 1;
              if (membersLoads === 1) {
                throw new Error("chunk failed");
              }
              return { default: MembersPage };
            },
          },
        },
        fetcher: fetcher as typeof fetch,
        onNavigationError: () => {},
      });
    });

    await act(async () => {
      await expect(getPhoenixNavigator()?.visit("/members")).rejects.toThrow("chunk failed");
    });

    expect(document.querySelector("[data-phoenix-page-load-error]")?.textContent)
      .toContain("chunk failed");
    expect(document.querySelector("#members")).toBeNull();

    await act(async () => {
      document.querySelector("[data-phoenix-page-load-error] button")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });

    expect(document.querySelector("#members")?.textContent).toBe("Members");
    expect(window.location.pathname).toBe("/members");
    expect(membersLoads).toBe(2);
  });
});
