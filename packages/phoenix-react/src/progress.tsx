import {
  createElement,
  type ReactElement,
  useEffect,
  useRef,
  useState,
} from "react";

export interface ProgressBarProps {
  /** 默认 document */
  document?: Document;
  className?: string;
  /** 完成到消失的延迟 ms，默认 200 */
  hideDelayMs?: number;
}

type ProgressStatus = "idle" | "loading" | "finishing";

const START_PROGRESS = 0.1;
const MAX_LOADING_PROGRESS = 0.9;
const RAMP_INTERVAL_MS = 200;
const RAMP_FACTOR = 0.15;
const DEFAULT_Z_INDEX = 9999;

export function ProgressBar({
  document: documentProp,
  className,
  hideDelayMs = 200,
}: ProgressBarProps): ReactElement | null {
  const documentRef = documentProp ?? document;
  const [status, setStatus] = useState<ProgressStatus>("idle");
  const [progress, setProgress] = useState(0);
  const statusRef = useRef<ProgressStatus>("idle");
  const rampTimerRef = useRef<number | null>(null);
  const hideTimerRef = useRef<number | null>(null);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  useEffect(() => {
    const windowRef = documentRef.defaultView;
    if (!windowRef) return;

    const clearRamp = () => {
      if (rampTimerRef.current != null) {
        windowRef.clearInterval(rampTimerRef.current);
        rampTimerRef.current = null;
      }
    };

    const clearHide = () => {
      if (hideTimerRef.current != null) {
        windowRef.clearTimeout(hideTimerRef.current);
        hideTimerRef.current = null;
      }
    };

    const start = () => {
      clearHide();
      clearRamp();
      setStatus("loading");
      setProgress(START_PROGRESS);
      rampTimerRef.current = windowRef.setInterval(() => {
        setProgress((current) => {
          const next = current + (MAX_LOADING_PROGRESS - current) * RAMP_FACTOR;
          if (next >= MAX_LOADING_PROGRESS - 0.001) {
            clearRamp();
            return MAX_LOADING_PROGRESS;
          }
          return next;
        });
      }, RAMP_INTERVAL_MS);
    };

    const complete = () => {
      if (statusRef.current === "idle") return;
      clearRamp();
      clearHide();
      setProgress(1);
      setStatus("finishing");
      hideTimerRef.current = windowRef.setTimeout(() => {
        hideTimerRef.current = null;
        setStatus("idle");
        setProgress(0);
      }, hideDelayMs);
    };

    documentRef.addEventListener("phoenix:navigation-start", start);
    documentRef.addEventListener("phoenix:navigation-success", complete);
    documentRef.addEventListener("phoenix:navigation-hard", complete);
    documentRef.addEventListener("phoenix:navigation-error", complete);
    documentRef.addEventListener("phoenix:navigation-finish", complete);

    return () => {
      clearRamp();
      clearHide();
      documentRef.removeEventListener("phoenix:navigation-start", start);
      documentRef.removeEventListener("phoenix:navigation-success", complete);
      documentRef.removeEventListener("phoenix:navigation-hard", complete);
      documentRef.removeEventListener("phoenix:navigation-error", complete);
      documentRef.removeEventListener("phoenix:navigation-finish", complete);
    };
  }, [documentRef, hideDelayMs]);

  if (status === "idle") return null;

  return createElement("div", {
    className,
    "data-phoenix-progress": "",
    "data-status": status,
    "aria-hidden": true,
    style: {
      position: "fixed",
      top: 0,
      left: 0,
      height: "2px",
      width: `${progress * 100}%`,
      zIndex: DEFAULT_Z_INDEX,
      pointerEvents: "none",
      background: "currentColor",
      transition: "width 200ms ease-out",
    },
  });
}
