import { createInterface } from "node:readline";

import { createElement, type ComponentType } from "react";
import { renderToString } from "react-dom/server";

import {
  PhoenixRenderProvider,
  registerIsland,
  type ComponentRegistry,
  type IslandDescriptor,
  type PageEnvelope,
} from "@phoenix/react";

export interface RenderResult {
  html: string;
  islands: IslandDescriptor[];
  mode: PageEnvelope["render_mode"];
}

export interface RendererOptions {
  pages: ComponentRegistry;
  islands?: ComponentRegistry;
}

interface RendererRequest {
  protocol: number;
  id: number;
  kind: "hello" | "render";
  envelope?: PageEnvelope;
}

export const RENDERER_PROTOCOL = 1;

export function renderPage(
  envelope: PageEnvelope,
  pages: ComponentRegistry,
): RenderResult {
  if (envelope.render_mode === "spa") {
    return { html: "", islands: [], mode: "spa" };
  }
  const Page = pages[envelope.page] as ComponentType<any> | undefined;
  if (!Page) {
    throw new Error(`Phoenix page is not registered: ${envelope.page}`);
  }

  const islands: IslandDescriptor[] = [];
  const ids = new Set<string>();
  const counts = new Map<string, number>();
  const collect = (component: string, rawProps: unknown, requestedId?: string): string => {
    const occurrence = (counts.get(component) ?? 0) + 1;
    counts.set(component, occurrence);
    const id = requestedId ?? (occurrence === 1 ? component : `${component}-${occurrence}`);
    if (ids.has(id)) {
      throw new Error(`Phoenix island id is duplicated: ${id}`);
    }
    ids.add(id);
    islands.push({ id, component, props: serializableProps(component, rawProps) });
    return id;
  };
  const props = isRecord(envelope.props)
    ? { ...envelope.props, phoenix: envelope.shared }
    : { value: envelope.props, phoenix: envelope.shared };
  const element = createElement(
    PhoenixRenderProvider,
    { mode: envelope.render_mode, collect },
    createElement(Page, props),
  );
  return {
    html: renderToString(element),
    islands,
    mode: envelope.render_mode,
  };
}

export function startRenderer({ pages, islands = {} }: RendererOptions): void {
  for (const [name, Component] of Object.entries(islands)) {
    registerIsland(name, Component);
  }

  createInterface({ input: process.stdin, crlfDelay: Infinity }).on("line", (line) => {
    let request: RendererRequest | undefined;
    try {
      request = JSON.parse(line) as RendererRequest;
      if (request.protocol !== RENDERER_PROTOCOL) {
        throw new Error(`unsupported renderer protocol: ${request.protocol}`);
      }
      if (request.kind === "hello") {
        write({ protocol: RENDERER_PROTOCOL, id: request.id, ok: true });
        return;
      }
      if (request.kind !== "render" || !request.envelope) {
        throw new Error("invalid renderer request");
      }

      const result = renderPage(request.envelope, pages);
      write({
        protocol: RENDERER_PROTOCOL,
        id: request.id,
        ok: true,
        html: result.html,
        islands: result.islands,
        head: [],
      });
    } catch (error) {
      write({
        protocol: RENDERER_PROTOCOL,
        id: request?.id ?? 0,
        ok: false,
        error: error instanceof Error ? error.message : "renderer failed",
      });
    }
  });
}

function serializableProps(component: string, props: unknown): unknown {
  try {
    const json = JSON.stringify(props, (_key, value: unknown) => {
      if (["bigint", "function", "symbol", "undefined"].includes(typeof value)) {
        throw new TypeError(`unsupported ${typeof value}`);
      }
      return value;
    });
    if (json === undefined) throw new TypeError("props are undefined");
    return JSON.parse(json) as unknown;
  } catch (error) {
    const reason = error instanceof Error ? error.message : "unknown serialization error";
    throw new Error(`Phoenix island ${component} props must be JSON-serializable: ${reason}`);
  }
}

function write(response: object): void {
  process.stdout.write(`${JSON.stringify(response)}\n`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
