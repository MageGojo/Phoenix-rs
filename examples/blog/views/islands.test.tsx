// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { startPhoenix, type PageEnvelope } from "@apizero/react";
import { renderPage } from "@apizero/react-ssr";

import MemberCreator from "./islands/member-creator.js";
import MembersIndex from "./pages/members/index.js";
import type { Member } from "./generated/contracts.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const members: Member[] = [{
  id: 1,
  name: "林知遥",
  email: "member001@example.test",
  city: "上海",
  role: "后端工程师",
  status: "active",
  projects: 3,
  joinedOn: "2024-01-01",
  lastActiveMinutes: 2,
}];

describe("member creator island", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("hydrates the discovered form boundary and leaves the directory server-rendered", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      id: 101,
      name: "岛屿测试成员",
      email: "rust101@example.test",
      city: "Rust 服务端",
      role: "新成员",
      status: "active",
      projects: 0,
      joinedOn: "2026-07-22",
      lastActiveMinutes: 0,
      createdBy: "Rust",
    }), {
      status: 201,
      headers: { "content-type": "application/json" },
    }));
    vi.stubGlobal("fetch", fetchMock);

    const envelope: PageEnvelope = {
      protocol: 1,
      render_mode: "islands",
      page: "members/index",
      props: { members, generatedBy: "Rust", total: 1 },
      shared: {},
      errors: {},
      flash: {},
      contract_hash: null,
      asset_version: null,
      request_id: null,
      routes: { "members.store": "/api/members" },
      islands: [],
    };
    const rendered = renderPage(envelope, { "members/index": MembersIndex });
    const hydratedEnvelope = { ...envelope, islands: rendered.islands };
    document.body.innerHTML = [
      `<div id="phoenix-root">${rendered.html}</div>`,
      `<script id="phoenix-page" type="application/json">${JSON.stringify(hydratedEnvelope)}</script>`,
    ].join("");

    const loadMemberCreator = vi.fn(async () => ({ default: MemberCreator }));
    const loadUnused = vi.fn(async () => ({ default: MemberCreator }));
    await act(async () => {
      await startPhoenix({
        islands: {
          "member-creator": { load: loadMemberCreator },
          "unused-island": { load: loadUnused },
        },
      });
    });

    expect(rendered.islands).toEqual([{
      id: "member-creator",
      component: "member-creator",
      props: { initialTotal: 1 },
    }]);
    expect(loadMemberCreator).toHaveBeenCalledOnce();
    expect(loadUnused).not.toHaveBeenCalled();
    expect(document.querySelectorAll("[data-phoenix-island]")).toHaveLength(1);
    expect(document.querySelector("#member-table")?.textContent).toContain("member001@example.test");

    const input = document.querySelector<HTMLInputElement>("#new-member-name");
    const form = document.querySelector<HTMLFormElement>(".member-composer");
    expect(input).not.toBeNull();
    expect(form).not.toBeNull();

    await act(async () => {
      const valueSetter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )?.set;
      valueSetter?.call(input, "岛屿测试成员");
      input?.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });

    expect(fetchMock).toHaveBeenCalledWith("/api/members", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Accept": "application/json" },
      body: JSON.stringify({ name: "岛屿测试成员" }),
    });
    expect(document.body.textContent).toContain("Rust 已创建 岛屿测试成员");
    expect(document.body.textContent).toContain("rust101@example.test");
    expect(document.body.textContent).toContain("当前共 2 条记录");
  });
});
