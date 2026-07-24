import { describe, expect, expectTypeOf, it } from "vitest";
import { renderPage } from "@apizero/react-ssr";
import type { PageEnvelope } from "@apizero/react";

import { routes } from "./generated/routes.js";
import type { AuthMessageResource, AuthSessionResource, LoginInput, Member, PasswordResetInput, StoreMemberInput } from "./generated/contracts.js";
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
  it("generates every Rust route name as a TypeScript property tree", () => {
    expect(routes).toEqual({
      admin: { dashboard: "admin.dashboard" },
      health: "health",
      login: { store: expect.any(Function) },
      logout: { store: "logout.store" },
      members: { index: "members.index", store: expect.any(Function) },
      "password-reset": { store: expect.any(Function) },
      react: { islands: "react.islands", spa: "react.spa", ssr: "react.ssr" },
      register: { store: "register.store" },
      users: { show: "users.show" },
    });
    expect(routes.members.store.routeName).toBe("members.store");
    expect(routes.login.store.routeName).toBe("login.store");
    expect(routes["password-reset"].store.routeName).toBe("password-reset.store");
    expectTypeOf(routes.members.store).parameter(0).toEqualTypeOf<StoreMemberInput>();
    expectTypeOf(routes.members.store).returns.toEqualTypeOf<Promise<Member>>();
    expectTypeOf(routes.login.store).parameter(0).toEqualTypeOf<LoginInput>();
    expectTypeOf(routes.login.store).returns.toEqualTypeOf<Promise<AuthSessionResource>>();
    expectTypeOf(routes["password-reset"].store).parameter(0).toEqualTypeOf<PasswordResetInput>();
    expectTypeOf(routes["password-reset"].store).returns.toEqualTypeOf<Promise<AuthMessageResource>>();
  });

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
