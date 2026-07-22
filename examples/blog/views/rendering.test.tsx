import { describe, expect, it } from "vitest";
import { renderPage } from "@phoenix/react-ssr";
import type { PageEnvelope } from "@phoenix/react";

import ArticleShow from "./pages/articles/show.js";
import MembersIndex from "./pages/members/index.js";

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

  it("renders one page from a 100-record Rust-shaped payload", () => {
    const members = Array.from({ length: 100 }, (_, index) => ({
      id: index + 1,
      name: `成员${String(index + 1).padStart(3, "0")}`,
      email: `member${String(index + 1).padStart(3, "0")}@example.test`,
      city: "杭州",
      role: "后端工程师",
      status: "active" as const,
      projects: index % 12 + 1,
      joinedOn: "2024-01-01",
      lastActiveMinutes: index,
    }));

    const html = renderPage(
      {
        ...envelope,
        render_mode: "ssr",
        page: "members/index",
        props: { members, generatedBy: "Rust", total: 100 },
      },
      { "members/index": MembersIndex },
    ).html;

    expect(html).toContain("团队成员目录");
    expect(html.match(/@example\.test/g)).toHaveLength(10);
    expect(html).toContain("member001@example.test");
    expect(html).not.toContain("member011@example.test");
  });
});
