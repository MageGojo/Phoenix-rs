import {
  createElement,
  type CSSProperties,
  type FC,
  type ReactElement,
  useEffect,
  useState,
} from "react";

import { getPhoenixNavigator } from "./navigation.js";
import { normalizePathname } from "./page-state.js";
import { readPage, type PageEnvelope } from "./protocol.js";

export interface PhoenixDevOverlayProps {
  enabled?: boolean;
  document?: Document;
}

function resolveEnabled(explicit?: boolean): boolean {
  if (explicit === true) return true;
  if (explicit === false) return false;
  try {
    if (
      typeof import.meta !== "undefined"
      && !!(import.meta as ImportMeta & { env?: { DEV?: boolean } }).env?.DEV
    ) {
      return true;
    }
  } catch {
    // ignore
  }
  try {
    if (
      typeof process !== "undefined"
      && process.env?.NODE_ENV
      && process.env.NODE_ENV !== "production"
    ) {
      return true;
    }
  } catch {
    // ignore
  }
  return false;
}

function safeReadPage(documentRef: Document): PageEnvelope | null {
  try {
    return getPhoenixNavigator(documentRef)?.page ?? readPage(documentRef);
  } catch {
    return null;
  }
}

function reverseRouteName(
  routes: Record<string, string>,
  pathname: string,
): string | null {
  const target = normalizePathname(pathname);
  for (const [name, path] of Object.entries(routes)) {
    if (normalizePathname(path) === target) return name;
  }
  return null;
}

const overlayStyle: CSSProperties = {
  position: "fixed",
  right: "8px",
  bottom: "8px",
  zIndex: 99999,
  margin: 0,
  padding: "6px 8px",
  maxWidth: "min(360px, calc(100vw - 16px))",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
  fontSize: "10px",
  lineHeight: 1.45,
  letterSpacing: "0",
  color: "rgba(230, 230, 230, 0.9)",
  background: "rgba(20, 20, 20, 0.72)",
  border: "1px solid rgba(255, 255, 255, 0.08)",
  borderRadius: "3px",
  opacity: 0.55,
  pointerEvents: "none",
  whiteSpace: "pre-wrap",
  wordBreak: "break-all",
};

export const PhoenixDevOverlay: FC<PhoenixDevOverlayProps> = (
  props = {},
): ReactElement | null => {
  const documentRef = props.document ?? document;
  const enabled = resolveEnabled(props.enabled);
  const [envelope, setEnvelope] = useState<PageEnvelope | null>(() =>
    enabled ? safeReadPage(documentRef) : null,
  );
  const [href, setHref] = useState(() =>
    documentRef.defaultView?.location.href ?? "",
  );
  const [lastVisitUrl, setLastVisitUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;

    const refresh = () => {
      setEnvelope(safeReadPage(documentRef));
      setHref(documentRef.defaultView?.location.href ?? "");
    };

    const onVisit = (event: Event) => {
      const detail = (event as CustomEvent<{ url?: string }>).detail;
      if (detail?.url) setLastVisitUrl(detail.url);
      refresh();
    };

    refresh();
    documentRef.addEventListener("phoenix:navigation-start", onVisit);
    documentRef.addEventListener("phoenix:navigation-success", onVisit);
    documentRef.addEventListener("phoenix:navigation-hard", refresh);
    documentRef.addEventListener("phoenix:navigation-finish", refresh);

    return () => {
      documentRef.removeEventListener("phoenix:navigation-start", onVisit);
      documentRef.removeEventListener("phoenix:navigation-success", onVisit);
      documentRef.removeEventListener("phoenix:navigation-hard", refresh);
      documentRef.removeEventListener("phoenix:navigation-finish", refresh);
    };
  }, [documentRef, enabled]);

  if (!enabled) return null;

  const pathname = (() => {
    try {
      return new URL(href).pathname;
    } catch {
      return href;
    }
  })();
  const routeName = envelope
    ? reverseRouteName(envelope.routes, pathname)
    : null;

  const lines = [
    `page: ${envelope?.page ?? "—"}`,
    `contract: ${envelope?.contract_hash ?? "—"}`,
    `asset: ${envelope?.asset_version ?? "—"}`,
    `url: ${href || "—"}`,
    `last: ${lastVisitUrl ?? "—"}`,
    `route: ${routeName ?? "—"}`,
  ];

  return createElement("aside", {
    "data-phoenix-dev-overlay": "",
    style: overlayStyle,
    children: lines.join("\n"),
  });
};
