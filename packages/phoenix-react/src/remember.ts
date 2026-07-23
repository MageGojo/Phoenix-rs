import {
  type Dispatch,
  type SetStateAction,
  useEffect,
  useRef,
} from "react";

const REMEMBER_PREFIX = "phoenix:remember:";
const DEBOUNCE_MS = 300;

type ClearListener = () => void;
const clearListeners = new Map<string, Set<ClearListener>>();

function subscribeClear(key: string, listener: ClearListener): () => void {
  let set = clearListeners.get(key);
  if (!set) {
    set = new Set();
    clearListeners.set(key, set);
  }
  set.add(listener);
  return () => {
    set!.delete(listener);
    if (set!.size === 0) clearListeners.delete(key);
  };
}

export function rememberKey(name: string): string {
  return `${REMEMBER_PREFIX}${name}`;
}

export function readRemembered<T>(key: string): T | undefined {
  if (!key || typeof sessionStorage === "undefined") return undefined;
  try {
    const raw = sessionStorage.getItem(key);
    if (raw == null) return undefined;
    return JSON.parse(raw) as T;
  } catch {
    return undefined;
  }
}

export function writeRemembered(key: string, value: unknown): void {
  if (!key || typeof sessionStorage === "undefined") return;
  try {
    sessionStorage.setItem(key, JSON.stringify(value));
  } catch {
    // quota / private mode — ignore
  }
}

export function clearRemembered(key: string): void {
  if (!key || typeof sessionStorage === "undefined") return;
  try {
    sessionStorage.removeItem(key);
  } catch {
    // ignore
  }
  clearListeners.get(key)?.forEach((listener) => listener());
}

/**
 * Persist `data` to sessionStorage under `key`, restoring on mount.
 * No-ops when `key` is empty. Debounces writes by 300ms; flushes dirty state on unmount.
 * `clearRemembered` cancels pending dirty writes for the same key.
 */
export function useRemember<T extends object>(
  key: string,
  data: T,
  setData: Dispatch<SetStateAction<T>>,
): void {
  const dataRef = useRef(data);
  dataRef.current = data;
  const enabledRef = useRef(false);
  const dirtyRef = useRef(false);

  useEffect(() => {
    if (!key) return;
    enabledRef.current = false;
    dirtyRef.current = false;
    const remembered = readRemembered<T>(key);
    if (remembered !== undefined) {
      setData(remembered);
    }
    const enableTimer = setTimeout(() => {
      enabledRef.current = true;
    }, 0);
    return () => clearTimeout(enableTimer);
  }, [key, setData]);

  useEffect(() => {
    if (!key) return;
    return subscribeClear(key, () => {
      dirtyRef.current = false;
    });
  }, [key]);

  useEffect(() => {
    if (!key || !enabledRef.current) return;
    dirtyRef.current = true;
    const timer = setTimeout(() => {
      if (!dirtyRef.current) return;
      writeRemembered(key, dataRef.current);
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [key, data]);

  useEffect(() => {
    if (!key) return;
    return () => {
      if (dirtyRef.current) writeRemembered(key, dataRef.current);
    };
  }, [key]);
}
