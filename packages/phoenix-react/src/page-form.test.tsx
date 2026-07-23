// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  FieldError,
  PageForm,
  RustCallError,
  resetConfirmImplementation,
  setConfirmImplementation,
  startPhoenix,
  stopPhoenix,
} from "./index.js";
import {
  installPage,
  nextNavigation,
  pageEnvelope,
  pageResponse,
} from "./test-utils.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

afterEach(async () => {
  await act(async () => stopPhoenix());
  resetConfirmImplementation();
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("Phoenix page forms", () => {
  it("submits via visit and renders the next page on success", async () => {
    interface CreateInput {
      title: string;
    }

    const created = pageEnvelope("posts/show", { title: "Hello" }, { title: "Post" });
    const fetcher = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      expect(new URL(url.toString()).pathname).toBe("/posts");
      expect(init?.method).toBe("POST");
      expect(new Headers(init?.headers).get("x-phoenix-page")).toBe("1");
      expect(new Headers(init?.headers).get("content-type")).toBe("application/json");
      expect(init?.body).toBe(JSON.stringify({ title: "Hello" }));
      return pageResponse(created);
    });
    const onSuccess = vi.fn();

    function CreatePage() {
      return (
        <PageForm<CreateInput>
          action="/posts"
          method="post"
          initialValues={{ title: "" }}
          replace
          onSuccess={onSuccess}
        >
          {(form) => (
            <>
              <input
                id="title"
                name="title"
                value={form.data.title}
                onChange={(event) => form.setField("title", event.currentTarget.value)}
              />
              <button type="submit" disabled={form.processing}>Publish</button>
            </>
          )}
        </PageForm>
      );
    }

    function ShowPage({ title }: { title: string }) {
      return <main id="show-main"><h1>{title}</h1></main>;
    }

    window.history.replaceState(null, "", "/posts/new");
    installPage(pageEnvelope("posts/new", {}));
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});

    await act(async () => {
      await startPhoenix({
        pages: {
          "posts/new": CreatePage,
          "posts/show": ShowPage,
        },
        fetcher: fetcher as typeof fetch,
      });
    });

    const input = document.getElementById("title") as HTMLInputElement;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
        ?.call(input, "Hello");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const finished = nextNavigation("phoenix:navigation-success");
    await act(async () => {
      submitForm();
      await finished;
    });

    expect(fetcher).toHaveBeenCalledOnce();
    expect(onSuccess).toHaveBeenCalledWith(expect.objectContaining({ page: "posts/show" }));
    expect(window.location.pathname).toBe("/posts");
    expect(document.getElementById("show-main")?.textContent).toContain("Hello");
  });

  it("maps 422 responses into field errors without navigating", async () => {
    interface CreateInput {
      title: string;
    }

    const fetcher = vi.fn(async () => new Response(JSON.stringify({
      message: "Invalid",
      errors: {
        title: [{ rule: "required", message: "Title is required." }],
      },
    }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
    const onError = vi.fn();
    const onSuccess = vi.fn();

    function CreatePage() {
      return (
        <PageForm<CreateInput>
          action="/posts"
          initialValues={{ title: "" }}
          onError={onError}
          onSuccess={onSuccess}
        >
          {(form) => (
            <>
              <FieldError errors={form.errors} name="title" id="title-error" />
              <button type="submit">Publish</button>
            </>
          )}
        </PageForm>
      );
    }

    window.history.replaceState(null, "", "/posts/new");
    installPage(pageEnvelope("posts/new", {}));
    await act(async () => {
      await startPhoenix({
        pages: { "posts/new": CreatePage },
        fetcher: fetcher as typeof fetch,
      });
    });

    const failed = new Promise<void>((resolve) => {
      onError.mockImplementationOnce(() => resolve());
    });
    await act(async () => {
      submitForm();
      await failed;
    });

    expect(fetcher).toHaveBeenCalledOnce();
    expect(onSuccess).not.toHaveBeenCalled();
    expect(onError).toHaveBeenCalledWith(expect.any(RustCallError), {
      title: [{ rule: "required", message: "Title is required." }],
    });
    expect(document.getElementById("title-error")?.textContent).toBe("Title is required.");
    expect(window.location.pathname).toBe("/posts/new");
  });

  it("skips the visit when confirm is cancelled", async () => {
    const confirmFn = vi.fn(() => false);
    setConfirmImplementation(confirmFn);
    const fetcher = vi.fn(async () => pageResponse(pageEnvelope("posts/show", {})));
    const onSuccess = vi.fn();
    const onError = vi.fn();

    function CreatePage() {
      return (
        <PageForm
          action="/posts"
          initialValues={{ title: "Hello" }}
          confirm="Publish this post?"
          onSuccess={onSuccess}
          onError={onError}
        >
          <button type="submit">Publish</button>
        </PageForm>
      );
    }

    installPage(pageEnvelope("posts/new", {}));
    await act(async () => {
      await startPhoenix({
        pages: { "posts/new": CreatePage },
        fetcher: fetcher as typeof fetch,
      });
    });

    await act(async () => {
      submitForm();
      await Promise.resolve();
    });

    expect(confirmFn).toHaveBeenCalledWith("Publish this post?");
    expect(fetcher).not.toHaveBeenCalled();
    expect(onSuccess).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });
});

function submitForm(): void {
  document.querySelector("form")?.dispatchEvent(new Event("submit", {
    bubbles: true,
    cancelable: true,
  }));
}
