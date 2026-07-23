import type { FieldErrors, FieldName } from "./actions.js";

export type FieldDescriptor = {
  name: string;
  type: string;
  required: boolean;
};

export type FieldMap<Input extends object> = {
  [K in FieldName<Input>]?: FieldDescriptor;
};

export type FieldChangeEvent = {
  currentTarget: {
    value: string;
    type?: string;
    checked?: boolean;
  };
};

export type FieldProps<Input extends object, K extends FieldName<Input>> = {
  name: K;
  value: Input[K];
  required?: boolean;
  "aria-invalid"?: boolean;
  "data-phoenix-field": string;
  onChange: (event: FieldChangeEvent) => void;
};

export type FieldFormSlice<Input extends object> = {
  data: Input;
  setField<Field extends FieldName<Input>>(field: Field, value: Input[Field]): void;
  errors: FieldErrors<Input>;
};

export function fieldProps<Input extends object, K extends FieldName<Input>>(
  form: FieldFormSlice<Input>,
  name: K,
  descriptor?: FieldDescriptor,
): FieldProps<Input, K> {
  const props: FieldProps<Input, K> = {
    name,
    value: form.data[name],
    "data-phoenix-field": name,
    onChange: (event) => {
      const target = event.currentTarget;
      let next: unknown = target.value;
      if (target.type === "checkbox") {
        next = Boolean(target.checked);
      } else if (target.type === "number") {
        next = Number(target.value);
      }
      form.setField(name, next as Input[K]);
    },
  };
  if (form.errors[name]?.length) props["aria-invalid"] = true;
  if (descriptor?.required) props.required = true;
  return props;
}
