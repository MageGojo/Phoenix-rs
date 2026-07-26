// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CaptchaImage, StoredCaptchaImage } from "./captcha.js";
import { registerRouteManifest, resetRouteManifest } from "./urls.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let container: HTMLDivElement | null = null;

function render(element: React.ReactElement): void {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root?.render(element));
}

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
  resetRouteManifest();
});

describe("CaptchaImage", () => {
  it("resolves the challenge URL from the captcha.image named route", () => {
    registerRouteManifest({ "captcha.image": "/captcha" });
    render(<CaptchaImage />);
    const image = container?.querySelector<HTMLImageElement>("img[data-phoenix-captcha]");
    expect(image).not.toBeNull();
    expect(image?.getAttribute("src")).toMatch(/^\/captcha\?t=\d+$/);
    expect(image?.getAttribute("alt")).toBe("captcha");
  });

  it("loads a fresh challenge when clicked", () => {
    registerRouteManifest({ "captcha.image": "/captcha" });
    render(<CaptchaImage alt="验证码" />);
    const image = container?.querySelector<HTMLImageElement>("img");
    const initial = image?.getAttribute("src");
    act(() => {
      image?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(image?.getAttribute("src")).toMatch(/^\/captcha\?t=\d+$/);
    expect(image?.getAttribute("src")).not.toBe(initial);
    expect(image?.getAttribute("alt")).toBe("验证码");
  });

  it("honours a custom route name", () => {
    registerRouteManifest({ "kaptcha.image": "/kaptcha" });
    render(<CaptchaImage route="kaptcha.image" />);
    const image = container?.querySelector<HTMLImageElement>("img");
    expect(image?.getAttribute("src")).toMatch(/^\/kaptcha\?t=\d+$/);
  });
});

describe("StoredCaptchaImage", () => {
  const svg = '<svg xmlns="http://www.w3.org/2000/svg"><text x="1">a</text></svg>';

  beforeEach(() => {
    registerRouteManifest({ "captcha.challenge": "/captcha/challenge" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function stubChallenges(...ids: string[]): { calls: string[] } {
    const calls: string[] = [];
    let next = 0;
    vi.stubGlobal("fetch", (url: string) => {
      calls.push(url);
      const id = ids[Math.min(next++, ids.length - 1)];
      return Promise.resolve({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ id, svg, expires_in: 300 }),
      } as Response);
    });
    return { calls };
  }

  async function flush(): Promise<void> {
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  }

  it("loads a challenge and inlines the SVG as a data URL", async () => {
    const { calls } = stubChallenges("challenge-1");
    const seen: string[] = [];
    render(<StoredCaptchaImage onChallenge={(id) => seen.push(id)} />);
    await flush();

    expect(calls[0]).toMatch(/^\/captcha\/challenge\?t=\d+$/);
    const image = container?.querySelector<HTMLImageElement>("img[data-phoenix-captcha]");
    expect(image?.getAttribute("src")).toBe(
      `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`,
    );
    expect(image?.getAttribute("data-phoenix-captcha-id")).toBe("challenge-1");
    expect(seen).toEqual(["challenge-1"]);
  });

  it("fetches a new challenge id when clicked", async () => {
    const { calls } = stubChallenges("challenge-1", "challenge-2");
    const seen: string[] = [];
    render(<StoredCaptchaImage onChallenge={(id) => seen.push(id)} />);
    await flush();

    const image = container?.querySelector<HTMLImageElement>("img");
    await act(async () => {
      image?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await flush();

    expect(calls).toHaveLength(2);
    expect(image?.getAttribute("data-phoenix-captcha-id")).toBe("challenge-2");
    expect(seen).toEqual(["challenge-1", "challenge-2"]);
  });

  it("drops the challenge when the request fails", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve({ ok: false, status: 500, json: () => Promise.resolve({}) } as Response),
    );
    const seen: string[] = [];
    render(<StoredCaptchaImage onChallenge={(id) => seen.push(id)} />);
    await flush();

    const image = container?.querySelector<HTMLImageElement>("img");
    expect(image?.getAttribute("src") ?? "").toBe("");
    expect(image?.hasAttribute("data-phoenix-captcha-id")).toBe(false);
    expect(seen).toEqual([]);
  });
});
