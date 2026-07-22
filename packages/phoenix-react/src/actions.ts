import { isRecord, readPage, type PageEnvelope } from "./protocol.js";

export interface RustCallOptions {
  signal?: AbortSignal;
}

export interface ValidationError {
  rule?: string;
  message: string;
}

export type FieldName<Input extends object> = Extract<keyof Input, string>;
export type FieldErrors<Input extends object> = Partial<
  Record<FieldName<Input>, readonly ValidationError[]>
> & Record<string, readonly ValidationError[] | undefined>;

export class RustCallError<Input extends object = Record<string, unknown>> extends Error {
  public readonly fieldErrors: FieldErrors<Input>;

  constructor(
    public readonly status: number,
    message: string,
    public readonly details: unknown,
  ) {
    super(message);
    this.name = "RustCallError";
    this.fieldErrors = status === 422 ? extractFieldErrors<Input>(details) : {};
  }
}

export async function callRust<Output, Input = unknown>(
  routeName: string,
  input: Input,
  fetcher: typeof fetch = fetch,
  options: RustCallOptions = {},
): Promise<Output> {
  const envelope = readPage(document);
  const url = rustRoute(routeName, envelope);
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "Accept": "application/json",
  };
  if (envelope.csrf_token) headers["X-CSRF-Token"] = envelope.csrf_token;
  const request: RequestInit = {
    method: "POST",
    headers,
    body: JSON.stringify(input),
  };
  if (options.signal) request.signal = options.signal;
  const response = await fetcher(url, request);
  const body = await response.json().catch(() => null) as unknown;
  if (!response.ok) {
    const message = isRecord(body) && typeof body.message === "string"
      ? body.message
      : `Rust action failed with ${response.status}`;
    throw new RustCallError(response.status, message, body);
  }
  return body as Output;
}

export type RustAction<Input, Output> = ((
  input: Input,
  options?: RustCallOptions,
) => Promise<Output>) & {
  readonly routeName: string;
};

export function createRustAction<Input, Output>(
  routeName: string,
): RustAction<Input, Output> {
  const action = (input: Input, options?: RustCallOptions) => (
    callRust<Output, Input>(routeName, input, fetch, options)
  );
  return Object.assign(action, { routeName });
}

export function fieldErrorsFrom<Input extends object>(error: unknown): FieldErrors<Input> {
  if (error instanceof RustCallError) {
    return error.fieldErrors as FieldErrors<Input>;
  }
  return {};
}

function rustRoute(
  routeName: string,
  envelope: PageEnvelope = readPage(document),
): string {
  const route = envelope.routes[routeName];
  if (!route) throw new Error(`Phoenix named route is not available: ${routeName}`);
  return route;
}

function extractFieldErrors<Input extends object>(details: unknown): FieldErrors<Input> {
  if (!isRecord(details) || !isRecord(details.errors)) return {};
  const output: Record<string, ValidationError[]> = {};
  for (const [field, rawErrors] of Object.entries(details.errors)) {
    const items = Array.isArray(rawErrors) ? rawErrors : [rawErrors];
    const errors = items.flatMap((item): ValidationError[] => {
      if (typeof item === "string") return [{ message: item }];
      if (!isRecord(item) || typeof item.message !== "string") return [];
      return [{
        ...(typeof item.rule === "string" ? { rule: item.rule } : {}),
        message: item.message,
      }];
    });
    if (errors.length > 0) output[field] = errors;
  }
  return output as FieldErrors<Input>;
}
