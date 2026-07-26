import {
  createElement,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ImgHTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
} from "react";

import { urlFor } from "./urls.js";

export const CAPTCHA_ROUTE = "captcha.image";
export const STORED_CAPTCHA_ROUTE = "captcha.challenge";

export interface CaptchaState {
  /** Image URL for the current challenge (cache-busted per refresh). */
  src: string;
  /** Request a fresh challenge; the previous one is invalidated server-side. */
  refresh(): void;
}

let captchaSequence = 0;

/**
 * Track a captcha challenge image served by the `phoenix-captcha` feature.
 * Call `refresh()` after a 422 with `errors.captcha` — the failed attempt
 * consumed the challenge.
 */
export function useCaptcha(routeName: string = CAPTCHA_ROUTE): CaptchaState {
  const [seed, setSeed] = useState(() => {
    captchaSequence += 1;
    return captchaSequence;
  });
  const refresh = useCallback(() => {
    captchaSequence += 1;
    setSeed(captchaSequence);
  }, []);
  const src = useMemo(
    () => urlFor(routeName, {}, { query: { t: seed } }),
    [routeName, seed],
  );
  return { src, refresh };
}

export interface CaptchaImageProps extends Omit<
  ImgHTMLAttributes<HTMLImageElement>,
  "src" | "children"
> {
  /** Named route serving the challenge; defaults to `captcha.image`. */
  route?: string;
}

/** JSON body served by the `captcha.challenge` route. */
interface ChallengePayload {
  id: string;
  svg: string;
  expires_in: number;
}

export interface StoredCaptchaState {
  /** Challenge id to submit alongside the answer; `""` until one loads. */
  id: string;
  /** `data:` URL of the challenge image; `""` until one loads. */
  src: string;
  /** Seconds the current challenge stays valid; `0` until one loads. */
  expiresIn: number;
  /** A challenge request is in flight. */
  loading: boolean;
  /** The last request failed (network or non-2xx). */
  error: Error | null;
  /** Request a fresh challenge. */
  refresh(): void;
}

/**
 * Track a **session-less** captcha challenge served by the `captcha.challenge`
 * route (`CaptchaFeature::with_store`). Unlike {@link useCaptcha}, which relies
 * on a session cookie, this carries the challenge id explicitly — submit
 * `state.id` next to the answer.
 *
 * Call `refresh()` after a 422 with `errors.captcha`: the failed attempt
 * consumed the challenge.
 */
export function useStoredCaptcha(
  routeName: string = STORED_CAPTCHA_ROUTE,
): StoredCaptchaState {
  const [seed, setSeed] = useState(0);
  const [challenge, setChallenge] = useState<ChallengePayload | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);
  const refresh = useCallback(() => setSeed((current) => current + 1), []);

  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    void (async () => {
      try {
        const response = await fetch(urlFor(routeName, {}, { query: { t: seed } }), {
          headers: { accept: "application/json" },
          credentials: "same-origin",
          signal: controller.signal,
        });
        if (!response.ok) {
          throw new Error(`captcha challenge failed: HTTP ${response.status}`);
        }
        const payload = (await response.json()) as ChallengePayload;
        setChallenge(payload);
      } catch (cause) {
        if (controller.signal.aborted) return;
        // Drop the stale challenge: showing an image whose id we no longer
        // trust would submit an id the server will reject anyway.
        setChallenge(null);
        setError(cause instanceof Error ? cause : new Error(String(cause)));
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    })();
    return () => controller.abort();
  }, [routeName, seed]);

  const src = useMemo(
    () =>
      challenge
        ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(challenge.svg)}`
        : "",
    [challenge],
  );

  return {
    id: challenge?.id ?? "",
    src,
    expiresIn: challenge?.expires_in ?? 0,
    loading,
    error,
    refresh,
  };
}

export interface StoredCaptchaImageProps extends Omit<
  ImgHTMLAttributes<HTMLImageElement>,
  "src" | "children"
> {
  /** Named route serving the challenge; defaults to `captcha.challenge`. */
  route?: string;
  /**
   * Called with the challenge id whenever a new challenge loads. Wire it into
   * the form so the id is submitted with the answer:
   * `onChallenge={(id) => form.setField("captcha_id", id)}`.
   */
  onChallenge?: (id: string) => void;
}

/**
 * Session-less challenge image that loads a fresh captcha when clicked.
 *
 * The SVG is inlined as a `data:` URL rather than injected as markup, so no
 * server-supplied string ever reaches the DOM as HTML.
 */
export function StoredCaptchaImage({
  route = STORED_CAPTCHA_ROUTE,
  alt = "captcha",
  onChallenge,
  onClick,
  ...props
}: StoredCaptchaImageProps): ReactElement {
  const { id, src, refresh } = useStoredCaptcha(route);
  const notify = useRef(onChallenge);
  notify.current = onChallenge;
  useEffect(() => {
    if (id) notify.current?.(id);
  }, [id]);
  const handleClick = (event: ReactMouseEvent<HTMLImageElement>) => {
    onClick?.(event);
    if (!event.defaultPrevented) refresh();
  };
  return createElement("img", {
    ...props,
    src,
    alt,
    onClick: handleClick,
    "data-phoenix-captcha": "",
    "data-phoenix-captcha-id": id || undefined,
  });
}

/** Challenge image that loads a fresh captcha when clicked. */
export function CaptchaImage({
  route = CAPTCHA_ROUTE,
  alt = "captcha",
  onClick,
  ...props
}: CaptchaImageProps): ReactElement {
  const { src, refresh } = useCaptcha(route);
  const handleClick = (event: ReactMouseEvent<HTMLImageElement>) => {
    onClick?.(event);
    if (!event.defaultPrevented) refresh();
  };
  return createElement("img", {
    ...props,
    src,
    alt,
    onClick: handleClick,
    "data-phoenix-captcha": "",
  });
}
