import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type PropsWithChildren
} from "react";
import { messages, type I18nKey } from "./messages";

export type LanguageCode = keyof typeof messages;

const supportedLanguages = Object.keys(messages) as LanguageCode[];
const storageKey = "bibi_work_language";

interface I18nContextValue {
  language: LanguageCode;
  setLanguage: (language: LanguageCode) => void;
  t: (key: I18nKey, values?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nContextValue>({
  language: "zh-CN",
  setLanguage: () => {},
  t: (key, values) => interpolate(messages["zh-CN"][key] ?? key, values)
});

export function I18nProvider({ children }: PropsWithChildren) {
  const [language, setLanguageState] = useState<LanguageCode>(() => detectInitialLanguage());

  const setLanguage = useCallback((nextLanguage: LanguageCode) => {
    setLanguageState(nextLanguage);
    try {
      localStorage.setItem(storageKey, nextLanguage);
    } catch {
      // Language preference is non-sensitive; failing to persist should not block the UI.
    }
  }, []);

  const value = useMemo<I18nContextValue>(() => {
    const activeMessages = messages[language];
    return {
      language,
      setLanguage,
      t(key, values) {
        return interpolate(activeMessages[key] ?? messages["zh-CN"][key] ?? key, values);
      }
    };
  }, [language, setLanguage]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}

export function languageLabel(language: LanguageCode): string {
  return language === "zh-CN"
    ? messages["zh-CN"]["app.language.zh"]
    : messages["en-US"]["app.language.en"];
}

export function languageLocale(language: LanguageCode): string {
  return language === "zh-CN" ? "zh-CN" : "en-US";
}

export function availableLanguages(): LanguageCode[] {
  return supportedLanguages;
}

function detectInitialLanguage(): LanguageCode {
  const stored = safeStoredLanguage();
  if (stored) {
    return stored;
  }
  const browserLanguage = navigator.language.toLowerCase();
  return browserLanguage.startsWith("zh") ? "zh-CN" : "en-US";
}

function safeStoredLanguage(): LanguageCode | null {
  try {
    const stored = localStorage.getItem(storageKey);
    return isLanguageCode(stored) ? stored : null;
  } catch {
    return null;
  }
}

function isLanguageCode(value: string | null): value is LanguageCode {
  return supportedLanguages.includes(value as LanguageCode);
}

function interpolate(template: string, values?: Record<string, string | number>): string {
  if (!values) {
    return template;
  }
  return template.replace(/\{(\w+)\}/g, (match, key: string) => {
    const value = values[key];
    return value === undefined ? match : String(value);
  });
}
