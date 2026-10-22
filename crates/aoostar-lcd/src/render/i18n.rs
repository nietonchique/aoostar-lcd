//! Panel UI strings in English and Russian, selected by
//! `[display] language` in the configuration.
//!
//! Static texts live in [`Strings`] tables; a few getters handle Russian
//! plural forms. Collector-produced detail texts (`Problem.detail`) are
//! stable English keys: [`Strings::detail`] maps the known ones and passes
//! anything unknown through unchanged.

use crate::collect::systemd::Target;
use crate::config::Lang;

/// Russian plural: one/few/many forms (`1 службу, 2 службы, 5 служб`).
fn ru_plural<'a>(n: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let n = n % 100;
    if (11..=14).contains(&n) {
        many
    } else {
        match n % 10 {
            1 => one,
            2..=4 => few,
            _ => many,
        }
    }
}

pub struct Strings {
    pub lang: Lang,
    // Status bar states.
    pub all_running: &'static str,
    pub down_label: &'static str,
    pub problem_label: &'static str,
    pub maintenance_label: &'static str,
    // Status bar chips.
    pub alerts_chip: &'static str,
    pub alerts_unknown_chip: &'static str,
    pub second_word: &'static str,
    pub minute_word: &'static str,
    pub hour_word: &'static str,
    pub day_word: &'static str,
    // Alarm takeover.
    pub alarm_title: &'static str,
    pub alarm_missing: &'static str,
    pub alarm_smart_failed: &'static str,
    pub alarm_service_down: &'static str,
    pub more_suffix: &'static str,
    // Home page.
    pub load_label: &'static str,
    pub free_ram_label: &'static str,
    pub of_word: &'static str,
    pub rate_unit: &'static str,
    pub idle_word: &'static str,
    pub max_word: &'static str,
    pub paused_word: &'static str,
    pub read_word: &'static str,
    pub write_word: &'static str,
    pub watt_unit: &'static str,
    pub smart_test: &'static str,
    // Problems page.
    pub no_problems: &'static str,
    pub alerts_word: &'static str,
    // CI page.
    pub no_builds: &'static str,
    pub queue_word: &'static str,
    pub waiting_word: &'static str,
    pub still_running_word: &'static str,
    // HDD page.
    pub disk_column: &'static str,
    pub busy_column: &'static str,
    pub read_column: &'static str,
    pub write_column: &'static str,
    pub disk_missing: &'static str,
    pub self_test_word: &'static str,
    pub no_hdds: &'static str,
    // ZFS page.
    pub free_of: &'static str,
    pub eta_word: &'static str,
    pub hdd_activity_title: &'static str,
    pub datasets_idle: &'static str,
    // Temps page.
    pub ram_label: &'static str,
    pub fan_label: &'static str,
    pub gone_word: &'static str,
    // Load page.
    pub memory_title: &'static str,
    pub no_data: &'static str,
    // Space page.
    pub no_volumes: &'static str,
    // Startup page.
    pub startup_title: &'static str,
    pub uptime_word: &'static str,
    pub all_services_up: &'static str,
    // Shutdown page.
    pub elapsed_word: &'static str,
    pub all_stopped: &'static str,
    pub status_unavailable: &'static str,
    // Binary size units.
    pub size_units: [&'static str; 6],
}

impl Strings {
    pub const EN: Strings = Strings {
        lang: Lang::En,
        all_running: "ALL RUNNING",
        down_label: "DOWN",
        problem_label: "PROBLEM",
        maintenance_label: "MAINTENANCE",
        alerts_chip: "alerts",
        alerts_unknown_chip: "alerts ?",
        second_word: "s",
        minute_word: "min",
        hour_word: "h",
        day_word: "d",
        alarm_title: "ALARM",
        alarm_missing: "missing",
        alarm_smart_failed: "SMART FAILED",
        alarm_service_down: "down",
        more_suffix: "and {n} more",
        load_label: "load",
        free_ram_label: "FREE RAM",
        of_word: "of",
        rate_unit: "MB/s",
        idle_word: "idle",
        max_word: "max",
        paused_word: "paused",
        read_word: "read",
        write_word: "write",
        watt_unit: "W",
        smart_test: "SMART test",
        no_problems: "no problems",
        alerts_word: "alerts",
        no_builds: "no builds running",
        queue_word: "queue",
        waiting_word: "waiting",
        still_running_word: "more running",
        disk_column: "disk",
        busy_column: "busy",
        read_column: "read MB/s",
        write_column: "write MB/s",
        disk_missing: "missing",
        self_test_word: "test",
        no_hdds: "no HDDs configured",
        free_of: "free",
        eta_word: "eta",
        hdd_activity_title: "HDD activity, MB/s",
        datasets_idle: "datasets idle",
        ram_label: "RAM",
        fan_label: "FAN",
        gone_word: "gone",
        memory_title: "MEMORY · free",
        no_data: "no data",
        no_volumes: "no volumes configured",
        startup_title: "STARTUP",
        uptime_word: "uptime",
        all_services_up: "all services up",
        elapsed_word: "elapsed",
        all_stopped: "all services stopped",
        status_unavailable: "status unavailable",
        size_units: ["B", "KB", "MB", "GB", "TB", "PB"],
    };

