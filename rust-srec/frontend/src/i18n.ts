import { i18n } from "@lingui/core";
import { messages as enMessages } from "./locales/en/messages";
import { messages as zhCNMessages } from "./locales/zh-CN/messages";

// Load messages for all locales
i18n.load({
    en: enMessages,
    "zh-CN": zhCNMessages,
});

// Set initial locale
const savedLocale = typeof window !== "undefined" ? localStorage.getItem("locale") : "en";
i18n.activate(savedLocale || "en");

export { i18n };
