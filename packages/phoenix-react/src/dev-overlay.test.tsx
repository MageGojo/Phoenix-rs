// @vitest-environment jsdom
import { act, createElement, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PhoenixDevOverlay } from "./dev-overlay.js";
import { installPage, pageEnvelope } from "./test-utils.js";

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

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  root = null;
  container?.remove();
  container = null;
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("PhoenixDevOverlay", () => {
  it("renders nothing when disabled", () => {
    installPage(pageEnvelope("home", {}));
    render(createElement(PhoenixDevOverlay, { enabled: false }));
    expect(document.querySelector("[data-phoenix-dev-overlay]")).toBeNull();
  });

  it("shows page identity, url, and reverse route when enabled", () => {
    window.history.replaceState(null, "", "/members");
    const envelope = pageEnvelope("members/index", {});
    envelope.contract_hash = "contract-abc";
    envelope.asset_version = "asset-1";
    envelope.routes = {
      home: "/",
      members: "/members",
      "posts.show": "/posts/:id",
    };
    installPage(envelope);

    render(createElement(PhoenixDevOverlay, { enabled: true }));
    const overlay = document.querySelector("[data-phoenix-dev-overlay]");
    expect(overlay).not.toBeNull();
    expect(overlay?.textContent).toContain("page: members/index");
    expect(overlay?.textContent).toContain("contract: contract-abc");
    expect(overlay?.textContent).toContain("asset: asset-1");
    expect(overlay?.textContent).toContain("route: members");
    expect(overlay?.textContent).toMatch(/url: .*\/members/);
  });

  it("tracks lastVisitUrl from navigation events", () => {
    installPage(pageEnvelope("home", {}));
    window.history.replaceState(null, "", "/home");
    render(createElement(PhoenixDevOverlay, { enabled: true }));

    act(() => {
      const event = document.createEvent("CustomEvent");
      event.initCustomEvent("phoenix:navigation-start", false, false, {
        url: "https://example.test/next",
      });
      document.dispatchEvent(event);
    });

    const overlay = document.querySelector("[data-phoenix-dev-overlay]");
    expect(overlay?.textContent).toContain("last: https://example.test/next");
  });
});
