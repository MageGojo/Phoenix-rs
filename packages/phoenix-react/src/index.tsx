import {
  Children,
  createContext,
  createElement,
  isValidElement,
  type ComponentType,
  type ReactElement,
  type ReactNode,
  useContext,
} from "react";
import { createRoot, hydrateRoot } from "react-dom/client";

declare module "react" {
  interface Attributes {
    "client:load"?: true;
  }
}

export type RenderMode = "spa" | "ssr" | "islands";

export interface IslandDescriptor {
  id: string;
  component: string;
  props: unknown;
}

export interface OpenGraph {
  title?: string | null;
  description?: string | null;
  image?: string | null;
  kind?: string | null;
}

export interface PageHead {
  title?: string | null;
  description?: string | null;
  canonical?: string | null;
  robots?: string | null;
  open_graph?: OpenGraph | null;
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
  head?: PageHead;
  csrf_token?: string | null;
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
export interface ComponentLoader {
  load: () => Promise<{ default: ComponentType<any> }>;
}
export type ComponentSource = ComponentRegistry | Record<string, ComponentType<any> | ComponentLoader>;
export type DecryptPage = (payload: EncryptedPayload) => Promise<PageEnvelope>;

export interface PhoenixOptions {
  pages?: ComponentSource;
  islands?: ComponentSource | ComponentList;
  document?: Document;
}

interface IslandRenderContext {
  mode: RenderMode;
  insideIsland: boolean;
  collect?: (component: string, props: unknown, requestedId?: string) => string;
}

const islandRenderContext = createContext<IslandRenderContext>({
  mode: "islands",
  insideIsland: false,
});
const registeredIslandNames = new WeakMap<object, string>();

export interface IslandProps {
  children?: ReactElement;
  id?: string;
}

export function Island({ children, id }: IslandProps): ReactElement {
  const context = useContext(islandRenderContext);
  const child = Children.only(children);
  if (!isValidElement(child) || typeof child.type === "string") {
    throw new Error("Phoenix Island requires one React component child");
  }
  if (context.mode !== "islands") {
    return child;
  }
  if (context.insideIsland) {
    throw new Error("Phoenix islands cannot be nested");
  }

  const name = componentName(child.type as ComponentType<any>);
  const islandId = context.collect?.(name, child.props, id) ?? id ?? name;
  return createElement(
    "div",
    { "data-phoenix-island": islandId, "data-component": name },
    createElement(
      islandRenderContext.Provider,
      { value: { ...context, insideIsland: true } },
      child,
    ),
  );
}

export function PhoenixRenderProvider({
  mode,
  collect,
  children,
}: {
  mode: RenderMode;
  collect?: IslandRenderContext["collect"];
  children?: ReactNode;
}): ReactElement {
  return createElement(
    islandRenderContext.Provider,
    { value: { mode, insideIsland: false, collect } },
    children,
  );
}

export function registerIsland(name: string, Component: ComponentType<any>): void {
  registeredIslandNames.set(Component, name);
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
    registerIsland(name, Component);
    return createElement(
      Island,
      { id: islandId },
      createElement(Component, props as Props),
    );
  };
}

export async function startPhoenix(options: PhoenixOptions = {}): Promise<PageEnvelope> {
  const documentRef = options.document ?? document;
  const envelope = readPage(documentRef);
  const root = requiredElement(documentRef, "phoenix-root");

  if (envelope.render_mode === "islands") {
    await hydrateIslands(documentRef, envelope, componentRegistry(options.islands));
    return envelope;
  }

  const Page = await requiredComponent(options.pages ?? {}, envelope.page, "page");
  const element = createElement(
    PhoenixRenderProvider,
    { mode: envelope.render_mode },
    createElement(Page, pageProps(envelope)),
  );
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
  const envelope = readPage(document);
  const url = rustRoute(routeName, envelope);
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "Accept": "application/json",
  };
  if (envelope.csrf_token) {
    headers["X-CSRF-Token"] = envelope.csrf_token;
  }
  const response = await fetcher(url, {
    method: "POST",
    headers,
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

export type RustAction<Input, Output> = ((input: Input) => Promise<Output>) & {
  readonly routeName: string;
};

export function createRustAction<Input, Output>(
  routeName: string,
): RustAction<Input, Output> {
  const action = (input: Input) => callRust<Output, Input>(routeName, input);
  return Object.assign(action, { routeName });
}

function rustRoute(
  routeName: string,
  envelope: PageEnvelope = readPage(document),
): string {
  const route = envelope.routes[routeName];
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
  registry: ComponentSource,
): Promise<void> {
  return Promise.all(envelope.islands.map(async (descriptor) => {
    const root = Array.from(documentRef.querySelectorAll("[data-phoenix-island]"))
      .find((element) => element.getAttribute("data-phoenix-island") === descriptor.id);
    if (!root) {
      throw new Error(`Phoenix island root not found: ${descriptor.id}`);
    }
    const Component = await requiredComponent(registry, descriptor.component, "island");
    hydrateRoot(root, createElement(Component, descriptor.props));
  })).then(() => undefined);
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

async function requiredComponent(
  registry: ComponentSource,
  name: string,
  kind: string,
): Promise<ComponentType<any>> {
  const entry = registry[name];
  if (!entry) {
    throw new Error(`Phoenix ${kind} is not registered: ${name}`);
  }
  if (typeof entry === "object" && "load" in entry) {
    const module = await entry.load();
    return module.default;
  }
  return entry;
}

function componentRegistry(
  components: ComponentSource | ComponentList | undefined,
): ComponentSource {
  if (!components) return {};
  if (!Array.isArray(components)) return components as ComponentRegistry;
  return Object.fromEntries(
    components.map((Component) => [componentName(Component), Component]),
  );
}

function componentName(Component: ComponentType<any>): string {
  const registered = registeredIslandNames.get(Component);
  if (registered) return registered;
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
