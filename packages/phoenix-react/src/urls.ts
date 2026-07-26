import { readPage } from "./protocol.js";

export type RouteParamValue = string | number;
export type RouteParams = Record<string, RouteParamValue>;

export interface RouteUrlOptions {
  /** Extra key/values appended as a query string; null/undefined entries are skipped. */
  query?: Record<string, RouteParamValue | null | undefined>;
}

export type RouteUrlBuilder<Params extends RouteParams | undefined = undefined> =
  (Params extends undefined ? (options?: RouteUrlOptions) => string
    : (params: Params, options?: RouteUrlOptions) => string)
  & { readonly routeName: string };

let manifest: Record<string, string> | null = null;

/**
 * Install the name → path-pattern table used to resolve named route URLs.
 * The navigator registers every rendered envelope's `routes` automatically;
 * the SSR renderer registers per render. Manual calls are only needed in
 * tests or custom environments.
 */
export function registerRouteManifest(routes: Record<string, string>): void {
  manifest = routes;
}

export function resetRouteManifest(): void {
  manifest = null;
}

/**
 * Build the URL for a Rust named route, mirroring `Router::url` semantics:
 * `{param}` segments are replaced with percent-encoded values and a missing
 * parameter is an error. Extra values can be appended via `options.query`.
 */
export function urlFor(
  routeName: string,
  params: RouteParams = {},
  options: RouteUrlOptions = {},
): string {
  const pattern = routePattern(routeName);
  let output = "";
  for (const segment of pattern.split("/")) {
    if (segment.length === 0) continue;
    output += "/";
    const parameter = segment.startsWith("{") && segment.endsWith("}")
      ? segment.slice(1, -1)
      : null;
    if (parameter === null) {
      output += segment;
      continue;
    }
    const value = params[parameter];
    if (value === undefined) {
      throw new Error(`Phoenix route "${routeName}" is missing parameter "${parameter}"`);
    }
    output += encodePathSegment(String(value));
  }
  if (output.length === 0) output = "/";
  return output + queryString(options.query);
}

export function createRouteUrl(routeName: string): RouteUrlBuilder;
export function createRouteUrl<Params extends RouteParams>(
  routeName: string,
): RouteUrlBuilder<Params>;
export function createRouteUrl(routeName: string): RouteUrlBuilder<RouteParams> {
  const build = (first?: RouteParams | RouteUrlOptions, second?: RouteUrlOptions): string => {
    if (second !== undefined) return urlFor(routeName, first as RouteParams, second);
    const parameterized = /\{[^}]*\}/.test(routePattern(routeName));
    return parameterized
      ? urlFor(routeName, (first as RouteParams | undefined) ?? {})
      : urlFor(routeName, {}, (first as RouteUrlOptions | undefined) ?? {});
  };
  return Object.assign(build, { routeName }) as RouteUrlBuilder<RouteParams>;
}

function routePattern(routeName: string): string {
  const routes = manifest
    ?? (typeof document === "undefined" ? null : readPage(document).routes);
  if (!routes) {
    throw new Error(
      `Phoenix route manifest is unavailable while resolving: ${routeName}`,
    );
  }
  const pattern = routes[routeName];
  if (!pattern) throw new Error(`Phoenix named route is not available: ${routeName}`);
  return pattern;
}

/** Encode exactly RFC 3986 unreserved characters, matching Rust's path segment set. */
function encodePathSegment(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

function queryString(query: RouteUrlOptions["query"]): string {
  if (!query) return "";
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === null || value === undefined) continue;
    search.append(key, String(value));
  }
  const output = search.toString();
  return output.length === 0 ? "" : `?${output}`;
}
