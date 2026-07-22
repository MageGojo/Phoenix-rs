import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { dirname, extname, join, relative, resolve, sep } from "node:path";

import type { Plugin, ResolvedConfig, ViteDevServer } from "vite";
import ts from "typescript";

import {
  generateContractTypes,
  type GeneratedActionContract,
  type RustActionContract,
} from "./contracts.js";
import { delimiterPairs, tokenizeRust, type RustToken } from "./rust.js";

const CLIENT_ID = "virtual:phoenix/client";
const SERVER_ID = "virtual:phoenix/server";
const RESOLVED_CLIENT_ID = `\0${CLIENT_ID}`;
const RESOLVED_SERVER_ID = `\0${SERVER_ID}`;
const COMPONENT_EXTENSIONS = new Set([".js", ".jsx", ".ts", ".tsx"]);

export interface PhoenixViteOptions {
  clientManifest?: string;
  contracts?: string;
  generatedContracts?: string;
  generatedRoutes?: string;
  manifest?: string;
  publicPath?: string;
  renderer?: boolean;
  routes?: string;
  views?: string;
}

interface DiscoveredModule {
  file: string;
  name: string;
}

interface Discovery {
  islands: DiscoveredModule[];
  pages: DiscoveredModule[];
  styles?: string;
}

export function phoenix(options: PhoenixViteOptions = {}): Plugin {
  let contractsRoot = "";
  let clientManifestFile = "";
  let generatedContractsFile = "";
  let generatedRoutesFile = "";
  let routesRoot = "";
  let viewsRoot = "";

  return {
    name: "phoenix",
    enforce: "pre",

    config() {
      if (options.renderer) {
        return {
          publicDir: false,
          build: {
            emptyOutDir: true,
            outDir: "public/ssr",
            ssr: true,
            ssrEmitAssets: true,
            rollupOptions: {
              input: SERVER_ID,
              output: { entryFileNames: "renderer.js" },
            },
          },
        };
      }
      return {
        publicDir: false,
        build: {
          emptyOutDir: true,
          outDir: "public/assets",
          rollupOptions: {
            input: CLIENT_ID,
            output: {
              entryFileNames: "phoenix-[hash].js",
              chunkFileNames: "chunks/[name]-[hash].js",
              assetFileNames: "[name][extname]",
            },
          },
        },
      };
    },

    configResolved(resolved) {
      viewsRoot = resolve(resolved.root, options.views ?? "views");
      routesRoot = resolve(resolved.root, options.routes ?? "routes");
      contractsRoot = resolve(resolved.root, options.contracts ?? ".");
      clientManifestFile = resolve(
        resolved.root,
        options.clientManifest ?? "public/assets/phoenix-manifest.json",
      );
      generatedContractsFile = resolve(
        resolved.root,
        options.generatedContracts ?? "views/generated/contracts.ts",
      );
      generatedRoutesFile = resolve(
        resolved.root,
        options.generatedRoutes ?? "views/generated/routes.ts",
      );
      generateRouteTypes(
        routesRoot,
        generatedRoutesFile,
        contractsRoot,
        generatedContractsFile,
      );
    },

    resolveId(id) {
      if (id === CLIENT_ID) return RESOLVED_CLIENT_ID;
      if (id === SERVER_ID) return RESOLVED_SERVER_ID;
      return null;
    },

    load(id) {
      if (id !== RESOLVED_CLIENT_ID && id !== RESOLVED_SERVER_ID) return null;
      const discovery = discover(viewsRoot);
      return id === RESOLVED_CLIENT_ID
        ? clientModule(discovery)
        : serverModule(
          discovery,
          generatedContractsFile,
          options.renderer ? clientManifestFile : undefined,
        );
    },

    transform(source, id) {
      if (!isWithin(id, join(viewsRoot, "pages")) || ![".jsx", ".tsx"].includes(extname(id))) {
        return null;
      }
      return transformClientDirectives(source, id);
    },

    configureServer(server) {
      watchForNewModules(server, viewsRoot);
      watchForRoutes(
        server,
        routesRoot,
        generatedRoutesFile,
        contractsRoot,
        generatedContractsFile,
      );
    },

    handleHotUpdate({ file, server }) {
      if (isWithin(file, contractsRoot) && extname(file) === ".rs") {
        generateRouteTypes(
          routesRoot,
          generatedRoutesFile,
          contractsRoot,
          generatedContractsFile,
        );
        server.ws.send({ type: "full-reload" });
        return;
      }
      if (!isWithin(file, viewsRoot)) return;
      invalidateVirtualModules(server);
    },

    generateBundle(_output, bundle) {
      const contractHash = readContractHash(generatedContractsFile);
      const version = bundleVersion(bundle);
      if (options.renderer) {
        this.emitFile({
          type: "asset",
          fileName: options.manifest ?? "phoenix-renderer.json",
          source: `${JSON.stringify({
            schema: 1,
            version,
            contract_hash: contractHash,
            entry: Object.values(bundle).find((item) => item.type === "chunk" && item.isEntry)?.fileName,
          }, null, 2)}\n`,
        });
        return;
      }

      const entry = Object.values(bundle).find(
        (item) => item.type === "chunk" && item.isEntry,
      );
      if (!entry || entry.type !== "chunk") {
        throw new Error("Phoenix client build did not produce an entry chunk");
      }
      const css = Object.values(bundle)
        .filter((item) => item.type === "asset" && item.fileName.endsWith(".css"))
        .map((item) => item.fileName)
        .sort();
      const imports = [...new Set(entry.imports)].sort();
      this.emitFile({
        type: "asset",
        fileName: options.manifest ?? "phoenix-manifest.json",
        source: `${JSON.stringify({
          schema: 1,
          version,
          contract_hash: contractHash,
          public_path: options.publicPath ?? "/assets/",
          entries: {
            client: { file: entry.fileName, css, imports },
          },
        }, null, 2)}\n`,
      });
    },
  };
}

