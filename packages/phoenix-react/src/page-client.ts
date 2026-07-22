import type { DecryptPage, EncryptedPayload, PageEnvelope } from "./protocol.js";

export interface FetchPageOptions {
  signal?: AbortSignal;
}

export async function fetchPage(
  url: string,
  decrypt?: DecryptPage,
  fetcher: typeof fetch = fetch,
  options: FetchPageOptions = {},
): Promise<PageEnvelope> {
  const request: RequestInit = {
    headers: { "X-Phoenix-Page": "1" },
  };
  if (options.signal) request.signal = options.signal;
  const response = await fetcher(url, request);
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
