import { existsSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve, sep } from "node:path";

import type { Plugin, ResolvedConfig, ViteDevServer } from "vite";
import ts from "typescript";

const CLIENT_ID = "virtual:phoenix/client";
const SERVER_ID = "virtual:phoenix/server";
const RESOLVED_CLIENT_ID = `\0${CLIENT_ID}`;
const RESOLVED_SERVER_ID = `\0${SERVER_ID}`;
const COMPONENT_EXTENSIONS = new Set([".js", ".jsx", ".ts", ".tsx"]);

export interface PhoenixViteOptions {
  renderer?: boolean;
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
    },

    handleHotUpdate({ file, server }) {
      if (!isWithin(file, viewsRoot)) return;
      invalidateVirtualModules(server);
    },
  };
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
