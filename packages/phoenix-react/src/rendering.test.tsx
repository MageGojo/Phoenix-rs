// @vitest-environment jsdom
import { act, createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  Island,
  island,
  startPhoenix,
  stopPhoenix,
  type PageEnvelope,
} from "./index.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

afterEach(async () => {
  await act(async () => stopPhoenix());
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("Phoenix React rendering", () => {
  it("marks an island with its stable backend id", () => {
    const Counter = ({ count }: { count: number }) => createElement("button", null, count);
    const CounterIsland = island("counter", Counter);

    expect(renderToString(createElement(CounterIsland, { islandId: "counter-7", count: 7 })))
      .toContain('data-phoenix-island="counter-7"');
  });

  it("derives the island name from a named React component", () => {
    function MemberDirectory({ count }: { count: number }) {
      return createElement("button", null, count);
    }
    const MemberDirectoryIsland = island(MemberDirectory);

    expect(renderToString(createElement(MemberDirectoryIsland, { count: 7 })))
      .toContain('data-phoenix-island="member-directory"');
  });

  it("marks one unchanged React component with the declarative Island boundary", () => {
    function SaveButton({ label }: { label: string }) {
      return createElement("button", null, label);
    }

    expect(renderToString(
      createElement(Island, null, createElement(SaveButton, { label: "Save" })),
    )).toContain('data-phoenix-island="save-button"');
  });

  it("loads only the current SSR page before hydrating it", async () => {
    function ArticlePage({ title }: { title: string }) {
      return createElement("main", null, createElement("h1", null, title));
    }
    const envelope: PageEnvelope = {
      protocol: 1,
      render_mode: "ssr",
      page: "articles/show",
      props: { title: "Server article" },
      shared: {},
      errors: {},
      flash: {},
      contract_hash: null,
      asset_version: null,
      request_id: null,
      routes: {},
      islands: [],
    };
    document.body.innerHTML = [
      '<div id="phoenix-root"><main><h1>Server article</h1></main></div>',
      `<script id="phoenix-page" type="application/json">${JSON.stringify(envelope)}</script>`,
    ].join("");
    const loadArticle = vi.fn(async () => ({ default: ArticlePage }));
    const loadOther = vi.fn(async () => ({ default: ArticlePage }));

    await act(async () => {
      await startPhoenix({
        pages: {
          "articles/show": { load: loadArticle },
          "members/index": { load: loadOther },
        },
      });
    });

    expect(loadArticle).toHaveBeenCalledOnce();
    expect(loadOther).not.toHaveBeenCalled();
  });
});
