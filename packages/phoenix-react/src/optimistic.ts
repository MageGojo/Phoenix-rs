import {
  type Dispatch,
  type SetStateAction,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

import { abortError, isAbortError } from "./errors.js";

export interface OptimisticActionOptions<Input, Output, Data> {
  initialData: Data;
  onMutate: (input: Input, current: Data) => Data;
  onSuccess?: (output: Output, input: Input, current: Data) => Data;
  onError?: (error: unknown, input: Input, rollback: Data, current: Data) => Data | void;
}

export function useOptimisticAction<Input, Output, Data>(
  action: (input: Input, options?: { signal?: AbortSignal }) => Promise<Output>,
  options: OptimisticActionOptions<Input, Output, Data>,
): {
  data: Data;
  setData: Dispatch<SetStateAction<Data>>;
  error: unknown;
  pending: boolean;
  run: (input: Input) => Promise<Output>;
  reset: () => void;
} {
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const initialDataRef = useRef(options.initialData);
  const [data, setDataState] = useState<Data>(options.initialData);
  const dataRef = useRef(data);
  dataRef.current = data;

  const [error, setError] = useState<unknown>(null);
  const [pending, setPending] = useState(false);
  const controllerRef = useRef<AbortController | null>(null);
  const runIdRef = useRef(0);

  useEffect(() => () => {
    runIdRef.current += 1;
    controllerRef.current?.abort();
    controllerRef.current = null;
  }, []);

  const setData = useCallback<Dispatch<SetStateAction<Data>>>((update) => {
    setDataState((current) => {
      const next = typeof update === "function"
        ? (update as (value: Data) => Data)(current)
        : update;
      dataRef.current = next;
      return next;
    });
  }, []);

  const reset = useCallback(() => {
    runIdRef.current += 1;
    controllerRef.current?.abort();
    controllerRef.current = null;
    const initial = initialDataRef.current;
    dataRef.current = initial;
    setDataState(initial);
    setError(null);
    setPending(false);
  }, []);

  const run = useCallback(async (input: Input): Promise<Output> => {
    const { onMutate, onSuccess, onError } = optionsRef.current;
    const runId = runIdRef.current + 1;
    runIdRef.current = runId;
    controllerRef.current?.abort();
    const controller = new AbortController();
    controllerRef.current = controller;

    const snapshot = dataRef.current;
    const optimistic = onMutate(input, snapshot);
    dataRef.current = optimistic;
    setDataState(optimistic);
    setPending(true);
    setError(null);

    try {
      const output = await action(input, { signal: controller.signal });
      if (runId !== runIdRef.current) throw abortError();
      const next = onSuccess
        ? onSuccess(output, input, dataRef.current)
        : dataRef.current;
      dataRef.current = next;
      setDataState(next);
      return output;
    } catch (err) {
      if (runId === runIdRef.current && !isAbortError(err)) {
        const recovered = onError?.(err, input, snapshot, dataRef.current);
        const next = recovered === undefined ? snapshot : recovered;
        dataRef.current = next;
        setDataState(next);
        setError(err);
      }
      throw err;
    } finally {
      if (runId === runIdRef.current) {
        controllerRef.current = null;
        setPending(false);
      }
    }
  }, [action]);

  return { data, setData, error, pending, run, reset };
}