function readContractHash(generatedContractsFile: string): string {
  const source = readFileSync(generatedContractsFile, "utf8");
  const hash = source.match(/export const contractHash = ["']([^"']+)["']/)?.[1];
  if (!hash) {
    throw new Error(`Phoenix generated contracts do not contain contractHash: ${generatedContractsFile}`);
  }
  return hash;
}

function readClientBuildIdentity(
  clientManifestFile: string,
  expectedContractHash: string,
): { version: string } {
  if (!existsSync(clientManifestFile)) {
    throw new Error(
      `Phoenix SSR build requires the client manifest; run the client build first: ${clientManifestFile}`,
    );
  }
  const manifest = JSON.parse(readFileSync(clientManifestFile, "utf8")) as {
    contract_hash?: string;
    schema?: number;
    version?: string;
  };
  if (manifest.schema !== 1 || !manifest.version || !manifest.contract_hash) {
    throw new Error(`Phoenix client manifest is invalid: ${clientManifestFile}`);
  }
  if (manifest.contract_hash !== expectedContractHash) {
    throw new Error(
      `Phoenix client/renderer contract hash mismatch: ${manifest.contract_hash} != ${expectedContractHash}`,
    );
  }
  return { version: manifest.version };
}

function bundleVersion(bundle: Record<string, { fileName: string; type: string; code?: string; source?: string | Uint8Array }>): string {
  const hash = createHash("sha256");
  for (const item of Object.values(bundle).sort((left, right) => left.fileName.localeCompare(right.fileName))) {
    hash.update(item.fileName);
    if (item.type === "chunk") hash.update(item.code ?? "");
    else hash.update(item.source ?? "");
  }
  return `sha256-${hash.digest("hex").slice(0, 24)}`;
}

interface RustNameCall {
  name: string;
  tokenIndex: number;
}

interface RustGroup {
  bodyEnd: number;
  bodyStart: number;
  prefix: string;
  prefixCall: number;
}

interface RouteTreeNode {
  children: Map<string, RouteTreeNode>;
  route?: RustRouteDeclaration;
}

interface RustRouteDeclaration {
  action?: RustActionContract;
  name: string;
}

export function generateRouteTypes(
  routesRoot: string,
  outputFile: string,
  contractsRoot?: string,
  contractsFile?: string,
): string[] {
  const routes = discoverRoutes(routesRoot);
  const actionDeclarations = routes.flatMap((route) => route.action ? [route.action] : []);
  let actions = new Map<string, GeneratedActionContract>();
  let typeImports: string[] = [];
  if (contractsRoot && contractsFile) {
    const generated = generateContractTypes(contractsRoot, contractsFile, actionDeclarations);
    actions = generated.actions;
    typeImports = generated.typeImports;
  } else if (actionDeclarations.length > 0) {
    throw new Error("Phoenix action contracts require a contract source root and output file");
  }
  const source = routeTypesModule(routes, actions, typeImports);
  mkdirSync(dirname(outputFile), { recursive: true });
  if (!existsSync(outputFile) || readFileSync(outputFile, "utf8") !== source) {
    writeFileSync(outputFile, source);
  }
  return routes.map((route) => route.name);
}

