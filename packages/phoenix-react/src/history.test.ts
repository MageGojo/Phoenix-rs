// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import {
  captureScrollRegions,
  historySnapshot,
  nextHistorySnapshot,
  restoreScrollRegions,
  type HistorySnapshot,
} from "./history.js";

describe("scroll regions", () => {
  it("captures named and unnamed scroll regions", () => {
    document.body.innerHTML = `
      <div data-phoenix-scroll-region="main" id="main"></div>
      <div data-phoenix-scroll-region id="side"></div>
      <div data-phoenix-scroll-region="aside" id="aside"></div>
    `;
    const main = document.getElementById("main") as HTMLElement;
    const side = document.getElementById("side") as HTMLElement;
    const aside = document.getElementById("aside") as HTMLElement;
    Object.defineProperty(main, "scrollLeft", { configurable: true, value: 10, writable: true });
    Object.defineProperty(main, "scrollTop", { configurable: true, value: 20, writable: true });
    Object.defineProperty(side, "scrollLeft", { configurable: true, value: 3, writable: true });
    Object.defineProperty(side, "scrollTop", { configurable: true, value: 4, writable: true });
    Object.defineProperty(aside, "scrollLeft", { configurable: true, value: 7, writable: true });
    Object.defineProperty(aside, "scrollTop", { configurable: true, value: 8, writable: true });

    expect(captureScrollRegions(document)).toEqual({
      main: [10, 20],
      "0": [3, 4],
      aside: [7, 8],
    });
  });

  it("restores scroll regions and tolerates missing keys", () => {
    document.body.innerHTML = `
      <div data-phoenix-scroll-region="main" id="main"></div>
      <div data-phoenix-scroll-region="aside" id="aside"></div>
    `;
    const main = document.getElementById("main") as HTMLElement;
    const aside = document.getElementById("aside") as HTMLElement;
    let mainLeft = 0;
    let mainTop = 0;
    let asideLeft = 0;
    let asideTop = 0;
    Object.defineProperty(main, "scrollLeft", {
      configurable: true,
      get: () => mainLeft,
      set: (value: number) => {
        mainLeft = value;
      },
    });
    Object.defineProperty(main, "scrollTop", {
      configurable: true,
      get: () => mainTop,
      set: (value: number) => {
        mainTop = value;
      },
    });
    Object.defineProperty(aside, "scrollLeft", {
      configurable: true,
      get: () => asideLeft,
      set: (value: number) => {
        asideLeft = value;
      },
    });
    Object.defineProperty(aside, "scrollTop", {
      configurable: true,
      get: () => asideTop,
      set: (value: number) => {
        asideTop = value;
      },
    });

    restoreScrollRegions(document, {
      main: [12, 34],
      missing: [1, 2],
    });

    expect(mainLeft).toBe(12);
    expect(mainTop).toBe(34);
    expect(asideLeft).toBe(0);
    expect(asideTop).toBe(0);
  });

  it("parses optional regions and preserves them for preserveScroll snapshots", () => {
    const withRegions = historySnapshot({
      __phoenix: {
        version: 1,
        key: "phoenix-1",
        scroll: [1, 2],
        focus: null,
        regions: { main: [9, 8] },
      },
    });
    expect(withRegions?.regions).toEqual({ main: [9, 8] });

    const legacy = historySnapshot({
      __phoenix: {
        version: 1,
        key: "phoenix-2",
        scroll: [0, 0],
        focus: null,
      },
    });
    expect(legacy?.regions).toBeUndefined();

    const invalid = historySnapshot({
      __phoenix: {
        version: 1,
        key: "phoenix-3",
        scroll: [0, 0],
        focus: null,
        regions: { main: [1] },
      },
    });
    expect(invalid).toBeUndefined();

    const previous: HistorySnapshot = {
      version: 1,
      key: "phoenix-4",
      scroll: [5, 6],
      focus: null,
      regions: { main: [11, 22] },
    };
    expect(nextHistorySnapshot(previous, { preserveScroll: true }).regions)
      .toEqual({ main: [11, 22] });
    expect(nextHistorySnapshot(previous, {}).regions).toBeUndefined();
  });
});
