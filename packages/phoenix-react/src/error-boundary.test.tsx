// @vitest-environment jsdom
import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  DefaultErrorFallback,
  PhoenixErrorBoundary,
} from "./error-boundary.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root;
let container: HTMLDivElement;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  document.body.innerHTML = "";
});

describe("PhoenixErrorBoundary", () => {
  it("catches render throws and resets via DefaultErrorFallback", async () => {
    let shouldThrow = true;
    function Boom(): ReactElement {
      if (shouldThrow) throw new Error("boom");
      return <main id="recovered">ok</main>;
    }

    await act(async () => {
      root.render(
        <PhoenixErrorBoundary>
          <Boom />
        </PhoenixErrorBoundary>,
      );
    });

    expect(container.querySelector("[data-phoenix-error-boundary]")).not.toBeNull();
    expect(container.textContent).toContain("boom");

    shouldThrow = false;
    await act(async () => {
      container.querySelector("button")?.dispatchEvent(new MouseEvent("click", {
        bubbles: true,
        cancelable: true,
      }));
    });

    expect(container.querySelector("#recovered")?.textContent).toBe("ok");
    expect(container.querySelector("[data-phoenix-error-boundary]")).toBeNull();
  });

  it("uses a custom errorFallback", async () => {
    function Boom(): ReactElement {
      throw new Error("custom-boom");
    }

    await act(async () => {
      root.render(
        <PhoenixErrorBoundary fallback={({ error, reset }) => (
          <div data-custom-fallback="">
            <span>{error.message}</span>
            <button type="button" onClick={reset}>reset</button>
          </div>
        )}
        >
          <Boom />
        </PhoenixErrorBoundary>,
      );
    });

    expect(container.querySelector("[data-custom-fallback]")?.textContent)
      .toContain("custom-boom");
    expect(DefaultErrorFallback).toBeTypeOf("function");
  });
});
