// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  FieldError,
  Form,
  getPhoenixNavigator,
  resetConfirmImplementation,
  RustCallError,
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
  sessionStorage.clear();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
});

describe("Phoenix typed forms", () => {
  it("maps 422 responses into field errors and clears an edited field", async () => {
    interface LoginInput {
      email: string;
      password: string;
    }
    interface LoginResult {
      user: string;
    }

    let valid = false;
    let submittedSignal: AbortSignal | undefined;
    const action = vi.fn(async (
      input: LoginInput,
      options?: { signal?: AbortSignal },
    ): Promise<LoginResult> => {
      submittedSignal = options?.signal;
      if (!valid) {
        throw new RustCallError<LoginInput>(422, "Invalid login", {
          errors: {
            email: [{ rule: "email", message: "Enter a valid email address." }],
          },
        });
      }
      return { user: input.email };
    });
    const onError = vi.fn();
    const onSuccess = vi.fn();

    function LoginPage() {
      return (
        <main>
          <Form<LoginInput, LoginResult>
            action={action}
            initialValues={{ email: "", password: "" }}
            onError={onError}
            onSuccess={onSuccess}
          >
            {(form) => (
              <>
                <input
                  id="email"
                  name="email"
                  value={form.data.email}
                  onChange={(event) => form.setField("email", event.currentTarget.value)}
                />
                <FieldError errors={form.errors} name="email" id="email-error" />
                <button type="submit" disabled={form.processing}>Sign in</button>
                {form.wasSuccessful ? <output id="success">Signed in</output> : null}
              </>
            )}
          </Form>
        </main>
      );
    }

    installPage(pageEnvelope("login", {}));
    await act(async () => {
      await startPhoenix({ pages: { login: LoginPage } });
    });
    const firstError = new Promise<void>((resolve) => {
      onError.mockImplementationOnce(() => resolve());
    });
    await act(async () => {
      submitForm();
      await firstError;
    });

    expect(submittedSignal).toBeInstanceOf(AbortSignal);
    expect(onError).toHaveBeenCalledWith(expect.any(RustCallError), {
      email: [{ rule: "email", message: "Enter a valid email address." }],
    });
    expect(document.getElementById("email-error")?.textContent)
      .toBe("Enter a valid email address.");
    expect(document.getElementById("email-error")?.getAttribute("role")).toBe("alert");

    valid = true;
    const input = document.getElementById("email") as HTMLInputElement;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
        ?.call(input, "ada@example.test");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(document.getElementById("email-error")).toBeNull();

    const succeeded = new Promise<void>((resolve) => {
      onSuccess.mockImplementationOnce(() => resolve());
    });
    await act(async () => {
      submitForm();
      await succeeded;
    });

    expect(onSuccess).toHaveBeenCalledWith({ user: "ada@example.test" });
    expect(document.getElementById("success")?.textContent).toBe("Signed in");
  });

  it("aborts the previous action when a form is submitted again", async () => {
    interface SearchInput {
      query: string;
    }

    const signals: AbortSignal[] = [];
    const resolvers: Array<(value: string) => void> = [];
    const action = vi.fn((_input: SearchInput, options?: { signal?: AbortSignal }) => {
      if (options?.signal) signals.push(options.signal);
      return new Promise<string>((resolve, reject) => {
        resolvers.push(resolve);
        options?.signal?.addEventListener("abort", () => {
          reject(new DOMException("cancelled", "AbortError"));
        }, { once: true });
      });
    });
    const onSuccess = vi.fn();

    function SearchPage() {
      return (
        <Form<SearchInput, string>
          action={action}
          initialValues={{ query: "phoenix" }}
          onSuccess={onSuccess}
        >
          {(form) => <button type="submit" data-processing={form.processing}>Search</button>}
        </Form>
      );
    }

    installPage(pageEnvelope("search", {}));
    await act(async () => {
      await startPhoenix({ pages: { search: SearchPage } });
    });
    await act(async () => {
      submitForm();
      await Promise.resolve();
      submitForm();
      await Promise.resolve();
    });

    expect(action).toHaveBeenCalledTimes(2);
    expect(signals[0]?.aborted).toBe(true);
    expect(signals[1]?.aborted).toBe(false);
    await act(async () => {
      resolvers[1]?.("done");
      await Promise.resolve();
    });
    expect(onSuccess).toHaveBeenCalledOnce();
    expect(onSuccess).toHaveBeenCalledWith("done");
  });

  it("invalidates an in-flight submission before unmounting", async () => {
    let signal: AbortSignal | undefined;
    const onError = vi.fn();
    const action = vi.fn((_input: { value: string }, options?: { signal?: AbortSignal }) => {
      signal = options?.signal;
      return new Promise<string>((_resolve, reject) => {
        options?.signal?.addEventListener("abort", () => {
          reject(new DOMException("cancelled", "AbortError"));
        }, { once: true });
      });
    });

    function PendingPage() {
      return (
        <Form action={action} initialValues={{ value: "pending" }} onError={onError}>
          <button type="submit">Submit</button>
        </Form>
      );
    }

    installPage(pageEnvelope("pending", {}));
    await act(async () => {
      await startPhoenix({ pages: { pending: PendingPage } });
    });
    await act(async () => {
      submitForm();
      await Promise.resolve();
    });
    await act(async () => {
      stopPhoenix();
      await Promise.resolve();
    });

    expect(signal?.aborted).toBe(true);
    expect(onError).not.toHaveBeenCalled();
  });

  it("redirects after a successful submit when redirectTo is set", async () => {
    const action = vi.fn(async () => ({ ok: true }));
    const onSuccess = vi.fn();
    const members = pageEnvelope("members", {});
    const fetcher = vi.fn(async () => pageResponse(members));

    function CreatePage() {
      return (
        <Form
          action={action}
          initialValues={{ name: "Ada" }}
          redirectTo="/members"
          onSuccess={onSuccess}
        >
          <button type="submit">Create</button>
        </Form>
      );
    }

    window.history.replaceState(null, "", "/create");
    installPage(pageEnvelope("create", {}));
    vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    await act(async () => {
      await startPhoenix({
        pages: {
          create: CreatePage,
          members: () => <main id="members-main">Members</main>,
        },
        fetcher: fetcher as typeof fetch,
      });
    });

    const navigator = getPhoenixNavigator();
    expect(navigator).not.toBeNull();
    const visit = vi.spyOn(navigator!, "visit");
    const finished = nextNavigation("phoenix:navigation-success");

    await act(async () => {
      submitForm();
      await finished;
    });

    expect(onSuccess).toHaveBeenCalledWith({ ok: true });
    expect(visit).toHaveBeenCalledWith("/members", expect.objectContaining({ replace: true }));
    expect(window.location.pathname).toBe("/members");
  });

  it("does not redirect when onSuccess throws", async () => {
    const action = vi.fn(async () => ({ ok: true }));
    const onSuccess = vi.fn(() => {
      throw new Error("toast failed");
    });

    function CreatePage() {
      return (
        <Form
          action={action}
          initialValues={{ name: "Ada" }}
          redirectTo="/members"
          onSuccess={onSuccess}
        >
          <button type="submit">Create</button>
        </Form>
      );
    }

    window.history.replaceState(null, "", "/create");
    installPage(pageEnvelope("create", {}));
    await act(async () => {
      await startPhoenix({ pages: { create: CreatePage } });
    });
    const visit = vi.spyOn(getPhoenixNavigator()!, "visit");

    await act(async () => {
      submitForm();
      await Promise.resolve();
    });

    expect(onSuccess).toHaveBeenCalledOnce();
    expect(visit).not.toHaveBeenCalled();
    expect(window.location.pathname).toBe("/create");
  });

  it("skips the action when confirm is cancelled", async () => {
    const action = vi.fn(async () => ({ ok: true }));
    const confirmFn = vi.fn(() => false);
    setConfirmImplementation(confirmFn);
    const onSuccess = vi.fn();

    function CreatePage() {
      return (
        <Form
          action={action}
          initialValues={{ name: "Ada" }}
          confirm="Create this member?"
          onSuccess={onSuccess}
        >
          <button type="submit">Create</button>
        </Form>
      );
    }

    installPage(pageEnvelope("create", {}));
    await act(async () => {
      await startPhoenix({ pages: { create: CreatePage } });
    });

    await act(async () => {
      submitForm();
      await Promise.resolve();
    });

    expect(confirmFn).toHaveBeenCalledWith("Create this member?");
    expect(action).not.toHaveBeenCalled();
    expect(onSuccess).not.toHaveBeenCalled();
  });

  it("restores remember drafts and clears them after a successful submit", async () => {
    const { rememberKey, readRemembered, writeRemembered } = await import("./remember.js");
    const key = rememberKey("posts.create");
    writeRemembered(key, { title: "Draft title" });
    const action = vi.fn(async (input: { title: string }) => ({ title: input.title }));
    const onSuccess = vi.fn();

    function CreatePage() {
      return (
        <Form
          action={action}
          initialValues={{ title: "" }}
          remember="posts.create"
          onSuccess={onSuccess}
        >
          {(form) => (
            <>
              <input id="title" value={form.data.title} readOnly />
              <button type="submit">Save</button>
            </>
          )}
        </Form>
      );
    }

    installPage(pageEnvelope("create", {}));
    await act(async () => {
      await startPhoenix({ pages: { create: CreatePage } });
    });

    expect((document.getElementById("title") as HTMLInputElement).value)
      .toBe("Draft title");

    const succeeded = new Promise<void>((resolve) => {
      onSuccess.mockImplementationOnce(() => resolve());
    });
    await act(async () => {
      submitForm();
      await succeeded;
    });

    expect(action).toHaveBeenCalledWith(
      { title: "Draft title" },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(readRemembered(key)).toBeUndefined();
  });
});

function submitForm(): void {
  document.querySelector("form")?.dispatchEvent(new Event("submit", {
    bubbles: true,
    cancelable: true,
  }));
}
