//! GNU gettext setup and session-locale selection.

use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};

const DOMAIN: &str = "sulafat";
const DEFAULT_LOCALE_DIR: &str = "/usr/share/locale";

pub fn tr(message: &str) -> String {
    gettextrs::gettext(message)
}

pub fn format(message: &str, values: &[(&str, &str)]) -> String {
    let mut result = tr(message);
    for (name, value) in values {
        result = result.replace(&format!("{{{name}}}"), value);
    }
    result
}

/// Return the supported BCP-47 locale for a POSIX locale, safely ignoring encoding and modifier.
fn normalize_locale(raw: &str) -> &'static str {
    let base = raw
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('-', "_")
        .to_ascii_lowercase();
    match base.as_str() {
        "pt" | "pt_br" => "pt-BR",
        "es" | "es_es" => "es-ES",
        "zh" | "zh_cn" | "zh_hans" => "zh-CN",
        "en" | "en_us" | "c" | "posix" => "en-US",
        _ => "en-US",
    }
}

fn session_locale(get: impl Fn(&str) -> Option<String>) -> &'static str {
    let raw = get("LC_ALL")
        .filter(|value| !value.is_empty())
        .or_else(|| get("LC_MESSAGES").filter(|value| !value.is_empty()))
        .or_else(|| get("LANG").filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "en_US.UTF-8".to_string());
    normalize_locale(&raw)
}

pub fn init() {
    let _ = setlocale(LocaleCategory::LcAll, "");
    let locale = session_locale(|name| std::env::var(name).ok());
    // Select only the messages category. Environment variables remain untouched for child
    // processes (notably OpenSSH), and the normalized locale cannot escape the supported set.
    let posix_locale = match locale {
        "pt-BR" => "pt_BR.UTF-8",
        "es-ES" => "es_ES.UTF-8",
        "zh-CN" => "zh_CN.UTF-8",
        _ => "en_US.UTF-8",
    };
    let _ = setlocale(LocaleCategory::LcMessages, posix_locale);
    let locale_dir = option_env!("SULAFAT_LOCALEDIR").unwrap_or(DEFAULT_LOCALE_DIR);
    bindtextdomain(DOMAIN, locale_dir).expect("failed to bind Sulafat translation directory");
    bind_textdomain_codeset(DOMAIN, "UTF-8").expect("failed to select UTF-8 translations");
    textdomain(DOMAIN).expect("failed to select Sulafat translation domain");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn normalizes_supported_posix_locales_and_falls_back() {
        assert_eq!(normalize_locale("en_US.UTF-8"), "en-US");
        assert_eq!(normalize_locale("pt_BR.UTF-8@custom"), "pt-BR");
        assert_eq!(normalize_locale("es_ES.utf8"), "es-ES");
        assert_eq!(normalize_locale("zh_CN.UTF-8"), "zh-CN");
        assert_eq!(normalize_locale("fr_FR.UTF-8"), "en-US");
        assert_eq!(normalize_locale("../../pt_BR"), "en-US");
    }

    #[test]
    fn locale_environment_uses_posix_precedence() {
        let vars = HashMap::from([
            ("LANG", "zh_CN.UTF-8".to_string()),
            ("LC_MESSAGES", "es_ES.UTF-8".to_string()),
            ("LC_ALL", "pt_BR.UTF-8".to_string()),
        ]);
        assert_eq!(session_locale(|name| vars.get(name).cloned()), "pt-BR");
    }

    #[test]
    fn each_required_lang_and_unsupported_fallback_are_selected() {
        for (lang, expected) in [
            ("en_US.UTF-8", "en-US"),
            ("pt_BR.UTF-8", "pt-BR"),
            ("es_ES.UTF-8", "es-ES"),
            ("zh_CN.UTF-8", "zh-CN"),
            ("fr_FR.UTF-8", "en-US"),
        ] {
            assert_eq!(
                session_locale(|name| (name == "LANG").then(|| lang.to_string())),
                expected
            );
        }
    }
}
