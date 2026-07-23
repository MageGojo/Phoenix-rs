import { navigate, type VisitOptions } from "./navigation.js";
import type { PageEnvelope } from "./protocol.js";

export function redirect(
  url: string | URL,
  options?: VisitOptions,
  documentRef?: Document,
): Promise<PageEnvelope> {
  return navigate(url, { replace: true, ...options }, documentRef);
}
