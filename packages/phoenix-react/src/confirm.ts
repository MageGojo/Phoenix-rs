export type ConfirmImplementation = (message: string) => boolean;

let confirmImpl: ConfirmImplementation = defaultConfirm;

function defaultConfirm(message: string): boolean {
  if (typeof window !== "undefined" && typeof window.confirm === "function") {
    return window.confirm(message);
  }
  return true;
}

/** Prompt the user; returns false when they cancel. */
export function confirmAction(message: string): boolean {
  return confirmImpl(message);
}

/** Override confirm for tests (or custom UI). */
export function setConfirmImplementation(fn: ConfirmImplementation): void {
  confirmImpl = fn;
}

/** Restore the default `window.confirm` implementation. */
export function resetConfirmImplementation(): void {
  confirmImpl = defaultConfirm;
}
