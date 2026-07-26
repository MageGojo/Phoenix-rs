// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RustCallError, createRustAction } from "./actions.js";
import { Form } from "./forms.js";
import { installPage, pageEnvelope } from "./test-utils.js";
import { hasFileValue, isFileValue, toFormData, uploadRust } from "./uploads.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

class FakeUpload {
  handlers = new Map<string, (event: ProgressEvent) => void>();
  addEventListener(name: string, handler: (event: ProgressEvent) => void): void {
    this.handlers.set(name, handler);
  }
}

class FakeXhr {
  static instances: FakeXhr[] = [];
  method = "";
  url = "";
  headers: Record<string, string> = {};
  body: FormData | null = null;
  responseType = "";
  responseText = "";
  status = 0;
  aborted = false;
  upload = new FakeUpload();
  private handlers = new Map<string, () => void>();

  constructor() {
    FakeXhr.instances.push(this);
  }

  open(method: string, url: string): void {
    this.method = method;
    this.url = url;
  }

  setRequestHeader(name: string, value: string): void {
    this.headers[name] = value;
  }

  addEventListener(name: string, handler: () => void): void {
    this.handlers.set(name, handler);
  }

  send(body: FormData): void {
    this.body = body;
  }

  abort(): void {
    this.aborted = true;
    this.handlers.get("abort")?.();
  }

  emitProgress(loaded: number, total: number): void {
    this.upload.handlers.get("progress")?.({
      lengthComputable: true,
      loaded,
      total,
    } as ProgressEvent);
  }

  respond(status: number, body: unknown): void {
    this.status = status;
    this.responseText = JSON.stringify(body);
    this.handlers.get("load")?.();
  }
}

beforeEach(() => {
  FakeXhr.instances = [];
  vi.stubGlobal("XMLHttpRequest", FakeXhr as unknown as typeof XMLHttpRequest);
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

function installUploadPage(): void {
  const envelope = pageEnvelope("home", {});
  envelope.routes = { "posts.cover": "/api/posts/cover" };
  envelope.csrf_token = "csrf-1";
  installPage(envelope);
}

describe("file detection and FormData serialization", () => {
  it("detects File, Blob, and file arrays", () => {
    const file = new File(["x"], "a.png", { type: "image/png" });
    expect(isFileValue(file)).toBe(true);
    expect(isFileValue([file, file])).toBe(true);
    expect(isFileValue("text")).toBe(false);
    expect(isFileValue([1, 2])).toBe(false);
    expect(hasFileValue({ title: "t", cover: file })).toBe(true);
    expect(hasFileValue({ title: "t" })).toBe(false);
  });

  it("serializes scalars, files, file lists, and nested objects", () => {
    const cover = new File(["x"], "cover.png", { type: "image/png" });
    const extra = new File(["y"], "extra.png", { type: "image/png" });
    const body = toFormData({
      title: "你好",
      pinned: true,
      count: 3,
      cover,
      gallery: [cover, extra],
      tags: ["a", "b"],
      skip: null,
      alsoSkip: undefined,
    });
    expect(body.get("title")).toBe("你好");
    expect(body.get("pinned")).toBe("true");
    expect(body.get("count")).toBe("3");
    expect(body.get("cover")).toBe(cover);
    expect(body.getAll("gallery")).toEqual([cover, extra]);
    expect(body.get("tags")).toBe('["a","b"]');
    expect(body.has("skip")).toBe(false);
    expect(body.has("alsoSkip")).toBe(false);
  });
});

describe("uploadRust", () => {
  it("posts multipart to the named route with CSRF and reports progress", async () => {
    installUploadPage();
    const file = new File(["binary"], "cover.png", { type: "image/png" });
    const progress: number[] = [];
    const pending = uploadRust<{ url: string }>("posts.cover", { cover: file }, {
      onUploadProgress: (value) => progress.push(value),
    });

    const xhr = FakeXhr.instances[0];
    expect(xhr.method).toBe("POST");
    expect(xhr.url).toBe("/api/posts/cover");
    expect(xhr.headers.Accept).toBe("application/json");
    expect(xhr.headers["X-CSRF-Token"]).toBe("csrf-1");
    expect(xhr.headers["Content-Type"]).toBeUndefined();
    expect(xhr.body?.get("cover")).toBe(file);

    xhr.emitProgress(5, 10);
    xhr.respond(201, { url: "/storage/cover.png" });
    await expect(pending).resolves.toEqual({ url: "/storage/cover.png" });
    expect(progress).toEqual([0.5, 1]);
  });

  it("maps 422 responses onto RustCallError field errors", async () => {
    installUploadPage();
    const pending = uploadRust("posts.cover", {
      cover: new File(["z"], "z.bin"),
    });
    FakeXhr.instances[0].respond(422, {
      message: "The submitted data is invalid.",
      errors: { cover: [{ rule: "mime", message: "封面必须是图片" }] },
    });
    const error = await pending.catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(RustCallError);
    expect((error as RustCallError).status).toBe(422);
    expect((error as RustCallError).fieldErrors.cover?.[0]?.message).toBe("封面必须是图片");
  });
});

describe("useForm multipart integration", () => {
  let root: Root | null = null;

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
  });

  it("switches to multipart upload when data contains a file and exposes progress", async () => {
    installUploadPage();
    const action = createRustAction<{ cover: File | null }, { url: string }>("posts.cover");
    const container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);

    const file = new File(["img"], "cover.png", { type: "image/png" });
    act(() => {
      root?.render(
        <Form action={action} initialValues={{ cover: file }}>
          {(form) => (
            <output id="progress">{form.progress === null ? "idle" : form.progress}</output>
          )}
        </Form>,
      );
    });

    const formElement = container.querySelector("form");
    await act(async () => {
      formElement?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    const xhr = FakeXhr.instances[0];
    expect(xhr).toBeDefined();
    expect(xhr.body?.get("cover")).toBe(file);

    await act(async () => {
      xhr.emitProgress(1, 4);
    });
    expect(container.querySelector("#progress")?.textContent).toBe("0.25");

    await act(async () => {
      xhr.respond(201, { url: "/storage/cover.png" });
    });
    expect(container.querySelector("#progress")?.textContent).toBe("idle");
  });
});
