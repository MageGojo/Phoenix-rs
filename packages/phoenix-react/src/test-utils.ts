import type { PageEnvelope } from "./protocol.js";

export function pageEnvelope<Props>(
  page: string,
  props: Props,
  head: PageEnvelope["head"] = {},
): PageEnvelope<Props> {
  return {
    protocol: 1,
    render_mode: "spa",
    page,
    props,
    shared: {},
    errors: {},
    flash: {},
    contract_hash: null,
    asset_version: null,
    request_id: null,
    head,
    csrf_token: null,
    routes: {},
    islands: [],
  };
}

export function pageResponse(envelope: PageEnvelope): Response {
  return new Response(JSON.stringify(envelope), {
    headers: {
      "content-type": "application/vnd.phoenix.page+json",
      "x-phoenix-encrypted": "0",
    },
  });
}

export function installPage(envelope: PageEnvelope, serverHtml = ""): void {
  document.body.innerHTML = [
    `<div id="phoenix-root">${serverHtml}</div>`,
    `<script id="phoenix-page" type="application/json">${JSON.stringify(envelope)}</script>`,
  ].join("");
}

export function readInstalledPage(): PageEnvelope {
  return JSON.parse(document.getElementById("phoenix-page")?.textContent ?? "") as PageEnvelope;
}

export function nextNavigation(name: string): Promise<void> {
  return new Promise((resolve) => {
    document.addEventListener(name, () => resolve(), { once: true });
  });
}
