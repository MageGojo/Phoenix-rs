import { useMemo } from "react";

import { usePage } from "./page-state.js";

export type TranslationParams = Record<string, string | number>;
export type TranslationMap = Record<string, string>;

/** Replace `{name}` slots in a template with `params.name`; unknown slots stay literal. */
export function interpolate(template: string, params?: TranslationParams): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : whole,
  );
}

/**
 * Resolve `key` against a translation map and interpolate params. Mirrors the
 * server `translate`: a missing key falls back to the key itself.
 */
export function translate(
  translations: TranslationMap | undefined,
  key: string,
  params?: TranslationParams,
): string {
  const template = translations?.[key];
  return template === undefined ? key : interpolate(template, params);
}

export interface TranslationsHook {
  /** Negotiated locale for the current page (defaults to "en"). */
  locale: string;
  /** Translate a key with optional `{name}` params; missing keys return the key. */
  t: (key: string, params?: TranslationParams) => string;
  /** True when a key has a template in the current locale. */
  has: (key: string) => boolean;
}

/**
 * Read the negotiated locale and translation catalog the server injected into
 * the page envelope (see `phoenix_view::i18n`). `t` interpolates `{name}` slots
 * and falls back to the key, identical to the Rust `translate`.
 */
export function useTranslations(): TranslationsHook {
  const { envelope } = usePage();
  const locale = envelope.locale ?? "en";
  const translations = envelope.translations;
  return useMemo(() => ({
    locale,
    t: (key: string, params?: TranslationParams) => translate(translations, key, params),
    has: (key: string) => translations?.[key] !== undefined,
  }), [locale, translations]);
}
