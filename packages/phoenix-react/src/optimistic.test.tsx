// @vitest-environment jsdom
import { act, useEffect, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useOptimisticAction } from "./optimistic.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let container: HTMLDivElement | null = null;

afterEach(async () => {
  await act(async () => {
    root?.unmount();
  });
  root = null;
  container?.remove();
  container = null;
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

function render(ui: ReactElement): void {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root?.render(ui);
  });
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("useOptimisticAction", () => {
  it("applies onMutate immediately while the action is pending", async () => {
    const gate = deferred<{ id: string; name: string }>();
    const action = vi.fn(async (_input: { name: string }) => gate.promise);
    let api: ReturnType<typeof useOptimisticAction<{ name: string }, { id: string; name: string }, string[]>> | null = null;

    function Probe(): ReactElement {
      const state = useOptimisticAction(action, {
        initialData: [] as string[],
        onMutate: (input, current) => [...current, input.name],
      });
      useEffect(() => {
        api = state;
      });
      return (
        <div>
          <span data-pending={String(state.pending)} />
          <ul>
            {state.data.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
      );
    }

    render(<Probe />);
    expect(api).not.toBeNull();

    let resultPromise!: Promise<{ id: string; name: string }>;
    await act(async () => {
      resultPromise = api!.run({ name: "Ada" });
    });

    expect(action).toHaveBeenCalledWith({ name: "Ada" }, expect.objectContaining({
      signal: expect.any(AbortSignal),
    }));
    expect(container?.querySelector("[data-pending]")?.getAttribute("data-pending"))
      .toBe("true");
    expect(container?.textContent).toContain("Ada");
    expect(api!.data).toEqual(["Ada"]);

    await act(async () => {
      gate.resolve({ id: "1", name: "Ada" });
      await resultPromise;
    });

    expect(api!.pending).toBe(false);
    expect(api!.data).toEqual(["Ada"]);
    expect(api!.error).toBeNull();
  });

  it("rolls back to the snapshot when the action fails", async () => {
    const action = vi.fn(async (_input: { name: string }) => {
      throw new Error("boom");
    });
    let api: ReturnType<typeof useOptimisticAction<{ name: string }, void, string[]>> | null = null;

    function Probe(): ReactElement {
      const state = useOptimisticAction(action, {
        initialData: ["existing"] as string[],
        onMutate: (input, current) => [...current, input.name],
      });
      useEffect(() => {
        api = state;
      });
      return <pre>{JSON.stringify(state.data)}</pre>;
    }

    render(<Probe />);

    await act(async () => {
      await expect(api!.run({ name: "temp" })).rejects.toThrow("boom");
    });

    expect(api!.data).toEqual(["existing"]);
    expect(api!.error).toEqual(expect.objectContaining({ message: "boom" }));
    expect(api!.pending).toBe(false);
  });

  it("replaces data with onSuccess return value", async () => {
    const action = vi.fn(async (input: { name: string }) => ({
      id: "42",
      name: input.name,
    }));
    let api: ReturnType<
      typeof useOptimisticAction<
        { name: string },
        { id: string; name: string },
        Array<{ id: string; name: string }>
      >
    > | null = null;

    function Probe(): ReactElement {
      const state = useOptimisticAction(action, {
        initialData: [] as Array<{ id: string; name: string }>,
        onMutate: (input, current) => [...current, { id: "temp", name: input.name }],
        onSuccess: (output, _input, current) => [
          ...current.filter((item) => item.id !== "temp"),
          output,
        ],
      });
      useEffect(() => {
        api = state;
      });
      return <pre>{JSON.stringify(state.data)}</pre>;
    }

    render(<Probe />);

    await act(async () => {
      await api!.run({ name: "Ada" });
    });

    expect(api!.data).toEqual([{ id: "42", name: "Ada" }]);
    expect(api!.pending).toBe(false);
  });

  it("uses a custom onError return value instead of the snapshot", async () => {
    const action = vi.fn(async (_input: { name: string }) => {
      throw new Error("denied");
    });
    let api: ReturnType<typeof useOptimisticAction<{ name: string }, void, string[]>> | null = null;
    const onError = vi.fn((_error, _input, _rollback, current: string[]) => [
      ...current,
      "failed",
    ]);

    function Probe(): ReactElement {
      const state = useOptimisticAction(action, {
        initialData: ["seed"] as string[],
        onMutate: (input, current) => [...current, input.name],
        onError,
      });
      useEffect(() => {
        api = state;
      });
      return <pre>{JSON.stringify(state.data)}</pre>;
    }

    render(<Probe />);

    await act(async () => {
      await expect(api!.run({ name: "temp" })).rejects.toThrow("denied");
    });

    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ message: "denied" }),
      { name: "temp" },
      ["seed"],
      ["seed", "temp"],
    );
    expect(api!.data).toEqual(["seed", "temp", "failed"]);
    expect(api!.error).toEqual(expect.objectContaining({ message: "denied" }));
  });
});
