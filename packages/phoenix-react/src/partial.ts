import { isRecord, type PageEnvelope } from "./protocol.js";

export interface PartialReloadOptions {
  only?: string[];
  except?: string[];
}

/**
 * Merge a partial page response into the current envelope.
 *
 * Top-level fields (shared/flash/errors/head/csrf/routes/page/…) always come from
 * `next`. When `only`/`except` is set and both sides have plain-object props,
 * props are merged onto `current.props`; otherwise `next` is returned as-is.
 */
export function mergePageEnvelope(
  current: PageEnvelope,
  next: PageEnvelope,
  options: PartialReloadOptions,
): PageEnvelope {
  if (!options.only && !options.except) {
    return next;
  }

  if (!isRecord(current.props) || !isRecord(next.props)) {
    return next;
  }

  const currentProps = current.props;
  const nextProps = next.props;
  let props: Record<string, unknown>;

  if (options.only) {
    props = { ...currentProps };
    for (const key of options.only) {
      props[key] = nextProps[key];
    }
  } else {
    props = { ...currentProps };
    const except = new Set(options.except ?? []);
    for (const [key, value] of Object.entries(nextProps)) {
      if (!except.has(key)) {
        props[key] = value;
      }
    }
  }

  return { ...next, props };
}

/** Build partial-reload request headers from VisitOptions. */
export function partialReloadHeaders(
  options: PartialReloadOptions,
): Record<string, string> {
  const headers: Record<string, string> = {};
  if (options.only?.length) {
    headers["X-Phoenix-Only"] = options.only.join(",");
  }
  if (options.except?.length) {
    headers["X-Phoenix-Except"] = options.except.join(",");
  }
  return headers;
}
