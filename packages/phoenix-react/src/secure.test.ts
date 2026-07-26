// @vitest-environment node
import { describe, expect, it, vi } from "vitest";

import {
  decodeBase64Url,
  encodeBase64Url,
  establishSecureChannel,
  isSecureResponse,
  SECURE_CONTENT_TYPE,
} from "./secure.js";
import type { PageEnvelope } from "./protocol.js";

const subtle = globalThis.crypto.subtle;
const encoder = new TextEncoder();

/** Emulate the server: derive the same session key and seal a frame. */
async function serverSeal(
  clientPublicRaw: Uint8Array,
  keyId: string,
  envelope: PageEnvelope,
  expiresAt: number,
): Promise<{ serverPublicRaw: Uint8Array; frame: Uint8Array }> {
  const serverPair = await subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    true,
    ["deriveBits"],
  ) as CryptoKeyPair;
  const serverPublicRaw = new Uint8Array(await subtle.exportKey("raw", serverPair.publicKey));
  const clientPublic = await subtle.importKey(
    "raw",
    clientPublicRaw,
    { name: "ECDH", namedCurve: "P-256" },
    false,
    [],
  );
  const shared = await subtle.deriveBits(
    { name: "ECDH", public: clientPublic },
    serverPair.privateKey,
    256,
  );
  const hkdf = await subtle.importKey("raw", shared, "HKDF", false, ["deriveKey"]);
  const key = await subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: encoder.encode(keyId), info: encoder.encode("phoenix.secure.session.v1") },
    hkdf,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt"],
  );

  const header = new Uint8Array(21);
  header.set(encoder.encode("PHX1"), 0);
  header[4] = 1;
  new DataView(header.buffer).setBigUint64(13, BigInt(expiresAt), false);
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const aad = new Uint8Array([...header, ...encoder.encode(`${keyId}res`)]);
  const sealed = new Uint8Array(await subtle.encrypt(
    { name: "AES-GCM", iv: nonce, additionalData: aad },
    key,
    encoder.encode(JSON.stringify(envelope)),
  ));
  const frame = new Uint8Array(header.byteLength + nonce.byteLength + sealed.byteLength);
  frame.set(header, 0);
  frame.set(nonce, header.byteLength);
  frame.set(sealed, header.byteLength + nonce.byteLength);
  return { serverPublicRaw, frame };
}

function envelope(): PageEnvelope {
  return {
    protocol: 1, render_mode: "spa", page: "secret", props: { flag: "🔒" },
    shared: {}, errors: {}, flash: {}, contract_hash: null, asset_version: null,
    request_id: null, head: {}, csrf_token: null, routes: {}, islands: [],
  };
}

describe("base64url", () => {
  it("round-trips arbitrary bytes without padding", () => {
    const bytes = new Uint8Array([0, 1, 2, 250, 251, 252, 253, 254, 255]);
    expect(encodeBase64Url(bytes)).not.toContain("=");
    expect([...decodeBase64Url(encodeBase64Url(bytes))]).toEqual([...bytes]);
  });
});

describe("isSecureResponse", () => {
  it("matches only the binary secure content type with the encrypted flag", () => {
    const secure = new Response(new ArrayBuffer(0), {
      headers: { "x-phoenix-encrypted": "1", "content-type": SECURE_CONTENT_TYPE },
    });
    const json = new Response("{}", {
      headers: { "x-phoenix-encrypted": "1", "content-type": "application/json" },
    });
    expect(isSecureResponse(secure)).toBe(true);
    expect(isSecureResponse(json)).toBe(false);
  });
});

describe("establishSecureChannel", () => {
  it("negotiates a key and decrypts a server-sealed binary frame", async () => {
    const page = envelope();
    const keyId = "sess-abc";
    const expiresAt = Math.floor(Date.now() / 1000) + 60;
    let sealed: { serverPublicRaw: Uint8Array; frame: Uint8Array } | null = null;

    const fetcher = vi.fn(async (_url: string, init?: RequestInit) => {
      const req = JSON.parse(String(init?.body)) as { client_public_key: string };
      sealed = await serverSeal(decodeBase64Url(req.client_public_key), keyId, page, expiresAt);
      return new Response(JSON.stringify({
        v: 1, key_id: keyId,
        server_public_key: encodeBase64Url(sealed.serverPublicRaw),
        expires_at: expiresAt, ttl: 60,
      }), { headers: { "content-type": "application/json" } });
    });

    const session = await establishSecureChannel(fetcher as unknown as typeof fetch);
    expect(session.keyId).toBe(keyId);
    expect(session.isExpired()).toBe(false);
    expect(session.requestHeaders()).toEqual({ "X-Phoenix-Secure": "1", "X-Phoenix-Key": keyId });

    const buffer = sealed!.frame.buffer.slice(
      sealed!.frame.byteOffset,
      sealed!.frame.byteOffset + sealed!.frame.byteLength,
    );
    await expect(session.decryptFrame(buffer)).resolves.toEqual(page);
  });

  it("rejects a frame sealed under a different key", async () => {
    const keyId = "sess-real";
    const expiresAt = Math.floor(Date.now() / 1000) + 60;
    const fetcher = vi.fn(async () => {
      const decoy = await subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]) as CryptoKeyPair;
      const raw = new Uint8Array(await subtle.exportKey("raw", decoy.publicKey));
      return new Response(JSON.stringify({
        v: 1, key_id: keyId, server_public_key: encodeBase64Url(raw), expires_at: expiresAt,
      }), { headers: { "content-type": "application/json" } });
    });
    const session = await establishSecureChannel(fetcher as unknown as typeof fetch);
    // A frame sealed by an unrelated server keypair must fail authentication.
    const foreign = await serverSeal(
      new Uint8Array(await subtle.exportKey("raw",
        ((await subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair).publicKey)),
      keyId, envelope(), expiresAt,
    );
    await expect(session.decryptFrame(foreign.frame.buffer as ArrayBuffer)).rejects.toThrow();
  });

  it("throws on an unsupported handshake envelope", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({ v: 2 }), {
      headers: { "content-type": "application/json" },
    }));
    await expect(establishSecureChannel(fetcher as unknown as typeof fetch)).rejects.toThrow(
      "unsupported envelope",
    );
  });
});

