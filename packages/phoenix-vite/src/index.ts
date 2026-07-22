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

const CLIENT_ID = "virtual:phoenix/client";
const SERVER_ID = "virtual:phoenix/server";
const RESOLVED_CLIENT_ID = `\0${CLIENT_ID}`;
const RESOLVED_SERVER_ID = `\0${SERVER_ID}`;
const COMPONENT_EXTENSIONS = new Set([".js", ".jsx", ".ts", ".tsx"]);

export interface PhoenixViteOptions {
  generatedRoutes?: string;
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
              entryFileNames: "phoenix.js",
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
      generatedRoutesFile = resolve(
        resolved.root,
        options.generatedRoutes ?? "views/generated/routes.ts",
      );
      generateRouteTypes(routesRoot, generatedRoutesFile);
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
        : serverModule(discovery);
    },

    transform(source, id) {
      if (!isWithin(id, join(viewsRoot, "pages")) || ![".jsx", ".tsx"].includes(extname(id))) {
        return null;
      }
      return transformClientDirectives(source, id);
    },

    configureServer(server) {
      watchForNewModules(server, viewsRoot);
      watchForRoutes(server, routesRoot, generatedRoutesFile);
    },

    handleHotUpdate({ file, server }) {
      if (isWithin(file, routesRoot) && extname(file) === ".rs") {
        generateRouteTypes(routesRoot, generatedRoutesFile);
        server.ws.send({ type: "full-reload" });
        return;
      }
      if (!isWithin(file, viewsRoot)) return;
      invalidateVirtualModules(server);
    },
  };
}

interface RustToken {
  kind: "identifier" | "punctuation" | "string";
  value: string;
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
  route?: string;
}

export function generateRouteTypes(routesRoot: string, outputFile: string): string[] {
  const names = discoverRouteNames(routesRoot);
  const source = routeTypesModule(names);
  mkdirSync(dirname(outputFile), { recursive: true });
  if (!existsSync(outputFile) || readFileSync(outputFile, "utf8") !== source) {
    writeFileSync(outputFile, source);
  }
  return names;
}

function discoverRouteNames(routesRoot: string): string[] {
  if (!existsSync(routesRoot)) return [];
  const names = walk(routesRoot)
    .filter((file) => extname(file) === ".rs")
    .flatMap((file) => routeNamesFromRust(readFileSync(file, "utf8"), file))
    .sort((left, right) => left.localeCompare(right));
  const seen = new Set<string>();
  for (const name of names) {
    if (seen.has(name)) {
      throw new Error(`Phoenix discovered duplicate Rust route name: ${name}`);
    }
    seen.add(name);
  }
  return names;
}

function routeNamesFromRust(source: string, file: string): string[] {
  const tokens = tokenizeRust(source);
  const pairs = delimiterPairs(tokens, file);
  const calls = nameCalls(tokens, file);
  const groups = routeGroups(tokens, pairs, calls);
  const prefixCalls = new Set(groups.map((group) => group.prefixCall));

  return calls
    .filter((call) => !prefixCalls.has(call.tokenIndex))
    .map((call) => {
      const prefix = groups
        .filter((group) => call.tokenIndex > group.bodyStart && call.tokenIndex < group.bodyEnd)
        .sort((left, right) => right.bodyEnd - right.bodyStart - (left.bodyEnd - left.bodyStart))
        .map((group) => group.prefix)
        .join("");
      return `${prefix}${call.name}`;
    });
}

function tokenizeRust(source: string): RustToken[] {
  const tokens: RustToken[] = [];
  let index = 0;
  while (index < source.length) {
    const character = source[index];
    if (/\s/.test(character)) {
      index += 1;
      continue;
    }
    if (source.startsWith("//", index)) {
      index = source.indexOf("\n", index + 2);
      if (index === -1) break;
      continue;
    }
    if (source.startsWith("/*", index)) {
      index = skipBlockComment(source, index);
      continue;
    }
    const rawString = readRawRustString(source, index);
    if (rawString) {
      tokens.push({ kind: "string", value: rawString.value });
      index = rawString.end;
      continue;
    }
    if (character === '"') {
      const string = readRustString(source, index);
      tokens.push({ kind: "string", value: string.value });
      index = string.end;
      continue;
    }
    if (/[A-Za-z_]/.test(character)) {
      let end = index + 1;
      while (end < source.length && /[A-Za-z0-9_]/.test(source[end])) end += 1;
      tokens.push({ kind: "identifier", value: source.slice(index, end) });
      index = end;
      continue;
    }
    tokens.push({ kind: "punctuation", value: character });
    index += 1;
  }
  return tokens;
}

