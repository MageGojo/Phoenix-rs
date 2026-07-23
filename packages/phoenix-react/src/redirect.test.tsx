// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  getPhoenixNavigator,
  redirect,
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
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("redirect", () => {
  it("navigates with replace: true by default", async () => {
    function HomePage() {
      return <main>Home</main>;
    }
    function NextPage() {
      return <main>Next</main>;
    }

    window.history.replaceState(null, "", "/home");
    installPage(pageEnvelope("home", {}));
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const pushState = vi.spyOn(window.history, "pushState");
    const replaceState = vi.spyOn(window.history, "replaceState");

    await act(async () => {
      await startPhoenix({
        pages: { home: HomePage, next: NextPage },
        fetcher: vi.fn(async () => pageResponse(pageEnvelope("next", {}))) as typeof fetch,
      });
    });

    const visit = vi.spyOn(getPhoenixNavigator()!, "visit");
    await act(async () => {
      await redirect("/next");
    });

    expect(visit).toHaveBeenCalledWith("/next", expect.objectContaining({ replace: true }));
    expect(pushState).not.toHaveBeenCalled();
    expect(replaceState).toHaveBeenCalled();
    expect(window.location.pathname).toBe("/next");
  });

  it("allows options to override replace", async () => {
    function HomePage() {
      return <main>Home</main>;
    }
    function NextPage() {
      return <main>Next</main>;
    }

    window.history.replaceState(null, "", "/home");
    installPage(pageEnvelope("home", {}));
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});

    await act(async () => {
      await startPhoenix({
        pages: { home: HomePage, next: NextPage },
        fetcher: vi.fn(async () => pageResponse(pageEnvelope("next", {}))) as typeof fetch,
      });
    });

    const visit = vi.spyOn(getPhoenixNavigator()!, "visit");
    await act(async () => {
      await redirect("/next", { replace: false });
    });

    expect(visit).toHaveBeenCalledWith("/next", expect.objectContaining({ replace: false }));
    expect(window.location.pathname).toBe("/next");
  });
});
