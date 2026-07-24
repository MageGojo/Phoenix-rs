import { createInterface } from "node:readline";
import { PassThrough } from "node:stream";

import { createElement, type ComponentType } from "react";
import { renderToPipeableStream, renderToString } from "react-dom/server";

import {
  PhoenixRenderProvider,
  registerIsland,
  type ComponentRegistry,
  type IslandDescriptor,
  type PageEnvelope,
} from "@apizero/react";

export interface RenderResult {
  html: string;
  islands: IslandDescriptor[];
  mode: PageEnvelope["render_mode"];
}

export interface RendererOptions {
  assetVersion?: string;
  contractHash?: string;
  pages: ComponentRegistry;
  islands?: ComponentRegistry;
}

interface RendererRequest {
  protocol: number;
  id: number;
  asset_version?: string;
  contract_hash?: string;
  csp_nonce?: string;
  kind: "hello" | "render" | "stream";
  envelope?: PageEnvelope;
}

export const RENDERER_PROTOCOL = 2;

export function renderPage(
  envelope: PageEnvelope,
  pages: ComponentRegistry,
): RenderResult {
  if (envelope.render_mode === "spa") {
    return { html: "", islands: [], mode: "spa" };
  }
  const { element, islands } = renderElement(envelope, pages);
  return {
    html: renderToString(element),
    islands,
    mode: envelope.render_mode,
  };
}

export async function streamPage(
  envelope: PageEnvelope,
  pages: ComponentRegistry,
  chunk: (html: string) => void,
  cspNonce?: string,
): Promise<Omit<RenderResult, "html">> {
  validateNonce(cspNonce);
  if (envelope.render_mode === "spa") {
    return { islands: [], mode: "spa" };
  }
  const { element, islands } = renderElement(envelope, pages);
  await new Promise<void>((resolve, reject) => {
    const output = new PassThrough();
    output.setEncoding("utf8");
    output.on("data", (value: string) => chunk(value));
    output.on("end", resolve);
    output.on("error", reject);
    const rendered = renderToPipeableStream(element, {
      nonce: cspNonce,
      onError: reject,
      onShellError: reject,
      onShellReady: () => rendered.pipe(output),
    });
  });
  return { islands, mode: envelope.render_mode };
}

function renderElement(envelope: PageEnvelope, pages: ComponentRegistry) {
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
  return { element, islands };
}

export function startRenderer({
  assetVersion,
  contractHash,
  pages,
  islands = {},
}: RendererOptions): void {
  for (const [name, Component] of Object.entries(islands)) {
    registerIsland(name, Component);
  }

  createInterface({ input: process.stdin, crlfDelay: Infinity }).on("line", async (line) => {
    let request: RendererRequest | undefined;
    try {
      request = JSON.parse(line) as RendererRequest;
      if (request.protocol !== RENDERER_PROTOCOL) {
        throw new Error(`unsupported renderer protocol: ${request.protocol}`);
      }
      if (request.kind === "hello") {
        write({
          protocol: RENDERER_PROTOCOL,
          id: request.id,
          ok: true,
          contract_hash: contractHash,
          asset_version: assetVersion,
        });
        return;
      }
      if (!request.envelope || !["render", "stream"].includes(request.kind)) {
        throw new Error("invalid renderer request");
      }
      validateNonce(request.csp_nonce);
      if (contractHash && request.envelope.contract_hash
        && request.envelope.contract_hash !== contractHash) {
        throw new Error("Phoenix renderer contract hash mismatch");
      }

      if (request.kind === "stream") {
        const result = await streamPage(request.envelope, pages, (chunk) => write({
          protocol: RENDERER_PROTOCOL,
          id: request!.id,
          ok: true,
          kind: "chunk",
          chunk,
        }), request.csp_nonce);
        write({
          protocol: RENDERER_PROTOCOL,
          id: request.id,
          ok: true,
          kind: "complete",
          islands: result.islands,
          head: [],
        });
        return;
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
        kind: request?.kind === "stream" ? "error" : undefined,
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

function validateNonce(nonce: unknown): asserts nonce is string | undefined {
  if (nonce === undefined) return;
  if (typeof nonce !== "string"
    || nonce.length < 16
    || nonce.length > 128
    || !/^[A-Za-z0-9+/_=-]+$/.test(nonce)) {
    throw new Error("invalid CSP nonce");
  }
}
