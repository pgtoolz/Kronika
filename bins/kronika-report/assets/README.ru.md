<a id="генерируемые-ресурсы-отчёта"></a>
# Собранные файлы отчёта

[English version](README.md)

Оболочка интерфейса собирается из основного кода React в `bins/kronika-web/ui`.
Связующий JavaScript и сжатый WebAssembly собираются из
`crates/kronika-report-wasm` версиями Rust и wasm-bindgen, закреплёнными в
репозитории. Проверка воспроизводимости побайтово сравнивает результат с
файлами репозитория.

Из корня репозитория выполните `scripts/report-assets.sh build` с
`wasm-bindgen 0.2.127` в `PATH` либо задайте путь к нему в `WASM_BINDGEN`.
Команда `scripts/report-assets.sh build --download-bindgen` загружает
закреплённую статическую сборку для x86_64 Linux musl, если локальная программа
не найдена, и проверяет её SHA-256 перед запуском.
`scripts/report-assets.sh check` сравнивает новую сборку с
сохранёнными JavaScript и gzip-файлами. `CARGO_BIN` и `NODE_BIN` задают пути к
Cargo и Node вместо поиска в `PATH`. Чтобы сборки на разных машинах совпадали,
скрипт задаёт одинаковые замены путей репозитория и каталога Cargo, начальное
значение генератора `const-random` и идентификатор компилятора C. Для сжатия
используется `pako` из зависимостей интерфейса с версией, закреплённой в lock-файле:
Apple gzip и GNU gzip дают разные байты.

Размер WebAssembly — 10 068 598 байт, gzip — 2 428 738 байт, SHA-256 gzip:
`197e5c32d05271ee5ffcf33b124b9d79807122436e43cc32c80b4bf5e53f6dc1`.
Размер связующего JavaScript — 3 885 байт, SHA-256:
`4635ae734e8c1e1aeb463ae1096f4fdc2a65d98e715b55cee9fe46956f29cba8`.
