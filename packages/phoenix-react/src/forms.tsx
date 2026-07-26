import {
  createElement,
  type Dispatch,
  type FormEvent,
  type FormHTMLAttributes,
  type HTMLAttributes,
  type ReactElement,
  type ReactNode,
  type SetStateAction,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

import {
  fieldErrorsFrom,
  type FieldErrors,
  type FieldName,
  type RustAction,
  type RustCallOptions,
} from "./actions.js";
import { confirmAction } from "./confirm.js";
import { abortError, isAbortError } from "./errors.js";
import {
  fieldProps,
  type FieldMap,
  type FieldProps,
} from "./fields.js";
import { redirect } from "./redirect.js";
import { hasFileValue, uploadRust } from "./uploads.js";
import {
  clearRemembered,
  rememberKey,
  useRemember,
} from "./remember.js";

export type FormAction<Input, Output> = (
  input: Input,
  options?: RustCallOptions,
) => Promise<Output>;

export interface FormOptions<Input extends object> {
  fields?: FieldMap<Input>;
}

export interface FormState<Input extends object, Output> {
  data: Input;
  setData: Dispatch<SetStateAction<Input>>;
  setField<Field extends FieldName<Input>>(field: Field, value: Input[Field]): void;
  field<Field extends FieldName<Input>>(name: Field): FieldProps<Input, Field>;
  errors: FieldErrors<Input>;
  error(field: FieldName<Input>): string | undefined;
  processing: boolean;
  /** Upload progress 0–1 while a multipart submission is in flight, else null. */
  progress: number | null;
  wasSuccessful: boolean;
  result: Output | undefined;
  failure: unknown;
  submit(): Promise<Output>;
  reset(): void;
  clearErrors(...fields: FieldName<Input>[]): void;
  cancel(): void;
}

export function useForm<Input extends object, Output>(
  action: FormAction<Input, Output>,
  initialValues: Input,
  options: FormOptions<Input> = {},
): FormState<Input, Output> {
  const initialValuesRef = useRef(initialValues);
  const fieldsRef = useRef(options.fields);
  fieldsRef.current = options.fields;
  const [data, setData] = useState<Input>(initialValues);
  const [errors, setErrors] = useState<FieldErrors<Input>>({});
  const [processing, setProcessing] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [wasSuccessful, setWasSuccessful] = useState(false);
  const [result, setResult] = useState<Output>();
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

  const submit = useCallback(async (): Promise<Output> => {
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
      let output: Output;
      if (hasFileValue(data)) {
        const routeName = (action as Partial<RustAction<Input, Output>>).routeName;
        if (!routeName) {
          throw new Error(
            "Phoenix multipart submissions require a generated named action (missing routeName)",
          );
        }
        setProgress(0);
        output = await uploadRust<Output>(routeName, data as Record<string, unknown>, {
          signal: controller.signal,
          onUploadProgress: (value) => {
            if (submission === submissionRef.current) setProgress(value);
          },
        });
      } else {
        output = await action(data, { signal: controller.signal });
      }
      if (submission !== submissionRef.current) throw abortError();
      setResult(output);
      setWasSuccessful(true);
      return output;
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
        setProgress(null);
      }
    }
  }, [action, data]);

  return {
    data,
    setData,
    setField,
    field: (name) => fieldProps({ data, setField, errors }, name, fieldsRef.current?.[name]),
    errors,
    error: (field) => errors[field]?.[0]?.message,
    processing,
    progress,
    wasSuccessful,
    result,
    failure,
    submit,
    reset,
    clearErrors,
    cancel,
  };
}

export interface FormProps<Input extends object, Output> extends Omit<
  FormHTMLAttributes<HTMLFormElement>,
  "action" | "children" | "onError" | "onSubmit"
> {
  action: FormAction<Input, Output>;
  initialValues: Input;
  fields?: FieldMap<Input>;
  children: ReactNode | ((form: FormState<Input, Output>) => ReactNode);
  confirm?: string;
  /** sessionStorage draft key name (`phoenix:remember:<name>`); cleared after successful submit */
  remember?: string;
  redirectTo?: string;
  onSuccess?: (output: Output) => void;
  onError?: (error: unknown, errors: FieldErrors<Input>) => void;
}

export function Form<Input extends object, Output>({
  action,
  initialValues,
  fields,
  children,
  confirm,
  remember,
  redirectTo,
  onSuccess,
  onError,
  ...props
}: FormProps<Input, Output>): ReactElement {
  const form = useForm(action, initialValues, { fields });
  const storageKey = remember ? rememberKey(remember) : "";
  useRemember(storageKey, form.data, form.setData);
  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (confirm && !confirmAction(confirm)) return;
    try {
      const output = await form.submit();
      if (storageKey) clearRemembered(storageKey);
      onSuccess?.(output);
      if (redirectTo) void redirect(redirectTo);
    } catch (error) {
      if (!isAbortError(error)) onError?.(error, fieldErrorsFrom<Input>(error));
    }
  };
  const content = typeof children === "function" ? children(form) : children;
  return createElement("form", { ...props, onSubmit: handleSubmit }, content);
}

export interface FieldErrorProps<Input extends object> extends Omit<
  HTMLAttributes<HTMLSpanElement>,
  "children"
> {
  errors: FieldErrors<Input>;
  name: FieldName<Input>;
}

export function FieldError<Input extends object>({
  errors,
  name,
  role = "alert",
  ...props
}: FieldErrorProps<Input>): ReactElement | null {
  const message = errors[name]?.[0]?.message;
  if (!message) return null;
  return createElement("span", {
    ...props,
    role,
    "data-phoenix-error": name,
    children: message,
  });
}
