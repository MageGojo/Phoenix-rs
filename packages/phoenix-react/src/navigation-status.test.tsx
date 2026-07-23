// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { NavigationStatusBanner } from "./navigation-status.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root;
let container: HTMLDivElement;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  Object.defineProperty(window.navigator, "onLine", {
    configurable: true,
    get: () => true,
  });
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  document.body.innerHTML = "";
});

function banner(): Element | null {
  return container.querySelector("[data-phoenix-navigation-status]");
}

describe("NavigationStatusBanner", () => {
  it("shows navigation errors and clears on start/success", async () => {
    await act(async () => {
      root.render(<NavigationStatusBanner />);
    });
    expect(banner()).toBeNull();

    await act(async () => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-error", {
        detail: { error: new Error("visit failed") },
      }));
    });

    expect(banner()?.getAttribute("data-phoenix-navigation-status")).toBe("error");
    expect(banner()?.textContent).toContain("visit failed");

    await act(async () => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-start", {
        detail: { url: "/next" },
      }));
    });
    expect(banner()).toBeNull();

    await act(async () => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-error", {
        detail: { error: new Error("again") },
      }));
    });
    expect(banner()?.getAttribute("data-phoenix-navigation-status")).toBe("error");

    await act(async () => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-success", {
        detail: { url: "/next" },
      }));
    });
    expect(banner()).toBeNull();
  });

  it("shows offline status from window offline/online events", async () => {
    await act(async () => {
      root.render(<NavigationStatusBanner offlineMessage="Offline now" />);
    });

    await act(async () => {
      window.dispatchEvent(new Event("offline"));
    });
    expect(banner()?.getAttribute("data-phoenix-navigation-status")).toBe("offline");
    expect(banner()?.textContent).toBe("Offline now");

    await act(async () => {
      document.dispatchEvent(new CustomEvent("phoenix:navigation-error", {
        detail: { error: new Error("hidden while offline") },
      }));
    });
    expect(banner()?.getAttribute("data-phoenix-navigation-status")).toBe("offline");

    await act(async () => {
      window.dispatchEvent(new Event("online"));
    });
    expect(banner()?.getAttribute("data-phoenix-navigation-status")).toBe("error");
    expect(banner()?.textContent).toContain("hidden while offline");
  });
});
