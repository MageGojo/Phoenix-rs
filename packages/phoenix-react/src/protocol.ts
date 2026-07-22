import type { ComponentType } from "react";

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

export type ComponentSource = ComponentRegistry | Record<
  string,
  ComponentType<any> | ComponentLoader
>;
export type DecryptPage = (payload: EncryptedPayload) => Promise<PageEnvelope>;

export function readPage(documentRef: Document): PageEnvelope {
  const script = requiredElement(documentRef, "phoenix-page");
  return JSON.parse(script.textContent ?? "") as PageEnvelope;
}

export function writePage(documentRef: Document, envelope: PageEnvelope): void {
  requiredElement(documentRef, "phoenix-page").textContent = JSON.stringify(envelope);
}

export function requiredElement(documentRef: Document, id: string): HTMLElement {
  const element = documentRef.getElementById(id);
  if (!element) throw new Error(`Phoenix element not found: #${id}`);
  return element;
}

export function assertPageEnvelope(envelope: PageEnvelope): void {
  if (
    envelope.protocol !== 1 ||
    typeof envelope.page !== "string" ||
    !["spa", "ssr", "islands"].includes(envelope.render_mode)
  ) {
    throw new Error("Invalid Phoenix page envelope");
  }
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
