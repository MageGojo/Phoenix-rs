import type { PageEnvelope } from "./protocol.js";

/**
 * Phoenix secure transport (client half).
 *
 * Wire protocol (must match `phoenix-view` server codec):
 *
 * Handshake — `POST /__phoenix/secure/handshake`, JSON over TLS:
 *   req: { v:1, kex:"ECDH-P256", hkdf:"HKDF-SHA256", aead:"A256GCM",
 *          client_public_key:"<b64url raw 65B uncompressed EC point>" }
 *   res: { v:1, key_id:"<b64url>", server_public_key:"<b64url raw 65B>",
 *          expires_at:<unix secs>, ttl:<secs> }
 *   session key = HKDF-SHA256(ikm = ECDH_x(32B), salt = UTF8(key_id),
 *                             info = "phoenix.secure.session.v1") → AES-256-GCM
 *
 * Encrypted body (both directions) — content-type
 * `application/vnd.phoenix.secure`, header `x-phoenix-encrypted: 1`, binary
 * frame:
 *   [0]   magic      "PHX1"          (4B)
 *   [4]   version    0x01            (1B)
 *   [5]   issued_at  u64 big-endian  (8B)
 *   [13]  expires_at u64 big-endian  (8B)
 *   [21]  nonce                      (12B)
 *   [33]  ciphertext || gcm_tag(16)  (rest)
 *   AAD = frame[0..21] ++ UTF8(key_id) ++ direction
 *
 * `direction` is `"req"` for client→server bodies and `"res"` for
 * server→client bodies. Binding it into the AAD is what stops a captured
 * response frame from being replayed as a request body (and vice versa) — they
 * are otherwise interchangeable ciphertexts under the same session key.
 *
 * Request encryption additionally sends the plaintext's own content type in
 * `X-Phoenix-Content-Type` so the server can restore it after opening.
 *
 * The derived key is imported as a NON-EXTRACTABLE CryptoKey so it cannot be
 * dumped from a live page via `exportKey`.
 */

export const SECURE_HANDSHAKE_PATH = "/__phoenix/secure/handshake";
export const SECURE_CONTENT_TYPE = "application/vnd.phoenix.secure";
const FRAME_MAGIC = "PHX1";
const FRAME_HEADER_BYTES = 21;
const NONCE_BYTES = 12;
const TAG_BYTES = 16;
const HKDF_INFO = "phoenix.secure.session.v1";
/** Direction labels mixed into the AAD; must match Rust's `FrameDirection`. */
const DIRECTION_REQUEST = "req";
const DIRECTION_RESPONSE = "res";
/** How long a sealed request frame stays valid, in seconds. */
const REQUEST_FRAME_TTL = 60;

/** A request body sealed for transmission. */
export interface SealedRequest {
  /** The binary `PHX1` frame to send as the body. */
  body: ArrayBuffer;
  /** Headers replacing the plaintext ones (content type included). */
  headers: Record<string, string>;
}

export interface SecureSession {
  readonly keyId: string;
  readonly expiresAt: number;
  /** Headers to attach to page-protocol requests so the server encrypts replies. */
  requestHeaders(): Record<string, string>;
  /** Decrypt a binary secure frame into a page envelope. */
  decryptFrame(frame: ArrayBuffer): Promise<PageEnvelope>;
  /**
   * Seal a request body so the server opens it before the handler runs.
   *
   * `contentType` is the plaintext's own type (default `application/json`); it
   * travels in a header so the server can restore it. Returns `null` when this
   * session can no longer seal (expired), so the caller can fall back to
   * plaintext rather than send something the server will reject.
   */
  sealRequest(body: string, contentType?: string): Promise<SealedRequest | null>;
  /** True once the negotiated key has passed its server-declared expiry. */
  isExpired(): boolean;
}

interface HandshakeResponse {
  v: number;
  key_id: string;
  server_public_key: string;
  expires_at: number;
  ttl?: number;
}

/**
 * Negotiate a per-session key with the server via ECDH and return a session
 * that can decrypt binary secure frames. Falls back to throwing on any protocol
 * mismatch so the caller can decide whether to continue in plaintext.
 */
export async function establishSecureChannel(
  fetcher: typeof fetch = fetch,
  path: string = SECURE_HANDSHAKE_PATH,
): Promise<SecureSession> {
  const subtle = requireSubtle();
  const keyPair = await subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    false,
    ["deriveKey", "deriveBits"],
  );
  const clientPublicRaw = new Uint8Array(await subtle.exportKey("raw", keyPair.publicKey));

  const response = await fetcher(path, {
    method: "POST",
    headers: { "Content-Type": "application/json", "Accept": "application/json" },
    body: JSON.stringify({
      v: 1,
      kex: "ECDH-P256",
      hkdf: "HKDF-SHA256",
      aead: "A256GCM",
      client_public_key: encodeBase64Url(clientPublicRaw),
    }),
  });
  if (!response.ok) {
    throw new Error(`Phoenix secure handshake failed with ${response.status}`);
  }
  const body = (await response.json()) as HandshakeResponse;
  if (body.v !== 1 || typeof body.key_id !== "string" || typeof body.server_public_key !== "string") {
    throw new Error("Phoenix secure handshake returned an unsupported envelope");
  }

  const serverPublicKey = await subtle.importKey(
    "raw",
    decodeBase64Url(body.server_public_key),
    { name: "ECDH", namedCurve: "P-256" },
    false,
    [],
  );
  const sharedBits = await subtle.deriveBits(
    { name: "ECDH", public: serverPublicKey },
    keyPair.privateKey,
    256,
  );
  const hkdfKey = await subtle.importKey("raw", sharedBits, "HKDF", false, ["deriveKey"]);
  // extractable:false — the negotiated key stays inside the Web Crypto boundary.
  const sessionKey = await subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: utf8(body.key_id),
      info: utf8(HKDF_INFO),
    },
    hkdfKey,
    { name: "AES-GCM", length: 256 },
    false,
    ["decrypt", "encrypt"],
  );

  return new NegotiatedSession(body.key_id, body.expires_at, sessionKey, subtle);
}

