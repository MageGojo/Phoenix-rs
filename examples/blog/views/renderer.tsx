import { createInterface } from "node:readline";

import type { PageEnvelope } from "@phoenix/react";
import { RENDERER_PROTOCOL, renderPage } from "@phoenix/react-ssr";

import ArticleShow from "./pages/articles/show.js";
import MembersIndex from "./pages/members/index.js";

interface RendererRequest {
  protocol: number;
  id: number;
  kind: "hello" | "render";
  envelope?: PageEnvelope;
  url?: string;
  locale?: string;
}

const pages = {
  "articles/show": ArticleShow,
  "members/index": MembersIndex,
};

const input = createInterface({ input: process.stdin, crlfDelay: Infinity });

input.on("line", (line) => {
  let request: RendererRequest | undefined;
  try {
    request = JSON.parse(line) as RendererRequest;
    if (request.protocol !== RENDERER_PROTOCOL) {
      throw new Error(`unsupported renderer protocol: ${request.protocol}`);
    }

    if (request.kind === "hello") {
      write({ protocol: RENDERER_PROTOCOL, id: request.id, ok: true });
      return;
    }
    if (request.kind !== "render" || !request.envelope) {
      throw new Error("invalid renderer request");
    }

    const result = renderPage(request.envelope, pages);
    write({
      protocol: RENDERER_PROTOCOL,
      id: request.id,
      ok: true,
      html: result.html,
      head: [],
    });
  } catch (error) {
    write({
      protocol: RENDERER_PROTOCOL,
      id: request?.id ?? 0,
      ok: false,
      error: error instanceof Error ? error.message : "renderer failed",
    });
  }
});

function write(response: object): void {
  process.stdout.write(`${JSON.stringify(response)}\n`);
}
