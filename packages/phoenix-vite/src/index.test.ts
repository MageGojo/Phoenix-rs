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
  it("hashes every production asset so immutable caching is safe", () => {
    const plugin = phoenix();
    const config = invokeHook(plugin.config) as {
      build: { rollupOptions: { output: { assetFileNames: string } } };
      html?: { cspNonce?: string };
    };

    expect(config.build.rollupOptions.output.assetFileNames).toBe("[name]-[hash][extname]");
    expect(config.html?.cspNonce).toBeUndefined();
  });

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
    expect(source).toContain("import { contractHash }");
    expect(source).toContain("startRenderer({ contractHash,");
    expect(source).toContain("startRenderer(");
  });

  it("pins the production renderer to the client asset and contract identity", () => {
    const root = fixture();
    const plugin = configuredPlugin(root, { renderer: true });
    const contracts = readFileSync(join(root, "views/generated/contracts.ts"), "utf8");
    const contractHash = contracts.match(/contractHash = "([^"]+)"/)?.[1];
    mkdirSync(join(root, "public/assets"), { recursive: true });
    writeFileSync(join(root, "public/assets/phoenix-manifest.json"), JSON.stringify({
      schema: 1,
      version: "sha256-client-build",
      contract_hash: contractHash,
    }));
    const resolved = invokeHook(plugin.resolveId, phoenixVirtualModules.server) as string;
    const source = invokeHook(plugin.load, resolved) as string;

    expect(source).toContain('assetVersion: "sha256-client-build"');
    expect(source).toContain("contractHash");
  });

  it("emits a versioned production manifest with contract identity", () => {
    const root = fixture();
    const plugin = configuredPlugin(root);
    const emitted: Array<{ fileName: string; source: string; type: string }> = [];
    invokeHookWithContext(plugin.generateBundle, {
      emitFile(value: { fileName: string; source: string; type: string }) {
        emitted.push(value);
        return value.fileName;
      },
    }, {}, {
      "phoenix-a1.js": {
        code: "entry",
        fileName: "phoenix-a1.js",
        imports: ["chunks/page-c3.js"],
        isEntry: true,
        type: "chunk",
      },
      "chunks/page-c3.js": {
        code: "page",
        fileName: "chunks/page-c3.js",
        imports: [],
        isEntry: false,
        type: "chunk",
      },
      "chunks/island-d4.js": {
        code: "island",
        fileName: "chunks/island-d4.js",
        imports: [],
        isEntry: false,
        type: "chunk",
      },
      "client-b2.css": {
        fileName: "client-b2.css",
        source: "body{}",
        type: "asset",
      },
    });

    const manifest = emitted.find((item) => item.fileName === "phoenix-manifest.json");
    expect(manifest).toBeDefined();
    const parsed = JSON.parse(manifest!.source) as {
      contract_hash: string;
      entries: { client: { css: string[]; file: string; imports: string[] } };
      schema: number;
      version: string;
    };
    expect(parsed.schema).toBe(1);
    expect(parsed.version).toMatch(/^sha256-[0-9a-f]{24}$/);
    expect(parsed.contract_hash).toMatch(/^fnv1a-[0-9a-f]{8}$/);
    expect(parsed.entries.client).toEqual({
      file: "phoenix-a1.js",
      css: ["client-b2.css"],
      imports: ["chunks/island-d4.js", "chunks/page-c3.js"],
    });
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

  it("generates callable actions and Serde-accurate Rust contracts", () => {
    const root = fixture();
    mkdirSync(join(root, "app"), { recursive: true });
    writeFileSync(join(root, "routes/web.rs"), [
      "Routes::new()",
      '  .post("/members", handler).name("members.store")',
      "  .action::<StoreMemberInput, MemberResource>()",
    ].join("\n"));
    writeFileSync(join(root, "app/contracts.rs"), [
      "#[phoenix::contract(input)]",
      "#[serde(rename_all = \"camelCase\")]",
      "pub struct StoreMemberInput {",
      "  pub display_name: String,",
      "  #[serde(default)]",
      "  pub note: Option<String>,",
      "}",
      "#[phoenix::contract(resource)]",
      "#[serde(rename_all = \"camelCase\")]",
      "pub struct MemberResource {",
      "  pub member_id: u32,",
      "  #[serde(rename = \"createdBy\")]",
      "  pub created_by: String,",
      "}",
      "#[phoenix::contract(page, page = \"members/index\")]",
      "pub struct MembersPageProps {",
      "  #[serde(flatten)]",
      "  pub member: MemberResource,",
      "  pub total: u32,",
      "}",
      "#[phoenix::contract(shared)]",
      "pub struct SharedProps {",
      "  pub locale: String,",
      "}",
    ].join("\n"));

    configuredPlugin(root);
    const routes = readFileSync(join(root, "views/generated/routes.ts"), "utf8");
    const contracts = readFileSync(join(root, "views/generated/contracts.ts"), "utf8");

    expect(routes).toContain('import { createRustAction } from "@phoenix/react";');
    expect(routes).toContain("createRustAction<StoreMemberInput, MemberResource>(\"members.store\")");
    expect(contracts).toContain('"displayName": string;');
    expect(contracts).toContain('"note"?: string | null;');
    expect(contracts).toContain('"memberId": number;');
    expect(contracts).toContain('"createdBy": string;');
    expect(contracts).toContain('"members/index": MembersPageProps;');
    expect(contracts).toContain("export type PhoenixSharedProps = SharedProps;");
    expect(contracts).toMatch(/export const contractHash = "fnv1a-[0-9a-f]{8}" as const;/);
  });

  it("rejects Serde field collisions after flattening", () => {
    const root = fixture();
    mkdirSync(join(root, "app"), { recursive: true });
    writeFileSync(join(root, "app/contracts.rs"), [
      "pub struct Identity { pub id: u32 }",
      "#[phoenix::contract(resource)]",
      "pub struct BrokenResource {",
      "  #[serde(flatten)] pub identity: Identity,",
      "  #[serde(rename = \"id\")] pub other_id: u32,",
      "}",
    ].join("\n"));

    expect(() => configuredPlugin(root)).toThrow("Serde wire name id collides");
  });

  it("applies directional Serde defaults, aliases, and skipped enum variants", () => {
    const root = fixture();
    mkdirSync(join(root, "app"), { recursive: true });
    writeFileSync(join(root, "app/contracts.rs"), [
      "#[phoenix::contract(input)]",
      "#[serde(default, rename_all(deserialize = \"camelCase\", serialize = \"snake_case\"))]",
      "pub struct SearchInput {",
      "  pub page_size: u32,",
      "}",
      "#[phoenix::contract(input)]",
      "#[serde(rename_all = \"snake_case\")]",
      "pub enum SortOrder {",
      "  #[serde(alias = \"newest\")] RecentFirst,",
      "  #[serde(skip_deserializing)] InternalOnly,",
      "}",
    ].join("\n"));

    configuredPlugin(root);
    const contracts = readFileSync(join(root, "views/generated/contracts.ts"), "utf8");

    expect(contracts).toContain('"pageSize"?: number;');
    expect(contracts).toContain('export type SortOrder = "recent_first" | "newest";');
    expect(contracts).not.toContain("internal_only");
  });

  it("fails closed for Serde wire transforms it cannot represent", () => {
    const root = fixture();
    mkdirSync(join(root, "app"), { recursive: true });
    writeFileSync(join(root, "app/contracts.rs"), [
      "#[phoenix::contract(resource)]",
      "pub struct BrokenResource {",
      "  #[serde(serialize_with = \"serialize_as_text\")]",
      "  pub amount: u32,",
      "}",
    ].join("\n"));

    expect(() => configuredPlugin(root)).toThrow(
      "does not support #[serde(serialize_with)]",
    );
  });

  it("rejects marked tuple and generic contracts instead of ignoring them", () => {
    const tupleRoot = fixture();
    mkdirSync(join(tupleRoot, "app"), { recursive: true });
    writeFileSync(join(tupleRoot, "app/contracts.rs"), [
      "#[phoenix::contract(resource)]",
      "pub struct TupleResource(pub String);",
    ].join("\n"));
    expect(() => configuredPlugin(tupleRoot)).toThrow("require named structs or enums");

    const genericRoot = fixture();
    mkdirSync(join(genericRoot, "app"), { recursive: true });
    writeFileSync(join(genericRoot, "app/contracts.rs"), [
      "#[phoenix::contract(resource)]",
      "pub struct GenericResource<T> { pub value: T }",
    ].join("\n"));
    expect(() => configuredPlugin(genericRoot)).toThrow("does not support generic contract");
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

function configuredPlugin(root: string, options: Parameters<typeof phoenix>[0] = {}) {
  const plugin = phoenix(options);
  invokeHook(plugin.configResolved, { root });
  return plugin;
}

function invokeHook(hook: unknown, ...args: unknown[]): unknown {
  const handler = typeof hook === "function"
    ? hook
    : (hook as { handler: (...values: unknown[]) => unknown }).handler;
  return Reflect.apply(handler, {}, args);
}

function invokeHookWithContext(
  hook: unknown,
  context: object,
  ...args: unknown[]
): unknown {
  const handler = typeof hook === "function"
    ? hook
    : (hook as { handler: (...values: unknown[]) => unknown }).handler;
  return Reflect.apply(handler, context, args);
}
