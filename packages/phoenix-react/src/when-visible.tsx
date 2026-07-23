import {
  createElement,
  useEffect,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";

import { useNavigator, usePage } from "./page-state.js";
import { isRecord } from "./protocol.js";

export interface WhenVisibleProps {
  data: string;
  rootMargin?: string;
  fallback?: ReactNode;
  children?: (value: unknown) => ReactNode;
}

type LoadStatus = "idle" | "loading" | "loaded" | "error";

/**
 * Lazily reload a single props key when the wrapper first enters the viewport.
 * Must be used inside {@link PhoenixPageProvider}.
 */
export function WhenVisible({
  data,
  rootMargin = "0px",
  fallback = null,
  children,
}: WhenVisibleProps): ReactElement {
  const navigator = useNavigator();
  const { props } = usePage();
  const [status, setStatus] = useState<LoadStatus>("idle");
  const [error, setError] = useState<unknown>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const startedRef = useRef(false);

  useEffect(() => {
    const node = rootRef.current;
    if (!node || startedRef.current) return;
    if (typeof IntersectionObserver === "undefined") return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        if (startedRef.current) return;
        startedRef.current = true;
        observer.disconnect();
        setStatus("loading");
        setError(null);
        void navigator
          .reload({
            only: [data],
            preserveScroll: true,
            preserveFocus: true,
          })
          .then(() => {
            setStatus("loaded");
          })
          .catch((cause) => {
            setError(cause);
            setStatus("error");
          });
      },
      { rootMargin },
    );

    observer.observe(node);
    return () => observer.disconnect();
  }, [data, navigator, rootMargin]);

  const value = isRecord(props) ? props[data] : undefined;
  const showFallback = status === "idle" || status === "loading";

  return createElement(
    "div",
    {
      ref: rootRef,
      "data-phoenix-when-visible": data,
      "data-phoenix-when-visible-status": status,
      ...(error != null ? { "data-phoenix-when-visible-error": "" } : {}),
    },
    showFallback ? fallback : children?.(value),
  );
}