    pub const RU: Strings = Strings {
        lang: Lang::Ru,
        all_running: "ВСЁ РАБОТАЕТ",
        down_label: "НЕ РАБОТАЕТ",
        problem_label: "ПРОБЛЕМА",
        maintenance_label: "ОБСЛУЖИВАНИЕ",
        alerts_chip: "алерты",
        alerts_unknown_chip: "алерты ?",
        second_word: "с",
        minute_word: "мин",
        hour_word: "ч",
        day_word: "д",
        alarm_title: "АВАРИЯ",
        alarm_missing: "пропал",
        alarm_smart_failed: "SMART отказ",
        alarm_service_down: "лежит",
        more_suffix: "и ещё {n}",
        load_label: "загр.",
        free_ram_label: "СВОБ. ОЗУ",
        of_word: "из",
        rate_unit: "МБ/с",
        idle_word: "тихо",
        max_word: "макс.",
        paused_word: "пауза",
        read_word: "чтение",
        write_word: "запись",
        watt_unit: "Вт",
        smart_test: "SMART тест",
        no_problems: "проблем нет",
        alerts_word: "алертов",
        no_builds: "сборки не идут",
        queue_word: "очередь",
        waiting_word: "ждёт",
        still_running_word: "ещё идёт",
        disk_column: "диск",
        busy_column: "загрузка",
        read_column: "чтение МБ/с",
        write_column: "запись МБ/с",
        disk_missing: "нет диска",
        self_test_word: "тест",
        no_hdds: "HDD не настроены",
        free_of: "своб.",
        eta_word: "ещё",
        hdd_activity_title: "кто нагружает HDD, МБ/с",
        datasets_idle: "датасеты без активности",
        ram_label: "ОЗУ",
        fan_label: "ВЕНТ",
        gone_word: "нет",
        memory_title: "ПАМЯТЬ · своб.",
        no_data: "нет данных",
        no_volumes: "тома не настроены",
        startup_title: "ЗАПУСК",
        uptime_word: "с загрузки",
        all_services_up: "службы работают",
        elapsed_word: "прошло",
        all_stopped: "все службы остановлены",
        status_unavailable: "статус недоступен",
        size_units: ["Б", "КБ", "МБ", "ГБ", "ТБ", "ПБ"],
    };

    pub fn get(lang: Lang) -> &'static Strings {
        match lang {
            Lang::En => &Self::EN,
            Lang::Ru => &Self::RU,
        }
    }

