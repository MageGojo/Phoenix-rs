// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";

import { installPage, pageEnvelope } from "./test-utils.js";
import {
  createRouteUrl,
  registerRouteManifest,
  resetRouteManifest,
  urlFor,
} from "./urls.js";

afterEach(() => {
  resetRouteManifest();
  document.body.innerHTML = "";
});

describe("urlFor", () => {
  it("substitutes path parameters with percent encoding", () => {
    registerRouteManifest({ "posts.show": "/posts/{slug}" });
    expect(urlFor("posts.show", { slug: "hello-world" })).toBe("/posts/hello-world");
    expect(urlFor("posts.show", { slug: "a b/c" })).toBe("/posts/a%20b%2Fc");
    expect(urlFor("posts.show", { slug: "it's*(ok)!" })).toBe("/posts/it%27s%2A%28ok%29%21");
    expect(urlFor("posts.show", { slug: "café" })).toBe("/posts/caf%C3%A9");
    expect(urlFor("posts.show", { slug: 42 })).toBe("/posts/42");
  });

  it("keeps RFC 3986 unreserved characters intact like the Rust encoder", () => {
    registerRouteManifest({ "posts.show": "/posts/{slug}" });
    expect(urlFor("posts.show", { slug: "a-b_c.d~e" })).toBe("/posts/a-b_c.d~e");
  });

  it("supports multiple parameters and static segments", () => {
    registerRouteManifest({ "orgs.repos.show": "/orgs/{org}/repos/{repo}" });
    expect(urlFor("orgs.repos.show", { org: "apizero", repo: "phoenix" }))
      .toBe("/orgs/apizero/repos/phoenix");
  });

  it("renders the root pattern as /", () => {
    registerRouteManifest({ home: "/" });
    expect(urlFor("home")).toBe("/");
  });

  it("appends query values and skips null/undefined", () => {
    registerRouteManifest({ "members.index": "/members" });
    expect(urlFor("members.index", {}, { query: { page: 2, q: "张三", skip: null } }))
      .toBe("/members?page=2&q=%E5%BC%A0%E4%B8%89");
    expect(urlFor("members.index", {}, { query: {} })).toBe("/members");
  });

  it("throws for a missing parameter, naming route and parameter", () => {
    registerRouteManifest({ "posts.show": "/posts/{slug}" });
    expect(() => urlFor("posts.show")).toThrowError(
      'Phoenix route "posts.show" is missing parameter "slug"',
    );
  });

  it("throws for an unknown route name", () => {
    registerRouteManifest({});
    expect(() => urlFor("nope")).toThrowError("Phoenix named route is not available: nope");
  });

  it("falls back to the document page envelope when nothing is registered", () => {
    const envelope = pageEnvelope("home", {});
    envelope.routes = { "users.show": "/users/{id}" };
    installPage(envelope);
    expect(urlFor("users.show", { id: 7 })).toBe("/users/7");
  });
});

describe("createRouteUrl", () => {
  it("builds parameterized URLs and exposes the route name", () => {
    registerRouteManifest({ "users.show": "/users/{id}" });
    const show = createRouteUrl<{ id: string | number }>("users.show");
    expect(show({ id: 9 })).toBe("/users/9");
    expect(show({ id: 9 }, { query: { tab: "posts" } })).toBe("/users/9?tab=posts");
    expect(show.routeName).toBe("users.show");
  });

  it("treats the first argument as options for parameterless routes", () => {
    registerRouteManifest({ "members.index": "/members" });
    const index = createRouteUrl("members.index");
    expect(index()).toBe("/members");
    expect(index({ query: { page: 3 } })).toBe("/members?page=3");
  });

  it("resolves against the manifest active at call time, not creation time", () => {
    const show = createRouteUrl<{ id: string | number }>("users.show");
    registerRouteManifest({ "users.show": "/users/{id}" });
    expect(show({ id: 1 })).toBe("/users/1");
    registerRouteManifest({ "users.show": "/people/{id}" });
    expect(show({ id: 1 })).toBe("/people/1");
  });
});
