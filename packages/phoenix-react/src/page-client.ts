import { RustCallError } from "./actions.js";
import { partialReloadHeaders } from "./partial.js";
import { isRecord, type DecryptPage, type EncryptedPayload, type PageEnvelope } from "./protocol.js";

export interface FetchPageOptions {
  signal?: AbortSignal;
  method?: string;
  body?: BodyInit | null;
  headers?: Record<string, string>;
  only?: string[];
  except?: string[];
}

export interface SubmitPageOptions {
  method?: string;
  data?: Record<string, unknown>;
  signal?: AbortSignal;
  headers?: Record<string, string>;
  decrypt?: DecryptPage;
  fetcher?: typeof fetch;
  only?: string[];
  except?: string[];
}

export async function fetchPage(
  url: string,
  decrypt?: DecryptPage,
  fetcher: typeof fetch = fetch,
  options: FetchPageOptions = {},
): Promise<PageEnvelope> {
  const method = (options.method ?? "GET").toUpperCase();
  const headers: Record<string, string> = {
    "X-Phoenix-Page": "1",
    ...partialReloadHeaders(options),
    ...options.headers,
  };
  const request: RequestInit = { headers };
  if (options.signal) request.signal = options.signal;
  if (method !== "GET") {
    request.method = method;
    if (options.body !== undefined) request.body = options.body;
  }
  const response = await fetcher(url, request);
  if (!response.ok) {
    throw new Error(`Phoenix page request failed with ${response.status}`);
  }
  return parsePageResponse(response, decrypt);
}

/**
 * Submit a page-protocol mutation (POST/PUT/PATCH/DELETE).
 * On 422, throws {@link RustCallError} with field details (no page swap).
 */
export async function submitPage(
  url: string,
  options: SubmitPageOptions = {},
): Promise<PageEnvelope> {
  const method = (options.method ?? "POST").toUpperCase();
  const fetcher = options.fetcher ?? fetch;
  const headers: Record<string, string> = {
    "X-Phoenix-Page": "1",
    ...partialReloadHeaders(options),
    ...options.headers,
  };
  let body: BodyInit | undefined;
  if (options.data !== undefined) {
    body = JSON.stringify(options.data);
    if (!hasHeader(headers, "Content-Type")) {
      headers["Content-Type"] = "application/json";
    }
  }
  const request: RequestInit = { method, headers };
  if (options.signal) request.signal = options.signal;
  if (body !== undefined) request.body = body;

  const response = await fetcher(url, request);
  if (response.status === 422) {
    const details = await response.json().catch(() => null) as unknown;
    const message = isRecord(details) && typeof details.message === "string"
      ? details.message
      : "The submitted data is invalid.";
    throw new RustCallError(422, message, details);
  }
  if (!response.ok) {
    throw new Error(`Phoenix page request failed with ${response.status}`);
  }
  return parsePageResponse(response, options.decrypt);
}

async function parsePageResponse(
  response: Response,
  decrypt?: DecryptPage,
): Promise<PageEnvelope> {
  if (response.headers.get("x-phoenix-encrypted") === "1") {
    if (!decrypt) {
      throw new Error("Encrypted Phoenix page response requires a decrypt callback");
    }
    return decrypt((await response.json()) as EncryptedPayload);
  }
  return (await response.json()) as PageEnvelope;
}

function hasHeader(headers: Record<string, string>, name: string): boolean {
  const lower = name.toLowerCase();
  return Object.keys(headers).some((key) => key.toLowerCase() === lower);
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
    if (!key) throw new Error(`No key is available for ${payload.key_id}`);
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
