import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en";
import tr from "./locales/tr";

export type UiLanguage = "en" | "tr";

export function resolveLanguage(setting: string | undefined): UiLanguage {
  if (setting === "en" || setting === "tr") return setting;
  return navigator.language.toLowerCase().startsWith("tr") ? "tr" : "en";
}

/** Name of the UI language, used to ask the AI to write its reasons in it. */
export function languageName(lang: UiLanguage): string {
  return lang === "tr" ? "Turkish" : "English";
}

void i18n.use(initReactI18next).init({
  resources: { en: { translation: en }, tr: { translation: tr } },
  lng: resolveLanguage("system"),
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

export default i18n;
