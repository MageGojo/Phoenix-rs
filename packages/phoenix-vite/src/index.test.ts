import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { afterEach, describe, expect, it } from "vitest";

import { phoenix, phoenixVirtualModules } from "./index.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  temporaryDirectories.splice(0).forEach((directory) => rmSync(directory, {
    force: true,
    recursive: true,
  }));
});

describe("Phoenix Vite plugin", () => {
  it("generates lazy browser loaders from page and island conventions", () => {
    const root = fixture();
    const plugin = configuredPlugin(root);
    const resolved = invokeHook(plugin.resolveId, phoenixVirtualModules.client) as string;
    const source = invokeHook(plugin.load, resolved) as string;

    expect(source).toContain('"members/index":{load:()=>import(');
    expect(source).toContain('"member-creator":{load:()=>import(');
    expect(source).toContain("void startPhoenix({ pages, islands })");
    expect(source).toContain("styles.css");
  });

  it("generates an SSR renderer with eager page registration", () => {
    const root = fixture();
    const plugin = configuredPlugin(root);
    const resolved = invokeHook(plugin.resolveId, phoenixVirtualModules.server) as string;
    const source = invokeHook(plugin.load, resolved) as string;

    expect(source).toContain('registerIsland("member-creator", Island0)');
    expect(source).toContain('pages: {"members/index":Page0}');
    expect(source).toContain('islands: {"member-creator":Island0}');
    expect(source).toContain("startRenderer(");
  });

  it("compiles client:load into an Island boundary without changing the component", () => {
    const root = fixture();
    const plugin = configuredPlugin(root);
    const source = [
      'import MemberCreator from "../../islands/member-creator";',
      "export default function Page() {",
      "  return <MemberCreator client:load initialTotal={100} />;",
      "}",
    ].join("\n");
    const result = invokeHook(
      plugin.transform,
      source,
      join(root, "views/pages/members/index.tsx"),
    ) as { code: string };

    expect(result.code).toContain('import { Island as __PhoenixIslandBoundary }');
    expect(result.code).toContain("<__PhoenixIslandBoundary><MemberCreator initialTotal={100} /></__PhoenixIslandBoundary>");
    expect(result.code).not.toContain("client:load");
  });

  it("generates an autocomplete-safe TypeScript tree from Rust route names", () => {
    const root = fixture();
    configuredPlugin(root);
    const source = readFileSync(join(root, "views/generated/routes.ts"), "utf8");

    expect(source).toContain('"members": {');
    expect(source).toContain('"store": "members.store"');
    expect(source).toContain('"dashboard": "admin.dashboard"');
    expect(source).toContain("export const members = routes[\"members\"];");
    expect(source).toContain('export type PhoenixRouteName = "admin.dashboard" | "health" | "members.store";');
    expect(source).not.toContain('"dashboard": "dashboard"');
  });

  it("rejects dynamic Rust route names that cannot produce TypeScript hints", () => {
    const root = fixture();
    writeFileSync(join(root, "routes/web.rs"), [
      'let route_name = "members.store";',
      'Routes::new().post("/members", handler).name(route_name)',
    ].join("\n"));

    expect(() => configuredPlugin(root)).toThrow("route names must be string literals");
  });
});

function fixture(): string {
  const root = mkdtempSync(join(tmpdir(), "phoenix-vite-"));
  temporaryDirectories.push(root);
  mkdirSync(join(root, "views/pages/members"), { recursive: true });
  mkdirSync(join(root, "views/islands"), { recursive: true });
  mkdirSync(join(root, "routes"), { recursive: true });
  writeFileSync(join(root, "views/pages/members/index.tsx"), "export default () => null;");
  writeFileSync(join(root, "views/islands/member-creator.tsx"), "export default () => null;");
  writeFileSync(join(root, "views/styles.css"), "body {}");
  writeFileSync(join(root, "routes/web.rs"), [
    "Routes::new()",
    '  .get("/health", handler).name("health")',
    '  .post("/members", handler).name("members.store")',
    "  .group(",
    '    RouteGroup::new().prefix("/admin").name("admin."),',
    "    |routes| routes.get(\"/dashboard\", handler).name(\"dashboard\"),",
    "  )",
  ].join("\n"));
  return root;
}

function configuredPlugin(root: string) {
  const plugin = phoenix();
  invokeHook(plugin.configResolved, { root });
  return plugin;
}

function invokeHook(hook: unknown, ...args: unknown[]): unknown {
  const handler = typeof hook === "function"
    ? hook
    : (hook as { handler: (...values: unknown[]) => unknown }).handler;
  return Reflect.apply(handler, {}, args);
}