function discoverRoutes(routesRoot: string): RustRouteDeclaration[] {
  if (!existsSync(routesRoot)) return [];
  const routes = walk(routesRoot)
    .filter((file) => extname(file) === ".rs")
    .flatMap((file) => routesFromRust(readFileSync(file, "utf8"), file))
    .sort((left, right) => left.name.localeCompare(right.name));
  const seen = new Set<string>();
  for (const route of routes) {
    if (seen.has(route.name)) {
      throw new Error(`Phoenix discovered duplicate Rust route name: ${route.name}`);
    }
    seen.add(route.name);
  }
  return routes;
}

function routesFromRust(source: string, file: string): RustRouteDeclaration[] {
  const tokens = tokenizeRust(source);
  const pairs = delimiterPairs(tokens, file);
  const calls = nameCalls(tokens, file);
  const groups = routeGroups(tokens, pairs, calls);
  const prefixCalls = new Set(groups.map((group) => group.prefixCall));
  const actions = actionCalls(tokens, file);

  return calls
    .filter((call) => !prefixCalls.has(call.tokenIndex))
    .map((call) => {
      const prefix = groups
        .filter((group) => call.tokenIndex > group.bodyStart && call.tokenIndex < group.bodyEnd)
        .sort((left, right) => right.bodyEnd - right.bodyStart - (left.bodyEnd - left.bodyStart))
        .map((group) => group.prefix)
        .join("");
      const name = `${prefix}${call.name}`;
      const action = actions.find((candidate) => candidate.nameTokenIndex === call.tokenIndex);
      return {
        name,
        action: action ? { input: action.input, output: action.output, route: name } : undefined,
      };
    });
}

function actionCalls(
  tokens: RustToken[],
  file: string,
): Array<{ input: string; nameTokenIndex: number; output: string }> {
  const calls: Array<{ input: string; nameTokenIndex: number; output: string }> = [];
  const names = nameCalls(tokens, file);
  for (let index = 1; index < tokens.length - 7; index += 1) {
    if (tokens[index - 1].value !== "." || tokens[index].value !== "action"
      || tokens[index + 1].value !== ":" || tokens[index + 2].value !== ":"
      || tokens[index + 3].value !== "<") continue;
    let angleDepth = 1;
    let comma: number | undefined;
    let close: number | undefined;
    for (let cursor = index + 4; cursor < tokens.length; cursor += 1) {
      if (tokens[cursor].value === "<") angleDepth += 1;
      if (tokens[cursor].value === ">") {
        angleDepth -= 1;
        if (angleDepth === 0) {
          close = cursor;
          break;
        }
      }
      if (tokens[cursor].value === "," && angleDepth === 1 && comma === undefined) comma = cursor;
    }
    if (comma === undefined || close === undefined
      || tokens[close + 1]?.value !== "(" || tokens[close + 2]?.value !== ")") {
      throw new Error(`Phoenix action requires .action::<Input, Output>() (${file})`);
    }
    const name = [...names].reverse().find((candidate) => candidate.tokenIndex < index);
    if (!name) throw new Error(`Phoenix action must follow a literal .name(\"...\") (${file})`);
    calls.push({
      input: rustTypeSource(tokens.slice(index + 4, comma)),
      nameTokenIndex: name.tokenIndex,
      output: rustTypeSource(tokens.slice(comma + 1, close)),
    });
    index = close + 2;
  }
  return calls;
}

function rustTypeSource(tokens: RustToken[]): string {
  return tokens.map((token) => token.kind === "string" ? JSON.stringify(token.value) : token.value).join("");
}

function nameCalls(tokens: RustToken[], file: string): RustNameCall[] {
  const calls: RustNameCall[] = [];
  for (let index = 1; index < tokens.length - 2; index += 1) {
    if (tokens[index - 1].value !== "." || tokens[index].value !== "name"
      || tokens[index + 1].value !== "(") continue;
    if (tokens[index + 2].kind !== "string" || tokens[index + 3]?.value !== ")") {
      throw new Error(
        `Phoenix route names must be string literals so TypeScript routes can be generated (${file})`,
      );
    }
    calls.push({ name: tokens[index + 2].value, tokenIndex: index });
  }
  return calls;
}