describe("sealRequest", () => {
  /** Handshake against a server whose derived key we keep, as the server does. */
  async function negotiate(
    keyId: string,
    expiresAt: number,
  ): Promise<{ session: Awaited<ReturnType<typeof establishSecureChannel>>; key: CryptoKey }> {
    let key!: CryptoKey;
    const fetcher = vi.fn(async (_url: string, init?: RequestInit) => {
      const req = JSON.parse(String(init?.body)) as { client_public_key: string };
      const serverPair = await subtle.generateKey(
        { name: "ECDH", namedCurve: "P-256" },
        true,
        ["deriveBits"],
      ) as CryptoKeyPair;
      const clientPublic = await subtle.importKey(
        "raw",
        decodeBase64Url(req.client_public_key),
        { name: "ECDH", namedCurve: "P-256" },
        false,
        [],
      );
      const shared = await subtle.deriveBits(
        { name: "ECDH", public: clientPublic },
        serverPair.privateKey,
        256,
      );
      const hkdf = await subtle.importKey("raw", shared, "HKDF", false, ["deriveKey"]);
      key = await subtle.deriveKey(
        {
          name: "HKDF",
          hash: "SHA-256",
          salt: encoder.encode(keyId),
          info: encoder.encode("phoenix.secure.session.v1"),
        },
        hkdf,
        { name: "AES-GCM", length: 256 },
        false,
        ["decrypt"],
      );
      const raw = new Uint8Array(await subtle.exportKey("raw", serverPair.publicKey));
      return new Response(JSON.stringify({
        v: 1, key_id: keyId, server_public_key: encodeBase64Url(raw), expires_at: expiresAt,
      }), { headers: { "content-type": "application/json" } });
    });
    const session = await establishSecureChannel(fetcher as unknown as typeof fetch);
    return { session, key };
  }

  /** Open a client-sealed request frame the way the Rust server does. */
  async function serverOpen(
    key: CryptoKey,
    keyId: string,
    frame: ArrayBuffer,
    direction = "req",
  ): Promise<string> {
    const bytes = new Uint8Array(frame);
    const header = bytes.subarray(0, 21);
    const nonce = bytes.subarray(21, 33);
    const sealed = bytes.subarray(33);
    const aad = new Uint8Array([...header, ...encoder.encode(keyId + direction)]);
    const plaintext = await subtle.decrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad },
      key,
      sealed,
    );
    return new TextDecoder().decode(plaintext);
  }

  it("seals a request body into a frame the server can open", async () => {
    const keyId = "sess-req";
    const { session, key } = await negotiate(keyId, Math.floor(Date.now() / 1000) + 60);

    const sealed = await session.sealRequest(JSON.stringify({ title: "draft" }));
    expect(sealed).not.toBeNull();
    expect(sealed!.headers).toEqual({
      "X-Phoenix-Secure": "1",
      "X-Phoenix-Key": keyId,
      "Content-Type": SECURE_CONTENT_TYPE,
      "X-Phoenix-Encrypted": "1",
      "X-Phoenix-Content-Type": "application/json",
    });

    const bytes = new Uint8Array(sealed!.body);
    expect(new TextDecoder().decode(bytes.subarray(0, 4))).toBe("PHX1");
    expect(bytes[4]).toBe(1);
    // header(21) + nonce(12) + ciphertext + tag(16)
    expect(bytes.byteLength).toBe(21 + 12 + JSON.stringify({ title: "draft" }).length + 16);

    await expect(serverOpen(key, keyId, sealed!.body)).resolves.toBe(
      JSON.stringify({ title: "draft" }),
    );
  });

  it("carries the plaintext content type so the server can restore it", async () => {
    const keyId = "sess-form";
    const { session, key } = await negotiate(keyId, Math.floor(Date.now() / 1000) + 60);
    const sealed = await session.sealRequest("a=1&b=2", "application/x-www-form-urlencoded");
    expect(sealed!.headers["X-Phoenix-Content-Type"]).toBe("application/x-www-form-urlencoded");
    await expect(serverOpen(key, keyId, sealed!.body)).resolves.toBe("a=1&b=2");
  });

  it("binds the direction so a request frame cannot be read as a response", async () => {
    const keyId = "sess-dir";
    const { session, key } = await negotiate(keyId, Math.floor(Date.now() / 1000) + 60);
    const sealed = await session.sealRequest("{}");
    // Same key, same key_id, only the direction label differs.
    await expect(serverOpen(key, keyId, sealed!.body, "res")).rejects.toThrow();
  });

  it("returns null once the session has expired", async () => {
    const keyId = "sess-old";
    const { session } = await negotiate(keyId, Math.floor(Date.now() / 1000) - 1);
    expect(session.isExpired()).toBe(true);
    await expect(session.sealRequest("{}")).resolves.toBeNull();
  });

  it("uses a fresh nonce for every frame", async () => {
    const { session } = await negotiate("sess-nonce", Math.floor(Date.now() / 1000) + 60);
    const first = new Uint8Array((await session.sealRequest("{}"))!.body).subarray(21, 33);
    const second = new Uint8Array((await session.sealRequest("{}"))!.body).subarray(21, 33);
    expect([...first]).not.toEqual([...second]);
  });
});
