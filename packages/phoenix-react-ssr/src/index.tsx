import { createElement, type ComponentType } from "react";
import { renderToString } from "react-dom/server";

import type { ComponentRegistry, PageEnvelope } from "@phoenix/react";

export interface RenderResult {
  html: string;
  mode: PageEnvelope["render_mode"];
}

export const RENDERER_PROTOCOL = 1;

export function renderPage(
  envelope: PageEnvelope,
  pages: ComponentRegistry,
): RenderResult {
  if (envelope.render_mode === "spa") {
    return { html: "", mode: "spa" };
  }
  const Page = pages[envelope.page] as ComponentType<any> | undefined;
  if (!Page) {
    throw new Error(`Phoenix page is not registered: ${envelope.page}`);
  }
  const props = isRecord(envelope.props)
    ? { ...envelope.props, phoenix: envelope.shared }
    : { value: envelope.props, phoenix: envelope.shared };
  return {
    html: renderToString(createElement(Page, props)),
    mode: envelope.render_mode,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