function routeGroups(
  tokens: RustToken[],
  pairs: Map<number, number>,
  calls: RustNameCall[],
): RustGroup[] {
  const groups: RustGroup[] = [];
  for (let index = 1; index < tokens.length - 1; index += 1) {
    if (tokens[index - 1].value !== "." || tokens[index].value !== "group"
      || tokens[index + 1].value !== "(") continue;
    const callEnd = pairs.get(index + 1);
    if (callEnd === undefined) continue;
    const comma = firstTopLevelComma(tokens, pairs, index + 2, callEnd);
    if (comma === undefined) continue;
    const firstArgumentCalls = calls.filter(
      (call) => call.tokenIndex > index + 1 && call.tokenIndex < comma,
    );
    const prefixCall = firstArgumentCalls.at(-1);
    if (!prefixCall) continue;
    const closureBlock = findToken(tokens, "{", comma + 1, callEnd);
    const bodyStart = closureBlock ?? comma;
    const bodyEnd = closureBlock === undefined ? callEnd : pairs.get(closureBlock);
    if (bodyEnd === undefined) continue;
    groups.push({
      bodyEnd,
      bodyStart,
      prefix: prefixCall.name,
      prefixCall: prefixCall.tokenIndex,
    });
  }
  return groups;
}

function firstTopLevelComma(
  tokens: RustToken[],
  pairs: Map<number, number>,
  start: number,
  end: number,
): number | undefined {
  let index = start;
  while (index < end) {
    if (tokens[index].value === ",") return index;
    const pair = pairs.get(index);
    index = pair === undefined ? index + 1 : pair + 1;
  }
  return undefined;
}

function findToken(
  tokens: RustToken[],
  value: string,
  start: number,
  end: number,
): number | undefined {
  for (let index = start; index < end; index += 1) {
    if (tokens[index].value === value) return index;
  }
  return undefined;
}

function routeTypesModule(
  routes: RustRouteDeclaration[],
  actions: Map<string, GeneratedActionContract>,
  typeImports: string[],
): string {
  const root: RouteTreeNode = { children: new Map() };
  for (const route of routes) addRoute(root, route);
  const exports = [...root.children.keys()]
    .filter(isSafeBindingName)
    .map((name) => `export const ${name} = routes[${JSON.stringify(name)}];`);
  const routeNameType = routes.length === 0
    ? "never"
    : routes.map((route) => JSON.stringify(route.name)).join(" | ");
  const imports = actions.size === 0 ? [] : [
    'import { createRustAction } from "@phoenix/react";',
    ...(typeImports.length > 0
      ? [`import type { ${typeImports.join(", ")} } from "./contracts.js";`]
      : []),
    "",
  ];
  return [
    "// This file is generated by @phoenix/vite from Rust named routes. Do not edit.",
    ...imports,
    `export const routes = ${printRouteTree(root, 0, actions)} as const;`,
    "",
    `export type PhoenixRouteName = ${routeNameType};`,
    ...(exports.length > 0 ? ["", ...exports] : []),
    "",
  ].join("\n");
}

function addRoute(root: RouteTreeNode, route: RustRouteDeclaration): void {
  const segments = route.name.split(".");
  if (segments.some((segment) => segment.length === 0)) {
    throw new Error(`Phoenix route name cannot contain an empty segment: ${route.name}`);
  }
  let node = root;
  for (const [index, segment] of segments.entries()) {
    if (node.route) {
      throw new Error(`Phoenix route name cannot also be a TypeScript namespace: ${node.route.name}`);
    }
    let child = node.children.get(segment);
    if (!child) {
      child = { children: new Map() };
      node.children.set(segment, child);
    }
    node = child;
    if (index === segments.length - 1) {
      if (node.children.size > 0) {
        throw new Error(`Phoenix route name cannot also be a TypeScript namespace: ${route.name}`);
      }
      node.route = route;
    }
  }
}

function printRouteTree(
  node: RouteTreeNode,
  depth: number,
  actions: Map<string, GeneratedActionContract>,
): string {
  if (node.route) {
    const action = actions.get(node.route.name);
    return action
      ? `createRustAction<${action.input}, ${action.output}>(${JSON.stringify(node.route.name)})`
      : JSON.stringify(node.route.name);
  }
  if (node.children.size === 0) return "{}";
  const indent = "  ".repeat(depth);
  const childIndent = "  ".repeat(depth + 1);
  const children = [...node.children.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, child]) => (
      `${childIndent}${JSON.stringify(name)}: ${printRouteTree(child, depth + 1, actions)},`
    ));
  return ["{", ...children, `${indent}}`].join("\n");
}

