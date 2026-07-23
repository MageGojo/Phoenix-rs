import { describe, expect, it } from "vitest";

import { mergePageEnvelope, partialReloadHeaders } from "./partial.js";
import { pageEnvelope } from "./test-utils.js";

describe("mergePageEnvelope", () => {
  it("returns next when only/except are absent", () => {
    const current = pageEnvelope("posts/show", { title: "Old", comments: [] });
    const next = pageEnvelope("posts/show", { title: "New", comments: [1] });
    next.shared = { user: "ada" };
    expect(mergePageEnvelope(current, next, {})).toBe(next);
  });

  it("merges only listed props keys onto current props", () => {
    const current = pageEnvelope("posts/show", {
      title: "Keep",
      comments: [],
      sidebar: "old",
    });
    const next = pageEnvelope("posts/show", {
      title: "Ignore",
      comments: [{ id: 1 }],
      sidebar: "new",
    });
    next.shared = { user: "ada" };
    next.flash = { notice: "ok" };
    next.csrf_token = "csrf-next";
    next.page = "posts/show";

    const merged = mergePageEnvelope(current, next, { only: ["comments"] });

    expect(merged).toEqual({
      ...next,
      props: {
        title: "Keep",
        comments: [{ id: 1 }],
        sidebar: "old",
      },
    });
    expect(merged.shared).toEqual({ user: "ada" });
    expect(merged.flash).toEqual({ notice: "ok" });
    expect(merged.csrf_token).toBe("csrf-next");
  });

  it("merges all props except listed keys", () => {
    const current = pageEnvelope("posts/show", {
      title: "Keep-title-override-candidate",
      heavyChart: "keep-this",
      comments: [],
    });
    const next = pageEnvelope("posts/show", {
      title: "Updated",
      heavyChart: "should-not-apply",
      comments: [1, 2],
    });

    const merged = mergePageEnvelope(current, next, { except: ["heavyChart"] });

    expect(merged.props).toEqual({
      title: "Updated",
      heavyChart: "keep-this",
      comments: [1, 2],
    });
  });

  it("returns next when props are not both plain objects", () => {
    const current = pageEnvelope("posts/show", [{ id: 1 }]);
    const next = pageEnvelope("posts/show", [{ id: 2 }]);
    expect(mergePageEnvelope(current, next, { only: ["0"] })).toBe(next);
  });

  it("prefers only over except when both are provided", () => {
    const current = pageEnvelope("posts/show", { a: 1, b: 2, c: 3 });
    const next = pageEnvelope("posts/show", { a: 10, b: 20, c: 30 });
    const merged = mergePageEnvelope(current, next, {
      only: ["a"],
      except: ["b"],
    });
    expect(merged.props).toEqual({ a: 10, b: 2, c: 3 });
  });
});

describe("partialReloadHeaders", () => {
  it("emits X-Phoenix-Only and X-Phoenix-Except", () => {
    expect(partialReloadHeaders({ only: ["comments", "sidebar"] })).toEqual({
      "X-Phoenix-Only": "comments,sidebar",
    });
    expect(partialReloadHeaders({ except: ["heavyChart"] })).toEqual({
      "X-Phoenix-Except": "heavyChart",
    });
    expect(partialReloadHeaders({
      only: ["a"],
      except: ["b", "c"],
    })).toEqual({
      "X-Phoenix-Only": "a",
      "X-Phoenix-Except": "b,c",
    });
    expect(partialReloadHeaders({})).toEqual({});
    expect(partialReloadHeaders({ only: [] })).toEqual({});
  });
});
