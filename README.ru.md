# aoostar-lcd

Статус-панель для переднего LCD сервера AOOSTAR WTR MAX (960×376, USB
serial). Один systemd-сервис рисует все экраны: аварии, прогресс загрузки и
выключения, диски, ZFS, Docker, CI. Язык интерфейса: русский или английский —
и любой другой, добавленный TOML-таблицей перевода.

![rotation](docs/img/rotation.gif)

| | | | |
|---|---|---|---|
| ![home](docs/img/home.png) | ![startup](docs/img/startup.png) | ![alarm](docs/img/alarm.png) | ![reboot](docs/img/reboot.png) |
| ![problems](docs/img/problems.png) | ![ci](docs/img/ci.png) | ![hdd](docs/img/hdd.png) | ![zfs](docs/img/zfs.png) |

Страницы ротируются по релевантности: сводка видна всегда; проблемы, CI и
HDD подключаются только когда уместны; полноэкранная мигающая **АВАРИЯ**
перекрывает всё, если сломан настроенный диск/пул или лежит критичный
сервис. При загрузке страница **ЗАПУСК** перечисляет ещё не поднятые
сервисы; при выключении/перезагрузке хук `ExecStopPost` рисует прогресс
**ВЫКЛЮЧЕНИЕ / ПЕРЕЗАГРУЗКА**, пока не остановится всё наблюдаемое.

- **[Откуда берутся данные](docs/data-sources.ru.md)** — 11 источников «только
  чтение»: что каждый читает, с каким интервалом и как деградирует при сбоях.
- **[Конфигурация](docs/configuration.ru.md)** — все секции и ключи.
- **[Загрузка и смена состояний питания](docs/boot-shutdown.ru.md)** — почему
  юнит устроен именно так.
- [English documentation](README.md).

## Языки

`i18n/en.toml` и `i18n/ru.toml` вшиты в бинарник; выбранный задаёт ключ
`[display] language`. Новый язык — один переведённый TOML-файл: скопируйте
`i18n/en.toml`, переведите значения, назовите файлом кода языка; каталог
`[display] i18n_dir` позволяет подгружать таблицы без пересборки. Формы
множественного числа объявляются по языку (`plural_family = "en" | "ru"`).

## Установка

Linux + systemd, Rust 1.88+, `libudev-dev`, `pkg-config`.

```bash
cargo build --locked --release -p aoostar-lcd
sudo install -Dm0755 target/release/aoostar-lcd /usr/local/bin/aoostar-lcd
sudo install -Dm0644 config.example.toml /etc/aoostar-lcd.toml   # отредактировать
sudo install -Dm0644 dist/aoostar-lcd.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now aoostar-lcd
```

CLI: `run`, `check` (проверка конфига без записи в панель), `snapshot`
(JSON), `preview` (рендер страниц в PNG из снапшота), `shutdown`, `off`.

## Разработка

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Офлайн-превью всех страниц из фикстуры снапшота
cargo run -p aoostar-lcd -- preview --config config.example.toml \
  --fixture crates/aoostar-lcd/fixtures/busy.json --out-dir /tmp/lcd
```

## Лицензия

MIT OR Apache-2.0; Roboto Condensed — SIL OFL. Протокол панели — из
[zehnm/aoostar-rs](https://github.com/zehnm/aoostar-rs).
