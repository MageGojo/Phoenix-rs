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
  routes: {},
  islands: [],
};

describe("blog React case", () => {
  it("discovers article islands while rendering server HTML", () => {
    const result = renderPage(envelope, { "articles/show": ArticleShow });

    expect(result.html).toContain("React meets Phoenix");
    expect(result.html).toContain('data-phoenix-island="like-button"');
    expect(result.islands).toEqual([{
      id: "like-button",
      component: "like-button",
      props: { initialLikes: 7 },
    }]);
  });

  it("renders SSR without island wrappers because the full page hydrates", () => {
    const result = renderPage(
      { ...envelope, render_mode: "ssr" },
      { "articles/show": ArticleShow },
    );

    expect(result.html).toContain("React meets Phoenix");
    expect(result.html).not.toContain("data-phoenix-island");
    expect(result.islands).toEqual([]);
  });

  it("keeps the SPA server shell empty", () => {
    const result = renderPage(
      { ...envelope, render_mode: "spa" },
      { "articles/show": ArticleShow },
    );
    expect(result.html).toBe("");
  });

  it("keeps the member table in SSR and isolates only the creator", () => {
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

    const result = renderPage(
      {
        ...envelope,
        page: "members/index",
        props: { members, generatedBy: "Rust", total: 100 },
      },
      { "members/index": MembersIndex },
    );

    expect(result.html).toContain("团队成员目录");
    expect(result.html).toContain('data-phoenix-island="member-creator"');
    expect(result.html).toContain("新增成员");
    expect(result.html.match(/@example\.test/g)).toHaveLength(10);
    expect(result.html).toContain("member001@example.test");
    expect(result.html).not.toContain("member011@example.test");
    expect(result.islands).toEqual([{
      id: "member-creator",
      component: "member-creator",
      props: { initialTotal: 100 },
    }]);
  });
});
