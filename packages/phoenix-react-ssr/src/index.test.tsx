import { createElement } from "react";
import { describe, expect, it } from "vitest";

import type { PageEnvelope } from "@phoenix/react";
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
});