class NegotiatedSession implements SecureSession {
  constructor(
    public readonly keyId: string,
    public readonly expiresAt: number,
    private readonly key: CryptoKey,
    private readonly subtle: SubtleCrypto,
  ) {}

  requestHeaders(): Record<string, string> {
    return { "X-Phoenix-Secure": "1", "X-Phoenix-Key": this.keyId };
  }

  isExpired(): boolean {
    return this.expiresAt < Math.floor(Date.now() / 1000);
  }

  async decryptFrame(frame: ArrayBuffer): Promise<PageEnvelope> {
    const bytes = new Uint8Array(frame);
    if (bytes.byteLength < FRAME_HEADER_BYTES + NONCE_BYTES + TAG_BYTES) {
      throw new Error("Phoenix secure frame is truncated");
    }
    if (new TextDecoder().decode(bytes.subarray(0, 4)) !== FRAME_MAGIC || bytes[4] !== 1) {
      throw new Error("Phoenix secure frame has an unknown format");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const expiresAt = Number(view.getBigUint64(13, false));
    if (expiresAt < Math.floor(Date.now() / 1000)) {
      throw new Error("Phoenix secure frame has expired");
    }
    const header = bytes.subarray(0, FRAME_HEADER_BYTES);
    const nonce = bytes.subarray(FRAME_HEADER_BYTES, FRAME_HEADER_BYTES + NONCE_BYTES);
    const sealed = bytes.subarray(FRAME_HEADER_BYTES + NONCE_BYTES);
    const aad = concatBytes(header, utf8(this.keyId + DIRECTION_RESPONSE));

    const plaintext = await this.subtle.decrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad },
      this.key,
      sealed,
    );
    return JSON.parse(new TextDecoder().decode(plaintext)) as PageEnvelope;
  }

  async sealRequest(
    body: string,
    contentType = "application/json",
  ): Promise<SealedRequest | null> {
    if (this.isExpired()) return null;

    const issuedAt = Math.floor(Date.now() / 1000);
    const header = new Uint8Array(FRAME_HEADER_BYTES);
    header.set(utf8(FRAME_MAGIC), 0);
    header[4] = 1;
    const view = new DataView(header.buffer);
    view.setBigUint64(5, BigInt(issuedAt), false);
    view.setBigUint64(13, BigInt(issuedAt + REQUEST_FRAME_TTL), false);

    // A fresh random nonce per frame — never reused for a given key.
    const nonce = new Uint8Array(NONCE_BYTES);
    globalThis.crypto.getRandomValues(nonce);
    const aad = concatBytes(header, utf8(this.keyId + DIRECTION_REQUEST));
    const sealed = new Uint8Array(
      await this.subtle.encrypt(
        { name: "AES-GCM", iv: nonce, additionalData: aad },
        this.key,
        utf8(body),
      ),
    );

    const frame = new Uint8Array(header.byteLength + nonce.byteLength + sealed.byteLength);
    frame.set(header, 0);
    frame.set(nonce, header.byteLength);
    frame.set(sealed, header.byteLength + nonce.byteLength);
    return {
      body: frame.buffer,
      headers: {
        ...this.requestHeaders(),
        "Content-Type": SECURE_CONTENT_TYPE,
        "X-Phoenix-Encrypted": "1",
        "X-Phoenix-Content-Type": contentType,
      },
    };
  }
}

/** True when a response carries a binary Phoenix secure frame. */
export function isSecureResponse(response: Response): boolean {
  if (response.headers.get("x-phoenix-encrypted") !== "1") return false;
  const contentType = response.headers.get("content-type") ?? "";
  return contentType.startsWith(SECURE_CONTENT_TYPE);
}

function utf8(value: string): Uint8Array<ArrayBuffer> {
  const encoded = new TextEncoder().encode(value);
  return new Uint8Array(encoded.buffer.slice(0));
}

function requireSubtle(): SubtleCrypto {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    throw new Error("Phoenix secure transport requires Web Crypto (crypto.subtle)");
  }
  return subtle;
}

function concatBytes(
  left: Uint8Array<ArrayBuffer>,
  right: Uint8Array<ArrayBuffer>,
): Uint8Array<ArrayBuffer> {
  const output = new Uint8Array(left.byteLength + right.byteLength);
  output.set(left, 0);
  output.set(right, left.byteLength);
  return output;
}

export function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function decodeBase64Url(value: string): Uint8Array<ArrayBuffer> {
  const padded = value.replace(/-/g, "+").replace(/_/g, "/")
    + "=".repeat((4 - (value.length % 4)) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return new Uint8Array(bytes.buffer);
}
