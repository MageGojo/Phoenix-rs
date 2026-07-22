import { describe, expect, it } from "vitest";
import { renderPage } from "@phoenix/react-ssr";
import type { PageEnvelope } from "@phoenix/react";

import ArticleShow from "./pages/articles/show.js";

const envelope: PageEnvelope = {
  protocol: 1,
  render_mode: "islands",
  page: "articles/show",
  props: {
    title: "React meets Phoenix",
    summary: "One controller contract, three rendering modes.",
  },
  shared: {},
  errors: {},
  flash: {},
  contract_hash: null,
  asset_version: null,
  request_id: null,
  islands: [
    { id: "article-like", component: "like-button", props: { initialLikes: 7 } },
  ],
};

describe("blog React case", () => {
  it.each(["ssr", "islands"] as const)("renders the article in %s mode", (mode) => {
    const result = renderPage(
      { ...envelope, render_mode: mode },
      { "articles/show": ArticleShow },
    );

    expect(result.html).toContain("React meets Phoenix");
    expect(result.html).toContain('data-phoenix-island="article-like"');
  });

  it("keeps the SPA server shell empty", () => {
    const result = renderPage(
      { ...envelope, render_mode: "spa" },
      { "articles/show": ArticleShow },
    );
    expect(result.html).toBe("");
  });
});