    /// `more_suffix` with the count filled in: «и ещё 2» / "and 2 more".
    pub fn more(&self, n: usize) -> String {
        self.more_suffix.replace("{n}", &n.to_string())
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
            0..60 => format!("{seconds} {}", self.second_word),
            60..3600 => format!("{} {}", seconds / 60, self.minute_word),
            3600..172_800 => format!("{} {}", seconds / 3600, self.hour_word),
            _ => format!("{} {}", seconds / 86_400, self.day_word),
        }
    }

    /// Stopwatch for the CI chip: `1 ч 5 мин` / `7 мин`.
    pub fn stopwatch_short(&self, seconds: i64) -> String {
        if seconds >= 3600 {
            format!(
                "{} {} {} {}",
                seconds / 3600,
                self.hour_word,
                seconds / 60 % 60,
                self.minute_word
            )
        } else {
            format!("{} {}", seconds / 60, self.minute_word)
        }
    }

    /// Home headline under the big HDD rate: «МБ/с» / "MB/s".
    pub fn services_up_label(&self, n: usize) -> String {
        match self.lang {
            Lang::En => "services up".to_owned(),
            Lang::Ru => {
                let (noun, verb) = match n % 100 {
                    11..=14 => ("служб", "работает"),
                    _ => match n % 10 {
                        1 => ("служба", "работает"),
                        2..=4 => ("службы", "работают"),
                        _ => ("служб", "работает"),
                    },
                };
                format!("{noun} {verb}")
            }
        }
    }

    /// Shutdown headline label: «служб остановлено» / "services stopped".
    pub fn services_stopped_label(&self, n: usize) -> String {
        match self.lang {
            Lang::En => "services stopped".to_owned(),
            Lang::Ru => format!("{} остановлено", ru_plural(n, "служба", "службы", "служб")),
        }
    }

    /// Container count: «17 контейнеров» / "containers 17".
    pub fn containers(&self, n: u32) -> String {
        match self.lang {
            Lang::En => format!("containers {n}"),
            Lang::Ru => format!(
                "{n} {}",
                ru_plural(n as usize, "контейнер", "контейнера", "контейнеров")
            ),
        }
    }

    /// The «ещё {n} · алертов {total}» overflow line of the problems page.
    pub fn more_problems_line(&self, count: usize, alerts: u32) -> String {
        match self.lang {
            Lang::En => format!("{count} more · {alerts} alerts"),
            Lang::Ru => format!("ещё {count} · алертов {alerts}"),
        }
    }

    pub fn target_title(&self, target: Target) -> &'static str {
        match (target, self.lang) {
            (Target::Poweroff, Lang::En) => "SHUTDOWN",
            (Target::Reboot, Lang::En) => "REBOOT",
            (Target::Poweroff, Lang::Ru) => "ВЫКЛЮЧЕНИЕ",
            (Target::Reboot, Lang::Ru) => "ПЕРЕЗАГРУЗКА",
        }
    }

    /// Localizes a collector `Problem.detail` key; unknown text passes through.
    pub fn detail(&self, detail: &str) -> String {
        if self.lang == Lang::En {
            return detail.to_owned();
        }
        match detail {
            "stack inactive" => "стек не запущен".to_owned(),
            "unit failed" => "юнит упал".to_owned(),
            "unit missing" => "юнит отсутствует".to_owned(),
            "unit inactive" => "юнит не активен".to_owned(),
            "container unhealthy" => "контейнер нездоров".to_owned(),
            "container restarting" => "контейнер перезапускается".to_owned(),
            "maintenance" => "обслуживание".to_owned(),
            "alerts unavailable" => "алерты недоступны".to_owned(),
            "pool errors" => "ошибки пула".to_owned(),
            other => {
                if let Some(state) = other.strip_prefix("pool ") {
                    return format!("пул {state}");
                }
                if let Some((count, name)) = other.split_once(" alerts: ") {
                    return format!("{} {}: {name}", self.alerts_word, count.trim());
                }
                other.to_owned()
            }
        }
    }

    /// Localizes a `Problem.subject` key that is not a real service name.
    pub fn subject<'a>(&self, subject: &'a str) -> std::borrow::Cow<'a, str> {
        match (subject, self.lang) {
            ("failed jobs", Lang::Ru) => std::borrow::Cow::Borrowed("сбой задач"),
            (other, _) => std::borrow::Cow::Borrowed(other),
        }
    }
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
    fn russian_plural_forms() {
        assert_eq!(ru_plural(1, "а", "б", "в"), "а");
        assert_eq!(ru_plural(2, "а", "б", "в"), "б");
        assert_eq!(ru_plural(5, "а", "б", "в"), "в");
        assert_eq!(ru_plural(11, "а", "б", "в"), "в");
        assert_eq!(ru_plural(21, "а", "б", "в"), "а");
        assert_eq!(ru_plural(22, "а", "б", "в"), "б");
    }

    #[test]
    fn sizes_use_localized_units() {
        assert_eq!(Strings::EN.bytes(68 * 1024 * 1024 * 1024), "68 GB");
        assert_eq!(Strings::RU.bytes(68 * 1024 * 1024 * 1024), "68 ГБ");
        assert_eq!(Strings::RU.bytes(7_480_000_000_000), "6.8 ТБ");
        assert_eq!(Strings::RU.bytes(999 * 1024 * 1024), "999 МБ");
    }

    #[test]
    fn ages_switch_units_at_boundaries() {
        assert_eq!(Strings::RU.age(59), "59 с");
        assert_eq!(Strings::RU.age(60), "1 мин");
        assert_eq!(Strings::EN.age(3600), "1 h");
        assert_eq!(Strings::RU.age(172_800), "2 д");
    }

    #[test]
    fn service_counts_use_plural() {
        assert_eq!(Strings::RU.services_up_label(1), "служба работает");
        assert_eq!(Strings::RU.services_up_label(3), "службы работают");
        assert_eq!(Strings::RU.services_up_label(7), "служб работает");
        assert_eq!(Strings::EN.services_up_label(7), "services up");
        assert_eq!(Strings::RU.services_stopped_label(13), "служб остановлено");
        assert_eq!(Strings::RU.containers(0), "0 контейнеров");
        assert_eq!(Strings::RU.containers(1), "1 контейнер");
        assert_eq!(Strings::RU.containers(17), "17 контейнеров");
        assert_eq!(Strings::EN.containers(17), "containers 17");
    }

    #[test]
    fn detail_keys_localize_and_pass_through() {
        assert_eq!(Strings::RU.detail("stack inactive"), "стек не запущен");
        assert_eq!(Strings::RU.detail("pool DEGRADED"), "пул DEGRADED");
        assert_eq!(
            Strings::RU.detail("3 alerts: backup stale"),
            "алертов 3: backup stale"
        );
        assert_eq!(
            Strings::RU.detail("HTTPS probe failed"),
            "HTTPS probe failed"
        );
        assert_eq!(Strings::EN.detail("stack inactive"), "stack inactive");
    }

    #[test]
    fn more_problems_line_matches_language() {
        assert_eq!(Strings::RU.more_problems_line(2, 5), "ещё 2 · алертов 5");
        assert_eq!(Strings::EN.more_problems_line(2, 5), "2 more · 5 alerts");
    }
}
