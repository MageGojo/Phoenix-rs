import { RustCallError, type RustCallOptions } from "./actions.js";
import { abortError } from "./errors.js";
import { isRecord, readPage } from "./protocol.js";
import { urlFor } from "./urls.js";

export interface UploadOptions extends RustCallOptions {
  /** Called with 0–1 as the request body uploads. */
  onUploadProgress?: (progress: number) => void;
}

/** True when a value is a file-ish payload that requires multipart encoding. */
export function isFileValue(value: unknown): boolean {
  if (typeof Blob !== "undefined" && value instanceof Blob) return true;
  if (typeof FileList !== "undefined" && value instanceof FileList) return true;
  return Array.isArray(value) && value.length > 0 && value.every(
    (item) => typeof Blob !== "undefined" && item instanceof Blob,
  );
}

/** True when any top-level field of `data` holds a File/Blob/FileList. */
export function hasFileValue(data: object): boolean {
  return Object.values(data).some(isFileValue);
}

/**
 * Serialize form data for the Rust `Multipart<T>` extractor:
 * files become file parts (repeated name for lists), scalars become plain
 * text fields, nested objects/arrays are JSON-encoded text fields, and
 * null/undefined entries are skipped.
 */
export function toFormData(data: Record<string, unknown>): FormData {
  const body = new FormData();
  for (const [name, value] of Object.entries(data)) {
    if (value === null || value === undefined) continue;
    if (typeof Blob !== "undefined" && value instanceof Blob) {
      body.append(name, value);
      continue;
    }
    if (typeof FileList !== "undefined" && value instanceof FileList) {
      for (const file of Array.from(value)) body.append(name, file);
      continue;
    }
    if (Array.isArray(value) && isFileValue(value)) {
      for (const file of value as Blob[]) body.append(name, file);
      continue;
    }
    if (typeof value === "object") {
      body.append(name, JSON.stringify(value));
      continue;
    }
    body.append(name, String(value));
  }
  return body;
}

/**
 * POST multipart form data to a Rust named route, with upload progress.
 * The response contract matches `callRust`: JSON output, 422 carries field
 * errors via `RustCallError`. Uses XMLHttpRequest because `fetch` cannot
 * report upload progress.
 */
export function uploadRust<Output>(
  routeName: string,
  data: Record<string, unknown>,
  options: UploadOptions = {},
): Promise<Output> {
  const url = urlFor(routeName);
  const csrf = readPage(document).csrf_token;
  return new Promise<Output>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", url);
    xhr.responseType = "text";
    xhr.setRequestHeader("Accept", "application/json");
    if (csrf) xhr.setRequestHeader("X-CSRF-Token", csrf);
    xhr.upload.addEventListener("progress", (event) => {
      if (event.lengthComputable && event.total > 0) {
        options.onUploadProgress?.(event.loaded / event.total);
      }
    });
    xhr.addEventListener("load", () => {
      let body: unknown = null;
      try {
        body = JSON.parse(xhr.responseText) as unknown;
      } catch {
        body = null;
      }
      if (xhr.status >= 200 && xhr.status < 300) {
        options.onUploadProgress?.(1);
        resolve(body as Output);
        return;
      }
      const message = isRecord(body) && typeof body.message === "string"
        ? body.message
        : `Rust action failed with ${xhr.status}`;
      reject(new RustCallError(xhr.status, message, body));
    });
    xhr.addEventListener("error", () => {
      reject(new Error(`Phoenix upload failed for route: ${routeName}`));
    });
    xhr.addEventListener("abort", () => reject(abortError()));
    if (options.signal) {
      if (options.signal.aborted) {
        reject(abortError());
        return;
      }
      options.signal.addEventListener("abort", () => xhr.abort(), { once: true });
    }
    xhr.send(toFormData(data));
  });
}