function skipBlockComment(source: string, start: number): number {
  let depth = 1;
  let index = start + 2;
  while (index < source.length && depth > 0) {
    if (source.startsWith("/*", index)) {
      depth += 1;
      index += 2;
    } else if (source.startsWith("*/", index)) {
      depth -= 1;
      index += 2;
    } else {
      index += 1;
    }
  }
  return index;
}

function readRawRustString(source: string, start: number): { end: number; value: string } | null {
  if (source[start] !== "r") return null;
  let quote = start + 1;
  while (source[quote] === "#") quote += 1;
  if (source[quote] !== '"') return null;
  const hashes = source.slice(start + 1, quote);
  const terminator = `"${hashes}`;
  const end = source.indexOf(terminator, quote + 1);
  if (end === -1) throw new Error("Phoenix found an unterminated Rust raw string");
  return { end: end + terminator.length, value: source.slice(quote + 1, end) };
}

function readRustString(source: string, start: number): { end: number; value: string } {
  let index = start + 1;
  let value = "";
  while (index < source.length) {
    const character = source[index];
    if (character === '"') return { end: index + 1, value };
    if (character !== "\\") {
      value += character;
      index += 1;
      continue;
    }
    const escaped = source[index + 1];
    const replacements: Record<string, string> = {
      "\\": "\\",
      '"': '"',
      n: "\n",
      r: "\r",
      t: "\t",
    };
    value += replacements[escaped] ?? escaped;
    index += 2;
  }
  throw new Error("Phoenix found an unterminated Rust string");
}

function delimiterPairs(tokens: RustToken[], file: string): Map<number, number> {
  const pairs = new Map<number, number>();
  const stack: Array<{ index: number; value: string }> = [];
  const closing: Record<string, string> = { ")": "(", "]": "[", "}": "{" };
  for (const [index, token] of tokens.entries()) {
    if (["(", "[", "{"].includes(token.value)) {
      stack.push({ index, value: token.value });
    } else if (token.value in closing) {
      const opening = stack.pop();
      if (!opening || opening.value !== closing[token.value]) {
        throw new Error(`Phoenix could not parse Rust route delimiters in ${file}`);
      }
      pairs.set(opening.index, index);
    }
  }
  if (stack.length > 0) throw new Error(`Phoenix found an unclosed Rust delimiter in ${file}`);
  return pairs;
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

function routeTypesModule(names: string[]): string {
  const root: RouteTreeNode = { children: new Map() };
  for (const name of names) addRoute(root, name);
  const exports = [...root.children.keys()]
    .filter(isSafeBindingName)
    .map((name) => `export const ${name} = routes[${JSON.stringify(name)}];`);
  const routeNameType = names.length === 0
    ? "never"
    : names.map((name) => JSON.stringify(name)).join(" | ");
  return [
    "// This file is generated by @phoenix/vite from Rust named routes. Do not edit.",
    `export const routes = ${printRouteTree(root, 0)} as const;`,
    "",
    `export type PhoenixRouteName = ${routeNameType};`,
    ...(exports.length > 0 ? ["", ...exports] : []),
    "",
  ].join("\n");
}

function addRoute(root: RouteTreeNode, route: string): void {
  const segments = route.split(".");
  if (segments.some((segment) => segment.length === 0)) {
    throw new Error(`Phoenix route name cannot contain an empty segment: ${route}`);
  }
  let node = root;
  for (const [index, segment] of segments.entries()) {
    if (node.route) {
      throw new Error(`Phoenix route name cannot also be a TypeScript namespace: ${node.route}`);
    }
    let child = node.children.get(segment);
    if (!child) {
      child = { children: new Map() };
      node.children.set(segment, child);
    }
    node = child;
    if (index === segments.length - 1) {
      if (node.children.size > 0) {
        throw new Error(`Phoenix route name cannot also be a TypeScript namespace: ${route}`);
      }
      node.route = route;
    }
  }
}

function printRouteTree(node: RouteTreeNode, depth: number): string {
  if (node.route) return JSON.stringify(node.route);
  if (node.children.size === 0) return "{}";
  const indent = "  ".repeat(depth);
  const childIndent = "  ".repeat(depth + 1);
  const children = [...node.children.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, child]) => (
      `${childIndent}${JSON.stringify(name)}: ${printRouteTree(child, depth + 1)},`
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

function serverModule({ pages, islands }: Discovery): string {
  const imports: string[] = [
    'import { registerIsland } from "@phoenix/react";',
    'import { startRenderer } from "@phoenix/react-ssr";',
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
    `startRenderer({ pages: ${pageRegistry}, islands: ${islandRegistry} });`,
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
): void {
  server.watcher.add(routesRoot);
  const regenerate = (file: string) => {
    if (isWithin(file, routesRoot) && extname(file) === ".rs") {
      generateRouteTypes(routesRoot, generatedRoutesFile);
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
