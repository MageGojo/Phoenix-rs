// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { Pagination, paginationWindow } from "./pagination.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

describe("paginationWindow", () => {
  it("returns every page when the range is small", () => {
    expect(paginationWindow(1, 3)).toEqual([1, 2, 3]);
    expect(paginationWindow(2, 1)).toEqual([1]);
  });

  it("collapses distant ranges with ellipses on both sides", () => {
    expect(paginationWindow(10, 20)).toEqual([1, "…", 8, 9, 10, 11, 12, "…", 20]);
    expect(paginationWindow(1, 20)).toEqual([1, 2, 3, "…", 20]);
    expect(paginationWindow(20, 20)).toEqual([1, "…", 18, 19, 20]);
  });

  it("respects the siblings option", () => {
    expect(paginationWindow(10, 20, 1)).toEqual([1, "…", 9, 10, 11, "…", 20]);
  });
});

describe("Pagination", () => {
  let root: Root | null = null;
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    act(() => root?.unmount());
    container?.remove();
    root = null;
    container = null;
  });

  function render(element: React.ReactElement): void {
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    act(() => root?.render(element));
  }

  it("renders windowed links with prev/next and marks the current page", () => {
    render(
      <Pagination
        meta={{ currentPage: 5, lastPage: 9 }}
        href={(page) => `/members?page=${page}`}
      />,
    );
    const nav = container?.querySelector("nav[data-phoenix-pagination]");
    expect(nav).not.toBeNull();
    const prev = nav?.querySelector('a[rel="prev"]');
    const next = nav?.querySelector('a[rel="next"]');
    expect(prev?.getAttribute("href")).toBe("/members?page=4");
    expect(next?.getAttribute("href")).toBe("/members?page=6");
    const current = nav?.querySelector('a[aria-current="page"]');
    expect(current?.getAttribute("href")).toBe("/members?page=5");
    expect(current?.textContent).toBe("5");
    expect(nav?.textContent).toContain("…");
  });

  it("hides prev on the first page and renders nothing for a single page", () => {
    render(
      <Pagination meta={{ currentPage: 1, lastPage: 3 }} href={(page) => `/p/${page}`} />,
    );
    expect(container?.querySelector('a[rel="prev"]')).toBeNull();
    expect(container?.querySelector('a[rel="next"]')).not.toBeNull();

    act(() => root?.unmount());
    render(
      <Pagination meta={{ currentPage: 1, lastPage: 1 }} href={(page) => `/p/${page}`} />,
    );
    expect(container?.querySelector("nav[data-phoenix-pagination]")).toBeNull();
  });
});
