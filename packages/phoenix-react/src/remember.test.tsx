// @vitest-environment jsdom
import { act, createElement, useState, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  clearRemembered,
  readRemembered,
  rememberKey,
  useRemember,
  writeRemembered,
} from "./remember.js";

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
  sessionStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("remember helpers", () => {
  it("builds storage keys and round-trips JSON values", () => {
    expect(rememberKey("posts.create")).toBe("phoenix:remember:posts.create");
    writeRemembered(rememberKey("draft"), { title: "Hello" });
    expect(readRemembered<{ title: string }>(rememberKey("draft")))
      .toEqual({ title: "Hello" });
    clearRemembered(rememberKey("draft"));
    expect(readRemembered(rememberKey("draft"))).toBeUndefined();
  });

  it("returns undefined for invalid JSON", () => {
    sessionStorage.setItem(rememberKey("broken"), "{not-json");
    expect(readRemembered(rememberKey("broken"))).toBeUndefined();
  });
});

describe("useRemember", () => {
  it("restores remembered data on mount", () => {
    const key = rememberKey("note");
    writeRemembered(key, { text: "saved" });

    function Draft() {
      const [data, setData] = useState({ text: "" });
      useRemember(key, data, setData);
      return createElement("span", { id: "text" }, data.text);
    }

    render(createElement(Draft));
    expect(document.getElementById("text")?.textContent).toBe("saved");
  });

  it("writes after debounce when data changes", () => {
    vi.useFakeTimers();
    const key = rememberKey("counter");
    let setDataRef: ((value: { n: number }) => void) | null = null;

    function Counter() {
      const [data, setData] = useState({ n: 0 });
      setDataRef = setData;
      useRemember(key, data, setData);
      return createElement("span", { id: "n" }, String(data.n));
    }

    render(createElement(Counter));
    act(() => {
      vi.advanceTimersByTime(0);
    });
    act(() => {
      setDataRef?.({ n: 3 });
    });
    expect(readRemembered(key)).toBeUndefined();
    act(() => {
      vi.advanceTimersByTime(299);
    });
    expect(readRemembered(key)).toBeUndefined();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(readRemembered<{ n: number }>(key)).toEqual({ n: 3 });
  });

  it("flushes the latest dirty value on unmount", () => {
    vi.useFakeTimers();
    const key = rememberKey("flush");
    let setDataRef: ((value: { v: string }) => void) | null = null;

    function Draft() {
      const [data, setData] = useState({ v: "a" });
      setDataRef = setData;
      useRemember(key, data, setData);
      return null;
    }

    render(createElement(Draft));
    act(() => {
      vi.advanceTimersByTime(0);
    });
    act(() => {
      setDataRef?.({ v: "b" });
    });
    act(() => {
      root?.unmount();
      root = null;
    });
    expect(readRemembered(key)).toEqual({ v: "b" });
  });

  it("does not rewrite after clearRemembered", () => {
    vi.useFakeTimers();
    const key = rememberKey("cleared");
    let setDataRef: ((value: { v: string }) => void) | null = null;

    function Draft() {
      const [data, setData] = useState({ v: "a" });
      setDataRef = setData;
      useRemember(key, data, setData);
      return null;
    }

    render(createElement(Draft));
    act(() => {
      vi.advanceTimersByTime(0);
    });
    act(() => {
      setDataRef?.({ v: "draft" });
    });
    act(() => {
      clearRemembered(key);
      vi.advanceTimersByTime(300);
    });
    expect(readRemembered(key)).toBeUndefined();
  });
});
