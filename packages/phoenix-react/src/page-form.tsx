import {
  createElement,
  type FormEvent,
  type FormHTMLAttributes,
  type ReactElement,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";

import {
  fieldErrorsFrom,
  type FieldErrors,
  type FieldName,
} from "./actions.js";
import { confirmAction } from "./confirm.js";
import { abortError, isAbortError } from "./errors.js";
import {
  fieldProps,
  type FieldMap,
} from "./fields.js";
import {
  getPhoenixNavigator,
  type VisitMethod,
} from "./navigation.js";
import type { PageEnvelope } from "./protocol.js";
import type { FormState } from "./forms.js";

export interface PageFormOptions<Input extends object> {
  action: string;
  method?: VisitMethod;
  initialValues: Input;
  replace?: boolean;
  preserveScroll?: boolean;
  preserveFocus?: boolean;
  confirm?: string;
  fields?: FieldMap<Input>;
}

export type PageFormState<Input extends object> = FormState<Input, PageEnvelope>;

export function usePageForm<Input extends object>(
  options: PageFormOptions<Input>,
): PageFormState<Input> {
  const {
    action,
    method = "post",
    initialValues,
    replace,
    preserveScroll,
    preserveFocus,
    confirm,
    fields,
  } = options;
  const initialValuesRef = useRef(initialValues);
  const fieldsRef = useRef(fields);
  fieldsRef.current = fields;
  const optionsRef = useRef({
    action,
    method,
    replace,
    preserveScroll,
    preserveFocus,
    confirm,
  });
  optionsRef.current = {
    action,
    method,
    replace,
    preserveScroll,
    preserveFocus,
    confirm,
  };

  const [data, setData] = useState<Input>(initialValues);
  const [errors, setErrors] = useState<FieldErrors<Input>>({});
  const [processing, setProcessing] = useState(false);
  const [wasSuccessful, setWasSuccessful] = useState(false);
  const [result, setResult] = useState<PageEnvelope>();
  const [failure, setFailure] = useState<unknown>(null);
  const controllerRef = useRef<AbortController | null>(null);
  const submissionRef = useRef(0);

  useEffect(() => () => {
    submissionRef.current += 1;
    controllerRef.current?.abort();
    controllerRef.current = null;
  }, []);

  const clearErrors = useCallback((...fields: FieldName<Input>[]) => {
    if (fields.length === 0) {
      setErrors({});
      return;
    }
    setErrors((current) => {
      const next = { ...current };
      for (const field of fields) delete next[field];
      return next;
    });
  }, []);

  const setField = useCallback(<Field extends FieldName<Input>>(
    field: Field,
    value: Input[Field],
  ) => {
    setData((current) => ({ ...current, [field]: value }));
    clearErrors(field);
  }, [clearErrors]);

  const cancel = useCallback(() => {
    submissionRef.current += 1;
    controllerRef.current?.abort();
    controllerRef.current = null;
    setProcessing(false);
  }, []);

  const reset = useCallback(() => {
    setData(initialValuesRef.current);
    setErrors({});
    setFailure(null);
    setWasSuccessful(false);
    setResult(undefined);
  }, []);

  const submit = useCallback(async (): Promise<PageEnvelope> => {
    const current = optionsRef.current;
    if (current.confirm && !confirmAction(current.confirm)) {
      return Promise.reject(abortError());
    }

    const navigator = getPhoenixNavigator();
    if (!navigator) {
      throw new Error("Phoenix has not been started for this document");
    }

    const submission = submissionRef.current + 1;
    submissionRef.current = submission;
    controllerRef.current?.abort();
    const controller = new AbortController();
    controllerRef.current = controller;
    setProcessing(true);
    setWasSuccessful(false);
    setFailure(null);
    setErrors({});

    try {
      const envelope = await navigator.visit(current.action, {
        method: current.method,
        data: data as Record<string, unknown>,
        replace: current.replace,
        preserveScroll: current.preserveScroll,
        preserveFocus: current.preserveFocus,
        signal: controller.signal,
      });
      // Visit success may remount the tree and bump submissionRef via cleanup;
      // still surface the envelope to onSuccess without treating it as an abort.
      if (submission === submissionRef.current) {
        setResult(envelope);
        setWasSuccessful(true);
      }
      return envelope;
    } catch (error) {
      if (submission === submissionRef.current && !isAbortError(error)) {
        setErrors(fieldErrorsFrom<Input>(error));
        setFailure(error);
      }
      throw error;
    } finally {
      if (submission === submissionRef.current) {
        controllerRef.current = null;
        setProcessing(false);
      }
    }
  }, [data]);

  return {
    data,
    setData: setData as Dispatch<SetStateAction<Input>>,
    setField,
    field: (name) => fieldProps({ data, setField, errors }, name, fieldsRef.current?.[name]),
    errors,
    error: (field) => errors[field]?.[0]?.message,
    processing,
    wasSuccessful,
    result,
    failure,
    submit,
    reset,
    clearErrors,
    cancel,
  };
}

export interface PageFormProps<Input extends object> extends Omit<
  FormHTMLAttributes<HTMLFormElement>,
  "action" | "children" | "onError" | "onSubmit" | "method"
> {
  action: string;
  method?: VisitMethod;
  initialValues: Input;
  replace?: boolean;
  preserveScroll?: boolean;
  preserveFocus?: boolean;
  confirm?: string;
  fields?: FieldMap<Input>;
  children: ReactNode | ((form: PageFormState<Input>) => ReactNode);
  onSuccess?: (envelope: PageEnvelope) => void;
  onError?: (error: unknown, errors: FieldErrors<Input>) => void;
}

export function PageForm<Input extends object>({
  action,
  method = "post",
  initialValues,
  replace,
  preserveScroll,
  preserveFocus,
  confirm,
  fields,
  children,
  onSuccess,
  onError,
  ...props
}: PageFormProps<Input>): ReactElement {
  const form = usePageForm<Input>({
    action,
    method,
    initialValues,
    replace,
    preserveScroll,
    preserveFocus,
    confirm,
    fields,
  });
  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const envelope = await form.submit();
      onSuccess?.(envelope);
    } catch (error) {
      if (!isAbortError(error)) onError?.(error, fieldErrorsFrom<Input>(error));
    }
  };
  const content = typeof children === "function" ? children(form) : children;
  return createElement("form", { ...props, onSubmit: handleSubmit }, content);
}
