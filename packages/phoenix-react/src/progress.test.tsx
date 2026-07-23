// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ProgressBar } from "./progress.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root;
let container: HTMLDivElement;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  vi.useRealTimers();
  await act(async () => {
    root.unmount();
  });
  container.remove();
  document.body.innerHTML = "";
});

function dispatchNavigation(name: string): void {
  document.dispatchEvent(new CustomEvent(name, { detail: { url: "/members" } }));
}

function progressElement(): Element | null {
  return document.querySelector("[data-phoenix-progress]");
}

describe("ProgressBar", () => {
  it("shows a loading bar when phoenix:navigation-start fires", async () => {
    await act(async () => {
      root.render(<ProgressBar />);
    });
    expect(progressElement()).toBeNull();

    await act(async () => {
      dispatchNavigation("phoenix:navigation-start");
    });

    const bar = progressElement();
    expect(bar).not.toBeNull();
    expect(bar?.getAttribute("data-status")).toBe("loading");
    expect((bar as HTMLElement).style.width).not.toBe("0%");
    expect((bar as HTMLElement).style.position).toBe("fixed");
  });

  it("moves to finishing then idle after phoenix:navigation-finish", async () => {
    vi.useFakeTimers();

    await act(async () => {
      root.render(<ProgressBar hideDelayMs={200} />);
    });

    await act(async () => {
      dispatchNavigation("phoenix:navigation-start");
    });
    expect(progressElement()?.getAttribute("data-status")).toBe("loading");

    await act(async () => {
      dispatchNavigation("phoenix:navigation-finish");
    });

    const finishing = progressElement();
    expect(finishing?.getAttribute("data-status")).toBe("finishing");
    expect((finishing as HTMLElement).style.width).toBe("100%");

    await act(async () => {
      vi.advanceTimersByTime(200);
    });

    expect(progressElement()).toBeNull();
  });
});
