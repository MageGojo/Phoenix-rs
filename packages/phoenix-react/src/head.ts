import type { PageHead } from "./protocol.js";

export function updatePageHead(documentRef: Document, head: PageHead = {}): void {
  setHeadText(documentRef, "title[data-phoenix-head]", "title", head.title);
  setHeadAttribute(
    documentRef,
    'meta[data-phoenix-head][name="description"]',
    "meta",
    head.description,
    { name: "description" },
    "content",
  );
  setHeadAttribute(
    documentRef,
    'link[data-phoenix-head][rel="canonical"]',
    "link",
    head.canonical,
    { rel: "canonical" },
    "href",
  );
  setHeadAttribute(
    documentRef,
    'meta[data-phoenix-head][name="robots"]',
    "meta",
    head.robots,
    { name: "robots" },
    "content",
  );
  const openGraph = head.open_graph;
  setHeadAttribute(
    documentRef,
    'meta[data-phoenix-head][property="og:title"]',
    "meta",
    openGraph?.title,
    { property: "og:title" },
    "content",
  );
  setHeadAttribute(
    documentRef,
    'meta[data-phoenix-head][property="og:description"]',
    "meta",
    openGraph?.description,
    { property: "og:description" },
    "content",
  );
  setHeadAttribute(
    documentRef,
    'meta[data-phoenix-head][property="og:image"]',
    "meta",
    openGraph?.image,
    { property: "og:image" },
    "content",
  );
  setHeadAttribute(
    documentRef,
    'meta[data-phoenix-head][property="og:type"]',
    "meta",
    openGraph?.kind,
    { property: "og:type" },
    "content",
  );
}

function setHeadText(
  documentRef: Document,
  selector: string,
  tag: "title",
  value: string | null | undefined,
): void {
  let element = documentRef.head.querySelector(selector);
  if (value === null || value === undefined) {
    element?.remove();
    return;
  }
  if (!element) {
    element = documentRef.createElement(tag);
    documentRef.head.append(element);
  }
  element.setAttribute("data-phoenix-head", "");
  element.textContent = value;
}

function setHeadAttribute(
  documentRef: Document,
  selector: string,
  tag: "meta" | "link",
  value: string | null | undefined,
  fixedAttributes: Record<string, string>,
  valueAttribute: "content" | "href",
): void {
  let element = documentRef.head.querySelector(selector);
  if (value === null || value === undefined) {
    element?.remove();
    return;
  }
  if (!element) {
    element = documentRef.createElement(tag);
    documentRef.head.append(element);
  }
  element.setAttribute("data-phoenix-head", "");
  for (const [name, fixedValue] of Object.entries(fixedAttributes)) {
    element.setAttribute(name, fixedValue);
  }
  element.setAttribute(valueAttribute, value);
}
