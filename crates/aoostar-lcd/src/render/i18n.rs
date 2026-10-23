//! Panel UI strings loaded from TOML translation tables.
//!
//! `i18n/<code>.toml` files are embedded in the binary; `[display] language`
//! picks one, and an optional `[display] i18n_dir` adds or overrides tables
//! at runtime — adding a language is a new TOML file, no code changes.
//! Missing keys fall back to English, then to the key itself. Plural phrases
//! select a `one/few/many/other` category per the table's declared family.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, bail};
use include_dir::{Dir, include_dir};

use crate::collect::systemd::Target;
use crate::config::Config;

static EMBEDDED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../i18n");

/// Loaded translation table.
pub struct I18n {
    plurals: PluralFamily,
    table: HashMap<String, String>,
    /// Plural phrases: key -> category -> template.
    plural: HashMap<String, HashMap<String, String>>,
    details: HashMap<String, String>,
    subjects: HashMap<String, String>,
    size_units: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluralFamily {
    /// one / other
    En,
    /// one / few / many / other (Russian, Ukrainian, Polish)
    Ru,
    /// No plural distinction (Chinese, Korean); every count uses "other".
    None,
}

/// The language part of a locale value: `ru_RU.UTF-8` -> `ru`, `zh_CN` ->
/// `zh`. `C`, `POSIX` and empty values have no language.
fn base_locale(value: &str) -> Option<String> {
    let base = value
        .split(['.', '_', '@'])
        .next()?
        .trim()
        .to_ascii_lowercase();
    (!base.is_empty() && base != "c" && base != "posix").then_some(base)
}

/// The system language from the locale environment, if it names one.
fn detect_language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find_map(|value| base_locale(&value))
}

impl I18n {
    /// Loads the configured language, falling back to English, then to the
    /// key itself for anything still missing. `auto` follows the system
    /// locale and silently falls back to English when it names a language
    /// without a table.
    pub fn load(config: &Config) -> anyhow::Result<Self> {
        let requested = config.display.language.as_str();
        let auto = requested == "auto";
        let language = if auto {
            detect_language().unwrap_or_else(|| "en".to_owned())
        } else {
            requested.to_owned()
        };
        let mut i18n = Self::english();
        let Some(table) = read_table(&language, config.display.i18n_dir.as_deref()) else {
            if auto {
                return Ok(i18n);
            }
            let mut known: Vec<String> = EMBEDDED
                .files()
                .filter(|file| file.path().extension().is_some_and(|e| e == "toml"))
                .filter_map(|file| {
                    file.path()
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                })
                .collect();
            if let Some(dir) = config.display.i18n_dir.as_deref()
                && let Ok(entries) = std::fs::read_dir(dir)
            {
                for entry in entries.flatten() {
                    if entry.path().extension().is_some_and(|e| e == "toml")
                        && let Some(stem) = entry.path().file_stem()
                    {
                        let stem = stem.to_string_lossy().into_owned();
                        if !known.contains(&stem) {
                            known.push(stem);
                        }
                    }
                }
            }
            known.sort();
            bail!(
                "language {language:?} not found; available: {}",
                known.join(", ")
            );
        };
        i18n.merge(table);
        Ok(i18n)
    }

    /// The embedded English table (also the fallback for missing keys).
    pub fn english() -> Self {
        let mut i18n = Self::empty();
        if let Some(table) = parse(
            EMBEDDED
                .get_file("en.toml")
                .expect("embedded en.toml exists")
                .contents_utf8()
                .expect("en.toml is utf-8"),
        ) {
            i18n.merge(table);
        }
        i18n
    }

    /// The embedded Russian table (used by tests).
    #[cfg(test)]
    pub fn russian() -> Self {
        let mut i18n = Self::english();
        if let Some(table) = parse(
            EMBEDDED
                .get_file("ru.toml")
                .expect("embedded ru.toml exists")
                .contents_utf8()
                .expect("ru.toml is utf-8"),
        ) {
            i18n.merge(table);
        }
        i18n
    }

    fn empty() -> Self {
        Self {
            plurals: PluralFamily::En,
            table: HashMap::new(),
            plural: HashMap::new(),
            details: HashMap::new(),
            subjects: HashMap::new(),
            size_units: Vec::new(),
        }
    }

