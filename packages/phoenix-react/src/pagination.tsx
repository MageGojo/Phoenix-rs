import { createElement, Fragment, type ReactElement, type ReactNode } from "react";

import { Link, type LinkProps } from "./navigation.js";

/** Shape of `phoenix_database::PageMeta` after serde camelCase serialization. */
export interface PageMeta {
  currentPage: number;
  perPage: number;
  total: number;
  lastPage: number;
}

export interface PaginatedData<T> {
  data: T[];
  meta: PageMeta;
}

/** Shape of `phoenix_database::CursorPageMeta`. */
export interface CursorPageMeta {
  perPage: number;
  nextCursor: string | null;
}

export interface CursorPaginatedData<T> {
  data: T[];
  meta: CursorPageMeta;
}

export interface PaginationProps {
  meta: Pick<PageMeta, "currentPage" | "lastPage">;
  /** Build the href for a page, e.g. `(page) => members.index({ query: { page } })`. */
  href: (page: number) => string;
  /** Numbered links kept on each side of the current page. Default 2. */
  siblings?: number;
  previousLabel?: ReactNode;
  nextLabel?: ReactNode;
  className?: string;
  /** Preserve scroll position when switching pages. Default true. */
  preserveScroll?: boolean;
}

/** Windowed page list: 1 … current±siblings … last. */
export function paginationWindow(
  current: number,
  last: number,
  siblings = 2,
): Array<number | "…"> {
  if (last <= 1) return [1];
  const pages = new Set<number>([1, last]);
  for (let page = current - siblings; page <= current + siblings; page += 1) {
    if (page >= 1 && page <= last) pages.add(page);
  }
  const sorted = [...pages].sort((left, right) => left - right);
  const output: Array<number | "…"> = [];
  for (const [index, page] of sorted.entries()) {
    if (index > 0 && page - sorted[index - 1] > 1) output.push("…");
    output.push(page);
  }
  return output;
}

/**
 * Numbered pagination driven by `Paginated<T>` meta. Page URLs come from the
 * `href` builder so named-route query params stay in one place.
 */
export function Pagination({
  meta,
  href,
  siblings = 2,
  previousLabel = "‹",
  nextLabel = "›",
  className,
  preserveScroll = true,
}: PaginationProps): ReactElement | null {
  const { currentPage, lastPage } = meta;
  if (lastPage <= 1) return null;

  const item = (page: number, label: ReactNode, rel?: string) => {
    const props: LinkProps & Record<string, unknown> = {
      href: href(page),
      active: rel === undefined && page === currentPage,
      preserveScroll,
      "data-phoenix-page": page,
    };
    if (rel !== undefined) props.rel = rel;
    return createElement(Link, { key: rel ?? page, ...props }, label);
  };

  const children: ReactNode[] = [];
  if (currentPage > 1) children.push(item(currentPage - 1, previousLabel, "prev"));
  for (const entry of paginationWindow(currentPage, lastPage, siblings)) {
    children.push(entry === "…"
      ? createElement("span", { key: `gap-${children.length}`, "aria-hidden": true }, "…")
      : item(entry, entry));
  }
  if (currentPage < lastPage) children.push(item(currentPage + 1, nextLabel, "next"));

  return createElement(
    "nav",
    { "aria-label": "pagination", className, "data-phoenix-pagination": "" },
    createElement(Fragment, null, ...children),
  );
}
