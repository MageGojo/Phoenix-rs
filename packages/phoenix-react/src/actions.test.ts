// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  callRust,
  createAes256GcmDecryptor,
  createRustAction,
  fetchPage,
  RustCallError,
  type EncryptedPayload,
  type PageEnvelope,
} from "./index.js";

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("Phoenix React actions", () => {
  it("posts input to a Rust action and returns its JSON result", async () => {
    installRoutes({ "members.store": "/api/members" }, "csrf-action-token");
    const fetcher = async (url: string | URL | Request, init?: RequestInit) => {
      expect(url).toBe("/api/members");
      expect(init?.method).toBe("POST");
      expect(new Headers(init?.headers).get("content-type")).toBe("application/json");
      expect(new Headers(init?.headers).get("x-csrf-token")).toBe("csrf-action-token");
      expect(init?.body).toBe(JSON.stringify({ name: "Lin" }));
      return new Response(JSON.stringify({ id: 101, name: "Lin" }), {
        status: 201,
        headers: { "content-type": "application/json" },
      });
    };

    await expect(callRust<{ id: number; name: string }, { name: string }>(
      "members.store",
      { name: "Lin" },
      fetcher as typeof fetch,
    )).resolves.toEqual({ id: 101, name: "Lin" });
  });

  it("creates a callable Rust action with inferred input and output slots", async () => {
    installRoutes({ "members.store": "/api/members" });
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ id: 101, name: "Ada" }), {
        status: 201,
        headers: { "content-type": "application/json" },
      }),
    );
    const store = createRustAction<{ name: string }, { id: number; name: string }>(
      "members.store",
    );

    await expect(store({ name: "Ada" })).resolves.toEqual({ id: 101, name: "Ada" });
    expect(store.routeName).toBe("members.store");
    expect(fetchMock).toHaveBeenCalledWith("/api/members", expect.objectContaining({
      body: JSON.stringify({ name: "Ada" }),
      method: "POST",
    }));
  });

  it("surfaces a Rust action error with normalized field details", async () => {
    installRoutes({ "members.store": "/api/members" });
    const fetcher = async () => new Response(JSON.stringify({
      message: "The submitted data is invalid.",
      errors: { name: [{ rule: "required", message: "The name field is required." }] },
    }), {
      status: 422,
      headers: { "content-type": "application/json" },
    });

    const request = callRust("members.store", { name: "" }, fetcher as typeof fetch);
    await expect(request).rejects.toBeInstanceOf(RustCallError);
    await expect(request).rejects.toMatchObject({
      status: 422,
      fieldErrors: {
        name: [{ rule: "required", message: "The name field is required." }],
      },
    });
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

function installRoutes(routes: Record<string, string>, csrfToken?: string): void {
  document.body.innerHTML = `<script id="phoenix-page" type="application/json">${JSON.stringify({ routes, csrf_token: csrfToken })}</script>`;
}
