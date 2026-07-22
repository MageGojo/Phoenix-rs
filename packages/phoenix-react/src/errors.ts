export function abortError(): Error {
  if (typeof DOMException === "function") {
    return new DOMException("Phoenix request was cancelled", "AbortError");
  }
  return Object.assign(new Error("Phoenix request was cancelled"), { name: "AbortError" });
}

export function isAbortError(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "name" in error &&
    error.name === "AbortError"
  );
}
