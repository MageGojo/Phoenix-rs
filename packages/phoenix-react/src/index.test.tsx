// @vitest-environment jsdom
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";

import {
  createAes256GcmDecryptor,
  fetchPage,
  island,
  type EncryptedPayload,
  type PageEnvelope,
} from "./index.js";

describe("Phoenix React client", () => {
  it("marks an island with its stable backend id", () => {
    const Counter = ({ count }: { count: number }) => createElement("button", null, count);
    const CounterIsland = island("counter", Counter);

    expect(renderToString(createElement(CounterIsland, { islandId: "counter-7", count: 7 })))
      .toContain('data-phoenix-island="counter-7"');
  });

  it("requests the page protocol without requiring encryption", async () => {
    const envelope = { protocol: 1, page: "home", render_mode: "islands" } as PageEnvelope;
    const fetcher = async (_url: string | URL | Request, init?: RequestInit) => {
      expect(new Headers(init?.headers).get("x-phoenix-page")).toBe("1");
      return new Response(JSON.stringify(envelope), {
        headers: { "content-type": "application/json", "x-phoenix-encrypted": "0" },
      });
    };

    await expect(fetchPage("/", undefined, fetcher as typeof fetch)).resolves.toEqual(envelope);
  });

  it("requires an explicit decrypt callback for encrypted pages", async () => {
    const fetcher = async () => new Response("{}", {
      headers: { "content-type": "application/json", "x-phoenix-encrypted": "1" },
    });

    await expect(fetchPage("/private", undefined, fetcher as typeof fetch))
      .rejects.toThrow("requires a decrypt callback");
  });

  it("decrypts the authenticated AES-GCM envelope format", async () => {
    const rawKey = new Uint8Array(32).fill(9);
    const key = await crypto.subtle.importKey("raw", rawKey, "AES-GCM", false, [
      "encrypt",
      "decrypt",
    ]);
    const issuedAt = Math.floor(Date.now() / 1000);
    const expiresAt = issuedAt + 60;
    const nonce = new Uint8Array(12).fill(3);
    const page = { protocol: 1, page: "secure", render_mode: "islands" };
    const aad = new TextEncoder().encode(
      `phoenix.page.v1|test-key|page-navigation|${issuedAt}|${expiresAt}`,
    );
    const sealed = new Uint8Array(await crypto.subtle.encrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad },
      key,
      new TextEncoder().encode(JSON.stringify(page)),
    ));
    const payload: EncryptedPayload = {
      version: 1,
      algorithm: "A256GCM",
      key_id: "test-key",
      purpose: "page-navigation",
      issued_at: issuedAt,
      expires_at: expiresAt,
      nonce: base64Url(nonce),
      ciphertext: base64Url(sealed.slice(0, -16)),
      tag: base64Url(sealed.slice(-16)),
    };

    await expect(createAes256GcmDecryptor({ "test-key": key })(payload))
      .resolves.toMatchObject(page);
  });
});

function base64Url(value: Uint8Array): string {
  return btoa(String.fromCharCode(...value))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
}
