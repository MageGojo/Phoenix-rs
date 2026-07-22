import { createElement, type ComponentType, type ReactNode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";

export type RenderMode = "spa" | "ssr" | "islands";

export interface IslandDescriptor {
  id: string;
  component: string;
  props: unknown;
}

export interface PageEnvelope<Props = unknown> {
  protocol: 1;
  render_mode: RenderMode;
  page: string;
  props: Props;
  shared: Record<string, unknown>;
  errors: Record<string, unknown>;
  flash: Record<string, unknown>;
  contract_hash: string | null;
  asset_version: string | null;
  request_id: string | null;
  routes: Record<string, string>;
  islands: IslandDescriptor[];
}

export interface EncryptedPayload {
  version: 1;
  algorithm: "A256GCM";
  key_id: string;
  purpose: "page-navigation";
  issued_at: number;
  expires_at: number;
  nonce: string;
  ciphertext: string;
  tag: string;
}

export type ComponentRegistry = Record<string, ComponentType<any>>;
export type ComponentList = readonly ComponentType<any>[];
export type DecryptPage = (payload: EncryptedPayload) => Promise<PageEnvelope>;

export interface PhoenixOptions {
  pages?: ComponentRegistry;
  islands?: ComponentRegistry | ComponentList;
  document?: Document;
}

export function island<Props extends object>(
  Component: ComponentType<Props>,
): ComponentType<Props & { islandId?: string }>;
export function island<Props extends object>(
  componentName: string,
  Component: ComponentType<Props>,
): ComponentType<Props & { islandId?: string }>;
export function island<Props extends object>(
  nameOrComponent: string | ComponentType<Props>,
  explicitComponent?: ComponentType<Props>,
): ComponentType<Props & { islandId?: string }> {
  const Component = typeof nameOrComponent === "string" ? explicitComponent : nameOrComponent;
  if (!Component) {
    throw new Error("Phoenix island component is required");
  }
  const name = typeof nameOrComponent === "string"
    ? nameOrComponent
    : componentName(Component);
  return function PhoenixIsland({ islandId = name, ...props }) {
    return createElement(
      "div",
      { "data-phoenix-island": islandId, "data-component": name },
      createElement(Component, props as Props),
    );
  };
}

export function startPhoenix(options: PhoenixOptions = {}): PageEnvelope {
  const documentRef = options.document ?? document;
  const envelope = readPage(documentRef);
  const root = requiredElement(documentRef, "phoenix-root");

  if (envelope.render_mode === "islands") {
    hydrateIslands(documentRef, envelope, componentRegistry(options.islands));
    return envelope;
  }

  const Page = requiredComponent(options.pages ?? {}, envelope.page, "page");
  const element = createElement(Page, pageProps(envelope));
  if (envelope.render_mode === "ssr") {
    hydrateRoot(root, element);
  } else {
    createRoot(root).render(element);
  }
  return envelope;
}

export class RustCallError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly details: unknown,
  ) {
    super(message);
    this.name = "RustCallError";
  }
}

export async function callRust<Output, Input = unknown>(
  routeName: string,
  input: Input,
  fetcher: typeof fetch = fetch,
): Promise<Output> {
  const url = rustRoute(routeName);
  const response = await fetcher(url, {
    method: "POST",
    headers: { "Content-Type": "application/json", "Accept": "application/json" },
    body: JSON.stringify(input),
  });
  const body = await response.json().catch(() => null) as unknown;
  if (!response.ok) {
    const message = isRecord(body) && typeof body.message === "string"
      ? body.message
      : `Rust action failed with ${response.status}`;
    throw new RustCallError(response.status, message, body);
  }
  return body as Output;
}

function rustRoute(routeName: string, documentRef: Document = document): string {
  const route = readPage(documentRef).routes[routeName];
  if (!route) {
    throw new Error(`Phoenix named route is not available: ${routeName}`);
  }
  return route;
}

export async function fetchPage(
  url: string,
  decrypt?: DecryptPage,
  fetcher: typeof fetch = fetch,
): Promise<PageEnvelope> {
  const response = await fetcher(url, {
    headers: { "X-Phoenix-Page": "1" },
  });
  if (!response.ok) {
    throw new Error(`Phoenix page request failed with ${response.status}`);
  }
  if (response.headers.get("x-phoenix-encrypted") === "1") {
    if (!decrypt) {
      throw new Error("Encrypted Phoenix page response requires a decrypt callback");
    }
    return decrypt((await response.json()) as EncryptedPayload);
  }
  return (await response.json()) as PageEnvelope;
}

