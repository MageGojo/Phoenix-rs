import {
  Component,
  createElement,
  type ComponentType,
  type ErrorInfo,
  type ReactNode,
} from "react";

export interface ErrorFallbackProps {
  error: Error;
  reset: () => void;
}

export interface PhoenixErrorBoundaryProps {
  children?: ReactNode;
  fallback?: ComponentType<ErrorFallbackProps>;
  onError?: (error: Error, info: ErrorInfo) => void;
}

interface PhoenixErrorBoundaryState {
  error: Error | null;
}

export function DefaultErrorFallback({ error, reset }: ErrorFallbackProps) {
  return createElement(
    "div",
    { "data-phoenix-error-boundary": "" },
    createElement("p", null, error.message),
    createElement(
      "button",
      { type: "button", onClick: () => reset() },
      "Try again",
    ),
  );
}

export class PhoenixErrorBoundary extends Component<
  PhoenixErrorBoundaryProps,
  PhoenixErrorBoundaryState
> {
  state: PhoenixErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): PhoenixErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    this.props.onError?.(error, info);
  }

  private readonly reset = (): void => {
    this.setState({ error: null });
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children ?? null;
    const Fallback = this.props.fallback ?? DefaultErrorFallback;
    return createElement(Fallback, { error, reset: this.reset });
  }
}
