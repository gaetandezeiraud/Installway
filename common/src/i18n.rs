//! Multi-language string lookup.
//!
//! Locales are TOML files under `common/locales/`, embedded at compile time
//! via `include_str!`. Each locale is a flat key→string map after parsing
//! (sections are flattened with dots, e.g. `install.next`).
//!
//! Detection on Windows uses `GetUserDefaultLocaleName` and takes the first
//! 2 ISO-639 chars (`"en-US"` → `"en"`). A `--lang <code>` CLI flag and the
//! `RUSTINSTALLER_LANG` env var both override detection. Unknown languages
//! fall back to English; missing keys fall back to English, then to the key
//! literal so nothing ever returns an empty string.

use std::collections::HashMap;
use std::sync::OnceLock;

const EN: &str = include_str!("../locales/en.toml");
const FR: &str = include_str!("../locales/fr.toml");

const SUPPORTED: &[(&str, &str)] = &[("en", EN), ("fr", FR)];
const DEFAULT_LANG: &str = "en";

static TABLES: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();

fn tables() -> &'static HashMap<&'static str, HashMap<String, String>> {
    TABLES.get_or_init(|| {
        let mut all = HashMap::new();
        for (code, src) in SUPPORTED {
            let mut flat = HashMap::new();
            if let Ok(v) = src.parse::<toml::Value>() {
                flatten(&v, "", &mut flat);
            }
            all.insert(*code, flat);
        }
        all
    })
}

fn flatten(v: &toml::Value, prefix: &str, out: &mut HashMap<String, String>) {
    if let Some(tbl) = v.as_table() {
        for (k, val) in tbl {
            let full = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{}.{}", prefix, k)
            };
            if val.is_table() {
                flatten(val, &full, out);
            } else if let Some(s) = val.as_str() {
                out.insert(full, s.to_string());
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Translator {
    lang: &'static str,
}

impl Translator {
    /// Build a translator for an explicit language code (2 chars).
    /// Unknown codes return the default-language translator.
    pub fn for_lang(code: &str) -> Self {
        let two = code.split(['-', '_']).next().unwrap_or(code).to_ascii_lowercase();
        for (c, _) in SUPPORTED {
            if *c == two {
                return Self { lang: c };
            }
        }
        Self { lang: DEFAULT_LANG }
    }

    /// Detect from CLI args (`--lang <code>`), env (`RUSTINSTALLER_LANG`),
    /// then OS user locale, then default.
    pub fn detect(args: &[String]) -> Self {
        if let Some(idx) = args.iter().position(|a| a == "--lang") {
            if let Some(c) = args.get(idx + 1) {
                return Self::for_lang(c);
            }
        }
        if let Ok(c) = std::env::var("RUSTINSTALLER_LANG") {
            if !c.is_empty() {
                return Self::for_lang(&c);
            }
        }
        #[cfg(windows)]
        if let Some(c) = os_user_locale() {
            return Self::for_lang(&c);
        }
        Self { lang: DEFAULT_LANG }
    }

    pub fn lang(&self) -> &'static str {
        self.lang
    }

    /// Look up a key. Missing → fall back to English → key literal.
    pub fn get(&self, key: &str) -> String {
        let t = tables();
        if let Some(s) = t.get(self.lang).and_then(|m| m.get(key)) {
            return s.clone();
        }
        if self.lang != DEFAULT_LANG {
            if let Some(s) = t.get(DEFAULT_LANG).and_then(|m| m.get(key)) {
                return s.clone();
            }
        }
        key.to_string()
    }

    /// Look up with `{placeholder}` substitution.
    pub fn fmt(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let mut s = self.get(key);
        for (k, v) in vars {
            s = s.replace(&format!("{{{}}}", k), v);
        }
        s
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self {
            lang: DEFAULT_LANG,
        }
    }
}

#[cfg(windows)]
fn os_user_locale() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if n <= 0 {
        return None;
    }
    let end = (n as usize).saturating_sub(1).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]);
    Some(s)
}