export function createAes256GcmDecryptor(
  keys: Record<string, CryptoKey>,
): DecryptPage {
  return async (payload) => {
    if (
      payload.version !== 1 ||
      payload.algorithm !== "A256GCM" ||
      payload.purpose !== "page-navigation" ||
      payload.expires_at < Math.floor(Date.now() / 1000)
    ) {
      throw new Error("Unsupported Phoenix encrypted page envelope");
    }
    const key = keys[payload.key_id];
    if (!key) {
      throw new Error(`No key is available for ${payload.key_id}`);
    }
    const plaintext = await crypto.subtle.decrypt(
      {
        name: "AES-GCM",
        iv: decodeBase64Url(payload.nonce),
        additionalData: new TextEncoder().encode(
          `phoenix.page.v${payload.version}|${payload.key_id}|${payload.purpose}|${payload.issued_at}|${payload.expires_at}`,
        ),
      },
      key,
      concatBytes(decodeBase64Url(payload.ciphertext), decodeBase64Url(payload.tag)),
    );
    return JSON.parse(new TextDecoder().decode(plaintext)) as PageEnvelope;
  };
}

function readPage(documentRef: Document): PageEnvelope {
  const script = requiredElement(documentRef, "phoenix-page");
  return JSON.parse(script.textContent ?? "") as PageEnvelope;
}

function hydrateIslands(
  documentRef: Document,
  envelope: PageEnvelope,
  registry: ComponentRegistry,
): void {
  for (const descriptor of envelope.islands) {
    const root = Array.from(documentRef.querySelectorAll("[data-phoenix-island]"))
      .find((element) => element.getAttribute("data-phoenix-island") === descriptor.id);
    if (!root) {
      throw new Error(`Phoenix island root not found: ${descriptor.id}`);
    }
    const Component = requiredComponent(registry, descriptor.component, "island");
    hydrateRoot(root, createElement(Component, descriptor.props));
  }
}

function pageProps(envelope: PageEnvelope): Record<string, unknown> {
  return {
    ...(isRecord(envelope.props) ? envelope.props : { value: envelope.props }),
    phoenix: {
      shared: envelope.shared,
      errors: envelope.errors,
      flash: envelope.flash,
    },
  };
}

function requiredElement(documentRef: Document, id: string): HTMLElement {
  const element = documentRef.getElementById(id);
  if (!element) {
    throw new Error(`Phoenix element not found: #${id}`);
  }
  return element;
}

function requiredComponent(
  registry: ComponentRegistry,
  name: string,
  kind: string,
): ComponentType<any> {
  const component = registry[name];
  if (!component) {
    throw new Error(`Phoenix ${kind} is not registered: ${name}`);
  }
  return component;
}

function componentRegistry(
  components: ComponentRegistry | ComponentList | undefined,
): ComponentRegistry {
  if (!components) return {};
  if (!Array.isArray(components)) return components as ComponentRegistry;
  return Object.fromEntries(
    components.map((Component) => [componentName(Component), Component]),
  );
}

function componentName(Component: ComponentType<any>): string {
  const named = Component as ComponentType<any> & { displayName?: string; name?: string };
  const name = named.displayName || named.name;
  if (!name) {
    throw new Error("Phoenix island components must use a named function or explicit name");
  }
  return name
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
    .toLowerCase();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function decodeBase64Url(value: string): Uint8Array<ArrayBuffer> {
  const base64 = value.replaceAll("-", "+").replaceAll("_", "/");
  const padded = base64.padEnd(Math.ceil(base64.length / 4) * 4, "=");
  const bytes = Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
  return new Uint8Array(bytes.buffer);
}

function concatBytes(
  left: Uint8Array<ArrayBuffer>,
  right: Uint8Array<ArrayBuffer>,
): Uint8Array<ArrayBuffer> {
  const output = new Uint8Array(left.length + right.length);
  output.set(left);
  output.set(right, left.length);
  return output;
}

export type { ReactNode };
