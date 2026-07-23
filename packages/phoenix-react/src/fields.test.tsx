// @vitest-environment jsdom
import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fieldProps, useForm } from "./index.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | undefined;
let container: HTMLDivElement | undefined;

afterEach(() => {
  if (root && container) {
    act(() => {
      root!.unmount();
    });
  }
  root = undefined;
  container?.remove();
  container = undefined;
  vi.restoreAllMocks();
});

describe("contract-driven form.field", () => {
  it("builds input props and routes onChange through setField", () => {
    const setField = vi.fn();
    const form = {
      data: { email: "ada@example.test", active: false, age: 0 },
      setField,
      errors: {
        email: [{ rule: "email", message: "Invalid" }],
      },
    };

    const email = fieldProps(form, "email", {
      name: "email",
      type: "string",
      required: true,
    });
    expect(email).toMatchObject({
      name: "email",
      value: "ada@example.test",
      required: true,
      "aria-invalid": true,
      "data-phoenix-field": "email",
    });
    email.onChange({ currentTarget: { value: "grace@example.test", type: "email" } });
    expect(setField).toHaveBeenCalledWith("email", "grace@example.test");

    const active = fieldProps(form, "active");
    expect(active.required).toBeUndefined();
    expect(active["aria-invalid"]).toBeUndefined();
    active.onChange({ currentTarget: { value: "on", type: "checkbox", checked: true } });
    expect(setField).toHaveBeenCalledWith("active", true);

    const age = fieldProps(form, "age", { name: "age", type: "number", required: false });
    age.onChange({ currentTarget: { value: "42", type: "number" } });
    expect(setField).toHaveBeenCalledWith("age", 42);
  });

  it("exposes form.field from useForm with optional field map", async () => {
    interface CreateInput {
      name: string;
      count: number;
    }
    const action = vi.fn(async (input: CreateInput) => input);
    let latest: ReturnType<typeof useForm<CreateInput, CreateInput>> | undefined;

    function Probe() {
      const form = useForm(action, { name: "", count: 0 }, {
        fields: {
          name: { name: "name", type: "string", required: true },
          count: { name: "count", type: "number", required: false },
        },
      });
      useEffect(() => {
        latest = form;
      });
      return createElement("input", form.field("name"));
    }

    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(createElement(Probe));
    });

    const props = latest!.field("name");
    expect(props).toMatchObject({
      name: "name",
      value: "",
      required: true,
      "data-phoenix-field": "name",
    });
    expect(container!.querySelector("input")?.getAttribute("data-phoenix-field")).toBe("name");

    await act(async () => {
      props.onChange({ currentTarget: { value: "Ada", type: "text" } });
    });
    expect(latest!.data.name).toBe("Ada");

    const count = latest!.field("count");
    await act(async () => {
      count.onChange({ currentTarget: { value: "3", type: "number" } });
    });
    expect(latest!.data.count).toBe(3);
  });
});