    fn merge(&mut self, table: toml::Value) {
        let root = match table.as_table() {
            Some(table) => table,
            None => return,
        };
        if let Some(family) = root.get("plural_family").and_then(|v| v.as_str()) {
            self.plurals = match family {
                "ru" | "uk" | "pl" => PluralFamily::Ru,
                "none" => PluralFamily::None,
                _ => PluralFamily::En,
            };
        }
        if let Some(units) = root
            .get("units")
            .and_then(|u| u.get("bytes"))
            .and_then(|v| v.as_array())
        {
            let units: Vec<String> = units
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if units.len() == 6 {
                self.size_units = units;
            }
        }
        // Flat sections: "section.key" entries.
        for section in [
            "status", "units", "alarm", "home", "problems", "ci", "hdd", "zfs", "temps", "load",
            "space", "phase",
        ] {
            if let Some(entries) = root.get(section).and_then(|v| v.as_table()) {
                for (key, value) in entries {
                    if let Some(text) = value.as_str() {
                        self.table
                            .insert(format!("{section}.{key}"), text.to_owned());
                    }
                }
            }
        }
        if let Some(entries) = root.get("plurals").and_then(|v| v.as_table()) {
            for (key, value) in entries {
                let lookup = format!("plurals.{key}");
                // A plain string acts as the "other" category; a table holds
                // explicit categories.
                match value {
                    toml::Value::String(text) => {
                        self.plural
                            .entry(lookup)
                            .or_default()
                            .insert("other".to_owned(), text.clone());
                    }
                    toml::Value::Table(categories) => {
                        for (category, text) in categories {
                            if let Some(text) = text.as_str() {
                                self.plural
                                    .entry(lookup.clone())
                                    .or_default()
                                    .insert(category.clone(), text.to_owned());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for section in ["details", "subjects"] {
            if let Some(entries) = root.get(section).and_then(|v| v.as_table()) {
                let target = match section {
                    "details" => &mut self.details,
                    _ => &mut self.subjects,
                };
                for (key, value) in entries {
                    if let Some(text) = value.as_str() {
                        target.insert(key.clone(), text.to_owned());
                    }
                }
            }
        }
    }

    /// Plain string by key, with English fallback.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        match self.table.get(key) {
            Some(text) => text.as_str(),
            None => key,
        }
    }

    /// Plural phrase with the count substituted.
    pub fn tn(&self, key: &str, n: u64) -> String {
        let category = match (self.plurals, n % 100, n % 10) {
            (PluralFamily::None, _, _) => "other",
            (PluralFamily::En, 1, _) => "one",
            (PluralFamily::En, _, _) => "other",
            (PluralFamily::Ru, 11..=14, _) => "many",
            (PluralFamily::Ru, _, 1) => "one",
            (PluralFamily::Ru, _, 2..=4) => "few",
            (PluralFamily::Ru, _, _) => "many",
        };
        self.plural
            .get(key)
            .and_then(|categories| categories.get(category).or_else(|| categories.get("other")))
            .map_or_else(
                || key.to_owned(),
                |template| template.replace("{n}", &n.to_string()),
            )
    }

    /// Template with `{name}`-style placeholders filled.
    pub fn tf(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let mut text = self.t(key).to_owned();
        for (name, value) in vars {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }

    /// Binary size with localized units: `512 КБ` / `512 KB`.
    pub fn bytes(&self, value: u64) -> String {
        const KIB: f64 = 1024.0;
        let mut amount = value as f64;
        let mut unit = 0;
        while amount >= 999.5 && unit + 1 < self.size_units.len() {
            amount /= KIB;
            unit += 1;
        }
        format!("{} {}", significant(amount), self.size_units[unit])
    }

    /// Coarse age: `40 с` / `12 мин` / `3 ч` / `5 д`.
    pub fn age(&self, seconds: i64) -> String {
        let seconds = seconds.max(0);
        match seconds {
            0..60 => format!("{seconds} {}", self.t("units.second")),
            60..3600 => format!("{} {}", seconds / 60, self.t("units.minute")),
            3600..172_800 => format!("{} {}", seconds / 3600, self.t("units.hour")),
            _ => format!("{} {}", seconds / 86_400, self.t("units.day")),
        }
    }

    /// Stopwatch for the CI chip: `1 ч 5 мин` / `7 мин`.
    pub fn stopwatch_short(&self, seconds: i64) -> String {
        if seconds >= 3600 {
            format!(
                "{} {} {} {}",
                seconds / 3600,
                self.t("units.hour"),
                seconds / 60 % 60,
                self.t("units.minute")
            )
        } else {
            format!("{} {}", seconds / 60, self.t("units.minute"))
        }
    }

    pub fn target_title(&self, target: Target) -> &str {
        match target {
            Target::Poweroff => self.t("phase.shutdown"),
            Target::Reboot => self.t("phase.reboot"),
        }
    }

    /// Localizes a collector `Problem.detail` key; unknown text passes through.
    pub fn detail(&self, detail: &str) -> String {
        if let Some(text) = self.details.get(detail) {
            return text.clone();
        }
        if let Some(prefix) = self.details.get("pool_prefix")
            && let Some(state) = detail.strip_prefix("pool ")
        {
            return format!("{prefix}{state}");
        }
        if let Some((count, name)) = detail.split_once(" alerts: ") {
            return format!(
                "{} {}: {name}",
                self.t("problems.alerts_word"),
                count.trim()
            );
        }
        detail.to_owned()
    }

    /// Localizes a `Problem.subject` key that is not a real service name.
    pub fn subject<'a>(&'a self, subject: &'a str) -> &'a str {
        match self.subjects.get(subject) {
            Some(text) => text.as_str(),
            None => subject,
        }
    }

    /// Splits the unit off a localized size: `("68", "ГБ")`.
    pub fn split_unit(&self, text: &str) -> (String, &str) {
        for unit in self.size_units.iter().rev() {
            if let Some(value) = text.strip_suffix(unit.as_str()) {
                return (value.trim_end().to_owned(), unit);
            }
        }
        (text.to_owned(), "")
    }
}

fn parse(text: &str) -> Option<toml::Value> {
    toml::from_str(text).ok()
}

fn read_table(language: &str, dir: Option<&Path>) -> Option<toml::Value> {
    if let Some(dir) = dir {
        let path = dir.join(format!("{language}.toml"));
        if let Ok(text) = std::fs::read_to_string(&path) {
            return parse(&text)
                .with_context(|| format!("parsing {}", path.display()))
                .ok();
        }
    }
    EMBEDDED
        .get_file(format!("{language}.toml"))
        .and_then(|file| file.contents_utf8())
        .and_then(parse)
}

fn significant(amount: f64) -> String {
    if amount < 9.95 {
        let text = format!("{amount:.1}");
        text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
    } else {
        format!("{amount:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_languages_load() {
        let en = I18n::english();
        assert_eq!(en.t("status.all_running"), "ALL RUNNING");
        assert_eq!(en.bytes(68 * 1024 * 1024 * 1024), "68 GB");
        let ru = I18n::russian();
        assert_eq!(ru.t("status.all_running"), "ВСЁ РАБОТАЕТ");
        assert_eq!(ru.bytes(68 * 1024 * 1024 * 1024), "68 ГБ");
    }

    #[test]
    fn plural_categories_follow_the_family() {
        let ru = I18n::russian();
        assert_eq!(ru.tn("plurals.services_up", 1), "служба работает");
        assert_eq!(ru.tn("plurals.services_up", 3), "службы работают");
        assert_eq!(ru.tn("plurals.services_up", 7), "служб работает");
        assert_eq!(ru.tn("plurals.services_up", 11), "служб работает");
        assert_eq!(ru.tn("plurals.services_up", 21), "служба работает");
        assert_eq!(ru.tn("plurals.containers", 17), "17 контейнеров");
        assert_eq!(ru.tn("plurals.services_stopped", 13), "служб остановлено");
        let en = I18n::english();
        assert_eq!(en.tn("plurals.services_up", 7), "services up");
        assert_eq!(en.tn("plurals.containers", 17), "containers 17");
    }

    #[test]
    fn ages_and_sizes_localize() {
        let ru = I18n::russian();
        assert_eq!(ru.age(59), "59 с");
        assert_eq!(ru.age(60), "1 мин");
        assert_eq!(ru.age(172_800), "2 д");
        assert_eq!(
            ru.split_unit(&ru.bytes(7_480_000_000_000)),
            ("6.8".to_owned(), "ТБ")
        );
        assert_eq!(I18n::english().age(3600), "1 h");
    }

    #[test]
    fn detail_keys_translate_and_pass_through() {
        let ru = I18n::russian();
        assert_eq!(ru.detail("stack inactive"), "стек не запущен");
        assert_eq!(ru.detail("pool DEGRADED"), "пул DEGRADED");
        assert_eq!(
            ru.detail("3 alerts: backup stale"),
            "алертов 3: backup stale"
        );
        assert_eq!(ru.detail("HTTPS probe failed"), "HTTPS probe failed");
        assert_eq!(I18n::english().detail("stack inactive"), "stack inactive");
        assert_eq!(ru.subject("failed jobs"), "сбой задач");
        assert_eq!(ru.subject("nginx"), "nginx");
    }

    #[test]
    fn missing_language_names_alternatives() {
        let config =
            crate::config::Config::parse("[display]\ndevice = \"/dev/ttyUSB0\"\nlanguage = \"xx\"")
                .unwrap();
        let error = I18n::load(&config).err().expect("xx must fail").to_string();
        assert!(error.contains("language \"xx\" not found"), "{error}");
        assert!(error.contains("en"), "{error}");
        assert!(error.contains("ru"), "{error}");
    }
}
