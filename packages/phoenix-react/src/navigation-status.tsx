import {
  createElement,
  useEffect,
  useState,
  type ReactElement,
} from "react";

export interface NavigationStatusBannerProps {
  document?: Document;
  className?: string;
  offlineMessage?: string;
  errorMessage?: string | ((error: unknown) => string);
}

type BannerStatus = "error" | "offline" | null;

function defaultErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return "Navigation failed";
}

export function NavigationStatusBanner({
  document: documentProp,
  className,
  offlineMessage = "You are offline",
  errorMessage,
}: NavigationStatusBannerProps = {}): ReactElement | null {
  const documentRef = documentProp ?? document;
  const [offline, setOffline] = useState(() => {
    const windowRef = documentRef.defaultView;
    return windowRef ? windowRef.navigator.onLine === false : false;
  });
  const [navigationError, setNavigationError] = useState<unknown>(null);

  useEffect(() => {
    const windowRef = documentRef.defaultView;
    if (!windowRef) return;

    const onOffline = () => setOffline(true);
    const onOnline = () => setOffline(false);
    const onNavigationError = (event: Event) => {
      const detail = (event as CustomEvent<{ error?: unknown }>).detail;
      setNavigationError(detail?.error ?? new Error("Navigation failed"));
    };
    const clearError = () => setNavigationError(null);

    setOffline(windowRef.navigator.onLine === false);
    windowRef.addEventListener("offline", onOffline);
    windowRef.addEventListener("online", onOnline);
    documentRef.addEventListener("phoenix:navigation-error", onNavigationError);
    documentRef.addEventListener("phoenix:navigation-start", clearError);
    documentRef.addEventListener("phoenix:navigation-success", clearError);

    return () => {
      windowRef.removeEventListener("offline", onOffline);
      windowRef.removeEventListener("online", onOnline);
      documentRef.removeEventListener("phoenix:navigation-error", onNavigationError);
      documentRef.removeEventListener("phoenix:navigation-start", clearError);
      documentRef.removeEventListener("phoenix:navigation-success", clearError);
    };
  }, [documentRef]);

  const status: BannerStatus = offline ? "offline" : (navigationError != null ? "error" : null);
  if (!status) return null;

  const message = status === "offline"
    ? offlineMessage
    : typeof errorMessage === "function"
      ? errorMessage(navigationError)
      : (errorMessage ?? defaultErrorMessage(navigationError));

  return createElement(
    "div",
    {
      className,
      role: "status",
      "data-phoenix-navigation-status": status,
    },
    message,
  );
}
