import { createElement } from "react";
import { describe, expect, it } from "vitest";

import { Island, type PageEnvelope } from "@phoenix/react";
import { renderPage } from "./index.js";

const baseEnvelope: PageEnvelope = {
  protocol: 1,
  render_mode: "islands",
  page: "articles/show",
  props: { title: "A simple bridge" },
  shared: {},
  errors: {},
  flash: {},
  contract_hash: null,
  asset_version: null,
  request_id: null,
  routes: {},
  islands: [],
};

const pages = {
  "articles/show": ({ title }: { title: string }) => createElement("article", null, title),
};

describe("Phoenix React server renderer", () => {
  it.each(["ssr", "islands"] as const)("renders business HTML in %s mode", (mode) => {
    const result = renderPage({ ...baseEnvelope, render_mode: mode }, pages);

    expect(result.mode).toBe(mode);
    expect(result.html).toContain("A simple bridge");
  });

  it("leaves SPA rendering to the browser", () => {
    expect(renderPage({ ...baseEnvelope, render_mode: "spa" }, pages).html).toBe("");
  });

  it("collects JSON props and assigns stable ids to repeated islands", () => {
    function Counter({ count }: { count: number }) {
      return createElement("button", null, count);
    }
    function Counters() {
      return createElement("main", null,
        createElement(Island, null, createElement(Counter, { count: 1 })),
        createElement(Island, null, createElement(Counter, { count: 2 })),
      );
    }

    const result = renderPage(
      { ...baseEnvelope, page: "counters/index" },
      { "counters/index": Counters },
    );

    expect(result.islands).toEqual([
      { id: "counter", component: "counter", props: { count: 1 } },
      { id: "counter-2", component: "counter", props: { count: 2 } },
    ]);
    expect(result.html).toContain('data-phoenix-island="counter-2"');
  });

  it("rejects non-serializable island props during SSR", () => {
    function Action(_props: { onRun: () => void }) {
      return createElement("button", null, "Run");
    }
    function InvalidPage() {
      return createElement(Island, null, createElement(Action, { onRun: () => undefined }));
    }

    expect(() => renderPage(
      { ...baseEnvelope, page: "invalid/index" },
      { "invalid/index": InvalidPage },
    )).toThrow("props must be JSON-serializable");
  });
});
