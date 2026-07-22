// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  FieldError,
  Form,
  RustCallError,
  startPhoenix,
  stopPhoenix,
} from "./index.js";
import { installPage, pageEnvelope } from "./test-utils.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

afterEach(async () => {
  await act(async () => stopPhoenix());
  vi.restoreAllMocks();
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
});

function submitForm(): void {
  document.querySelector("form")?.dispatchEvent(new Event("submit", {
    bubbles: true,
    cancelable: true,
  }));
}
