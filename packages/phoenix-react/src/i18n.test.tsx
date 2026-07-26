// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { interpolate, translate, useTranslations } from "./i18n.js";
import { PhoenixPageProvider } from "./page-state.js";
import { pageEnvelope } from "./test-utils.js";
import type { PageEnvelope } from "./protocol.js";
import type { PhoenixNavigator } from "./navigation.js";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

describe("interpolate / translate", () => {
  it("fills {name} slots and leaves unknown slots literal", () => {
    expect(interpolate("你好，{name}！", { name: "世界" })).toBe("你好，世界！");
    expect(interpolate("{a}-{b}", { a: 1 })).toBe("1-{b}");
    expect(interpolate("no slots")).toBe("no slots");
  });

  it("falls back to the key when a translation is missing, like the server", () => {
    const map = { greeting: "Hi {name}" };
    expect(translate(map, "greeting", { name: "Ada" })).toBe("Hi Ada");
    expect(translate(map, "missing")).toBe("missing");
    expect(translate(undefined, "greeting")).toBe("greeting");
  });
});

describe("useTranslations", () => {
  let root: Root | null = null;
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    act(() => root?.unmount());
    container?.remove();
    root = null;
    container = null;
  });

  function renderWith(envelope: PageEnvelope): void {
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    function Probe() {
      const { locale, t, has } = useTranslations();
      return (
        <>
          <span id="locale">{locale}</span>
          <span id="msg">{t("greeting", { name: "小明" })}</span>
          <span id="missing">{t("nope")}</span>
          <span id="has">{String(has("greeting"))}</span>
        </>
      );
    }
    act(() => {
      root?.render(
        <PhoenixPageProvider envelope={envelope} navigator={{} as PhoenixNavigator}>
          <Probe />
        </PhoenixPageProvider>,
      );
    });
  }

  it("reads locale and translations from the page envelope", () => {
    const envelope = pageEnvelope("home", {});
    envelope.locale = "zh-CN";
    envelope.translations = { greeting: "你好，{name}！" };
    renderWith(envelope);
    expect(container?.querySelector("#locale")?.textContent).toBe("zh-CN");
    expect(container?.querySelector("#msg")?.textContent).toBe("你好，小明！");
    expect(container?.querySelector("#missing")?.textContent).toBe("nope");
    expect(container?.querySelector("#has")?.textContent).toBe("true");
  });

  it("defaults locale to en and treats absent translations as empty", () => {
    const envelope = pageEnvelope("home", {});
    delete envelope.locale;
    delete envelope.translations;
    renderWith(envelope);
    expect(container?.querySelector("#locale")?.textContent).toBe("en");
    expect(container?.querySelector("#msg")?.textContent).toBe("greeting");
    expect(container?.querySelector("#has")?.textContent).toBe("false");
  });
});