const RESERVED_BINDINGS = new Set([
  "await", "break", "case", "catch", "class", "const", "continue", "debugger",
  "default", "delete", "do", "else", "enum", "export", "extends", "false",
  "finally", "for", "function", "if", "implements", "import", "in", "instanceof",
  "interface", "let", "new", "null", "package", "private", "protected", "public",
  "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof",
  "var", "void", "while", "with", "yield",
]);

function isSafeBindingName(name: string): boolean {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) && !RESERVED_BINDINGS.has(name);
}

function transformClientDirectives(source: string, id: string): { code: string; map: null } | null {
  const scriptKind = extname(id) === ".tsx" ? ts.ScriptKind.TSX : ts.ScriptKind.JSX;
  const file = ts.createSourceFile(id, source, ts.ScriptTarget.Latest, true, scriptKind);
  const boundaries: Array<{ start: number; end: number; attributeStart: number; attributeEnd: number }> = [];

  const visit = (node: ts.Node): void => {
    if (ts.isJsxSelfClosingElement(node) || ts.isJsxOpeningElement(node)) {
      const directives = node.attributes.properties.filter((attribute) => (
        ts.isJsxAttribute(attribute) && ts.isJsxNamespacedName(attribute.name)
        && attribute.name.namespace.text === "client"
      )) as ts.JsxAttribute[];
      for (const directive of directives) {
        const directiveName = directive.name as ts.JsxNamespacedName;
        if (directiveName.name.text !== "load") {
          throw new Error(`Phoenix does not support client:${directiveName.name.text} in ${id}`);
        }
        if (directive.initializer) {
          throw new Error(`Phoenix client:load does not accept a value in ${id}`);
        }
        if (ts.isIdentifier(node.tagName) && /^[a-z]/.test(node.tagName.text)) {
          throw new Error(`Phoenix client:load requires a React component in ${id}`);
        }
        const boundary = ts.isJsxOpeningElement(node) && ts.isJsxElement(node.parent)
          ? node.parent
          : node;
        boundaries.push({
          start: boundary.getStart(file),
          end: boundary.getEnd(),
          attributeStart: directive.getFullStart(),
          attributeEnd: directive.getEnd(),
        });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  if (boundaries.length === 0) return null;

  const ordered = [...boundaries].sort((left, right) => left.start - right.start);
  for (let index = 1; index < ordered.length; index += 1) {
    if (ordered[index].start < ordered[index - 1].end) {
      throw new Error(`Phoenix client:load boundaries cannot be nested in ${id}`);
    }
  }

  const identifier = uniqueBoundaryIdentifier(source);
  const edits = boundaries.flatMap((boundary) => [
    { start: boundary.attributeStart, end: boundary.attributeEnd, text: "" },
    { start: boundary.start, end: boundary.start, text: `<${identifier}>` },
    { start: boundary.end, end: boundary.end, text: `</${identifier}>` },
  ]).sort((left, right) => right.start - left.start || right.end - left.end);
  let code = source;
  for (const edit of edits) {
    code = `${code.slice(0, edit.start)}${edit.text}${code.slice(edit.end)}`;
  }
  code = `import { Island as ${identifier} } from "@phoenix/react";\n${code}`;
  return { code, map: null };
}

function uniqueBoundaryIdentifier(source: string): string {
  const base = "__PhoenixIslandBoundary";
  let identifier = base;
  let suffix = 1;
  while (source.includes(identifier)) {
    suffix += 1;
    identifier = `${base}${suffix}`;
  }
  return identifier;
}

function discover(viewsRoot: string): Discovery {
  const pages = discoverDirectory(join(viewsRoot, "pages"), true);
  const islands = discoverDirectory(join(viewsRoot, "islands"), false);
  return {
    pages,
    islands,
    styles: existsSync(join(viewsRoot, "styles.css"))
      ? join(viewsRoot, "styles.css")
      : undefined,
  };
}

function discoverDirectory(directory: string, nestedName: boolean): DiscoveredModule[] {
  if (!existsSync(directory)) return [];
  const modules = walk(directory)
    .filter((file) => COMPONENT_EXTENSIONS.has(extname(file)))
    .map((file) => ({
      file,
      name: nestedName
        ? withoutExtension(relative(directory, file)).split(sep).join("/")
        : kebabCase(withoutExtension(relative(directory, file)).split(sep).join("-")),
    }))
    .sort((left, right) => left.name.localeCompare(right.name));

  const seen = new Set<string>();
  for (const module of modules) {
    if (seen.has(module.name)) {
      throw new Error(`Phoenix discovered duplicate component name: ${module.name}`);
    }
    seen.add(module.name);
  }
  return modules;
}

function walk(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
}

function clientModule({ pages, islands, styles }: Discovery): string {
  const lines = [
    'import { startPhoenix } from "@phoenix/react";',
  ];
  if (styles) lines.push(`import ${JSON.stringify(toImportPath(styles))};`);
  lines.push(
    `const pages = ${loaderRegistry(pages)};`,
    `const islands = ${loaderRegistry(islands)};`,
    "void startPhoenix({ pages, islands });",
  );
  return lines.join("\n");
}

function serverModule(
  { pages, islands }: Discovery,
  generatedContractsFile: string,
  clientManifestFile?: string,
): string {
  const contractHash = readContractHash(generatedContractsFile);
  const clientBuild = clientManifestFile
    ? readClientBuildIdentity(clientManifestFile, contractHash)
    : undefined;
  const imports: string[] = [
    'import { registerIsland } from "@phoenix/react";',
    'import { startRenderer } from "@phoenix/react-ssr";',
    `import { contractHash } from ${JSON.stringify(toImportPath(generatedContractsFile))};`,
  ];
  pages.forEach((module, index) => {
    imports.push(`import Page${index} from ${JSON.stringify(toImportPath(module.file))};`);
  });
  islands.forEach((module, index) => {
    imports.push(`import Island${index} from ${JSON.stringify(toImportPath(module.file))};`);
  });
  const pageRegistry = objectRegistry(pages, "Page");
  const islandRegistry = objectRegistry(islands, "Island");
  const registrations = islands.map(
    (module, index) => `registerIsland(${JSON.stringify(module.name)}, Island${index});`,
  );
  return [
    ...imports,
    ...registrations,
    `startRenderer({ ${clientBuild ? `assetVersion: ${JSON.stringify(clientBuild.version)}, ` : ""}contractHash, pages: ${pageRegistry}, islands: ${islandRegistry} });`,
  ].join("\n");
}

function loaderRegistry(modules: DiscoveredModule[]): string {
  return `{${modules.map((module) => (
    `${JSON.stringify(module.name)}:{load:()=>import(${JSON.stringify(toImportPath(module.file))})}`
  )).join(",")}}`;
}

function objectRegistry(modules: DiscoveredModule[], prefix: string): string {
  return `{${modules.map((module, index) => (
    `${JSON.stringify(module.name)}:${prefix}${index}`
  )).join(",")}}`;
}

function watchForNewModules(server: ViteDevServer, viewsRoot: string): void {
  const restart = (file: string) => {
    if (isWithin(file, viewsRoot) && COMPONENT_EXTENSIONS.has(extname(file))) {
      void server.restart();
    }
  };
  server.watcher.on("add", restart);
  server.watcher.on("unlink", restart);
}

function watchForRoutes(
  server: ViteDevServer,
  routesRoot: string,
  generatedRoutesFile: string,
  contractsRoot: string,
  generatedContractsFile: string,
): void {
  server.watcher.add([routesRoot, contractsRoot]);
  const regenerate = (file: string) => {
    if (isWithin(file, contractsRoot) && extname(file) === ".rs") {
      generateRouteTypes(
        routesRoot,
        generatedRoutesFile,
        contractsRoot,
        generatedContractsFile,
      );
    }
  };
  server.watcher.on("add", regenerate);
  server.watcher.on("unlink", regenerate);
}

function invalidateVirtualModules(server: ViteDevServer): void {
  for (const id of [RESOLVED_CLIENT_ID, RESOLVED_SERVER_ID]) {
    const module = server.moduleGraph.getModuleById(id);
    if (module) server.moduleGraph.invalidateModule(module);
  }
}

function isWithin(file: string, directory: string): boolean {
  const path = relative(directory, file);
  return path !== "" && !path.startsWith("..") && !path.startsWith(sep);
}

function toImportPath(file: string): string {
  return file.split(sep).join("/");
}

function withoutExtension(file: string): string {
  return file.slice(0, -extname(file).length);
}

function kebabCase(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/[^a-zA-Z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .toLowerCase();
}

export const phoenixVirtualModules = {
  client: CLIENT_ID,
  server: SERVER_ID,
} as const;
