# Справочник архивов Linux

[English version](releases.md) · [Установка](../INSTALL.ru.md)

## Релиз

[Текущий релиз](https://github.com/pgtoolz/Kronika/releases/latest)
содержит четыре программы для Linux x86-64 и ARM64.

<a id="download"></a>
## Скачивание

| Архитектура | Архив | Контрольная сумма |
| --- | --- | --- |
| x86-64 | [kronika-1.0.1-x86_64-unknown-linux-musl.tar.gz](https://github.com/pgtoolz/Kronika/releases/download/v1.0.1/kronika-1.0.1-x86_64-unknown-linux-musl.tar.gz) | [SHA-256](https://github.com/pgtoolz/Kronika/releases/download/v1.0.1/kronika-1.0.1-x86_64-unknown-linux-musl.tar.gz.sha256) |
| ARM64 | [kronika-1.0.1-aarch64-unknown-linux-musl.tar.gz](https://github.com/pgtoolz/Kronika/releases/download/v1.0.1/kronika-1.0.1-aarch64-unknown-linux-musl.tar.gz) | [SHA-256](https://github.com/pgtoolz/Kronika/releases/download/v1.0.1/kronika-1.0.1-aarch64-unknown-linux-musl.tar.gz.sha256) |

Выполните [команды скачивания и установки](../INSTALL.ru.md#1-скачивание-и-распаковка).
[HTML-пример](https://github.com/pgtoolz/Kronika/releases/download/v1.0.1/kronika-v1.0.1.html)
работает без сети; его также можно [открыть в браузере](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.1.html).

## Состав и идентификация

```text
kronika-<cargo-version>-<target>/
  kronika-collector   kronika-web   kronika-dump   kronika-report
  BUILDINFO          SHA256SUMS    LICENSE       THIRD_PARTY_LICENSES.html
  README.md          README.ru.md  INSTALL.md    INSTALL.ru.md
  docs/              bins/        crates/       licenses/
```

Четыре программы собраны в формате Linux ELF со встроенными библиотеками
(статическая сборка). Вместе с ними находятся руководства на двух языках,
PNG-иллюстрации, светлые и тёмные SVG-схемы, редактируемые исходники draw.io и
уведомления о лицензиях. В `bins/` и `crates/` лежат связанные руководства.
Ссылки на исходники, которых нет в архиве, открывают соответствующий коммит
на GitHub.

| Файл или поле | Значение |
| --- | --- |
| Имя архива | Версия проекта и целевая платформа. |
| `BUILDINFO.package_source_revision` | Полный идентификатор коммита, из которого упакован архив. Рабочий каталог при упаковке не содержит несохранённых изменений. |
| `BUILDINFO.build_mode` | `source`: программы скомпилированы командой упаковки. `prebuilt`: готовые программы переданы через `--bin-dir`; их исходная ревизия и компилятор в этом режиме не определяются. |
| `BUILDINFO.source_date_epoch` | Время коммита упаковки в Unix-секундах. |
| Поля `BUILDINFO` при сборке из исходников | Команда сборки, компилятор Rust, параметры компиляции Rust и C. |
| `SHA256SUMS` | Контрольная сумма SHA-256 каждого файла, кроме самого списка сумм. |
| `<archive>.sha256` | SHA-256 сжатого архива. |

<a id="native-targets-и-матрица-userspace"></a>
## Архитектуры и проверенные дистрибутивы

| Целевая платформа | Машина сборки | Набор инструкций CPU |
| --- | --- | --- |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | `x86-64` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | `generic` |

Каждая сборка выполняется на машине своей архитектуры и создаёт четыре
программы один раз. Один и тот же архив затем проверяется в окружениях
перечисленных дистрибутивов:

| Дистрибутив | Образ контейнера | Архитектуры |
| --- | --- | --- |
| Ubuntu 22.04 LTS | `ubuntu:22.04` | x86-64, ARM64 |
| Ubuntu 24.04 LTS | `ubuntu:24.04` | x86-64, ARM64 |
| Ubuntu 26.04 LTS | `ubuntu:26.04` | x86-64, ARM64 |
| Debian 12 | `debian:bookworm-slim` | x86-64, ARM64 |
| Debian 13 | `debian:trixie-slim` | x86-64, ARM64 |
| CentOS Stream 9 | `quay.io/centos/centos:stream9` | x86-64, ARM64 |
| CentOS Stream 10 | `quay.io/centos/centos:stream10` | x86-64, ARM64 |
| Fedora 44 | `fedora:44` | x86-64, ARM64 |
| Alpine 3.24 | `alpine:3.24` | x86-64, ARM64 |
| Rocky Linux 9 | `rockylinux/rockylinux:9` | x86-64, ARM64 |
| openSUSE Leap 16.0 | `registry.opensuse.org/opensuse/leap:16.0` | x86-64, ARM64 |

Для каждой строки проверяются контрольные суммы архива и файлов, наличие
документации, архитектура ELF и отсутствие внешнего загрузчика и динамических
библиотек (`INTERP`/`NEEDED`). Все четыре программы запускаются для проверки
справки, версии и обработки аргументов. В результатах `portability-*`
сохраняются точный хеш образа, `/etc/os-release`, ядро, архитектура и вывод
проверки. Контейнеры используют ядро машины сборки: здесь проверяется
окружение дистрибутива с этим ядром, а не каждое возможное ядро Linux.
Список проверок: [release-package.yml](../.github/workflows/release-package.yml).

## Сборки для разработки

Войдите в учётную запись через [GitHub CLI](https://cli.github.com/manual/gh_run_download)
и выберите успешный запуск автоматической сборки:

```sh
gh run list --repo pgtoolz/Kronika --workflow release-package.yml --status success --limit 10
run_id=REPLACE_WITH_RUN_ID
gh run view "$run_id" --repo pgtoolz/Kronika
source_revision=$(gh run view "$run_id" --repo pgtoolz/Kronika --json headSha --jq .headSha)
target=x86_64-unknown-linux-musl
gh run download "$run_id" --repo pgtoolz/Kronika \
  --name "kronika-$source_revision-$target" --dir kronika-download
cd kronika-download
archive=$(basename kronika-*-"$target".tar.gz)
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
```

Для ARM64 задайте `target=aarch64-unknown-linux-musl`. Команда `gh run view`
показывает две сборки для соответствующих архитектур и 22 проверки запуска
в разных дистрибутивах. Результат загрузки содержит архив `.tar.gz` и его
контрольную сумму `.tar.gz.sha256`. После проверки и распаковки выполните
[установку](../INSTALL.ru.md#2-установка).

## Упаковка

Требования: рабочая копия без несохранённых изменений, Linux нужной архитектуры,
[указанные инструменты сборки](build.ru.md), GNU tar, gzip, binutils и Python 3.11+.

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

На машине ARM64 используйте `--target aarch64-unknown-linux-musl`. Платформа
по умолчанию — `x86_64-unknown-linux-musl`, каталог результата — `dist`.

Чтобы упаковать уже собранные статические программы:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl \
  --bin-dir target/x86_64-unknown-linux-musl/release \
  --output-dir dist-review
```

Скрипт отклоняет рабочие копии с несохранёнными изменениями, неподдерживаемые
платформы, программы с динамическими библиотеками, неверную архитектуру и
существующие выходные пути. Уведомления о зависимостях сверяются с
`licenses/dependency-inputs.sha256`.

Порядок файлов, права, владелец и группа, время и метаданные gzip фиксированы.
При одинаковых программах, документах, ревизии исходников и режиме сборки
получаются побайтово одинаковые архивы. Автоматическая проверка сравнивает
две упаковки одних и тех же программ.
Исходник: [package-release.sh](../scripts/package-release.sh).

## Проверки

При установленных `strace`, Node.js 22 и Chromium/Google Chrome передайте один архив:

```sh
scripts/check-release.sh dist/kronika-*-x86_64-unknown-linux-musl.tar.gz
```

| Режим | Проверки |
| --- | --- |
| По умолчанию | Проверка архива и команд; сбор настоящих данных Linux и чтение через dump; вырезание тестовой записи; два одинаковых HTML-отчёта; доступ к каталогу данных веб-сервера с аутентификацией; список инструментов MCP; работа HTML с диска и его сетевые запросы. |
| `--no-browser` | Все проверки, кроме браузерной; используется при сборке на ARM64. |
| `--cli-only` | Проверки архива и командной строки; используется для каждого дистрибутива. |

Проверки командной строки запускают программы без привилегий в каталоге только
для чтения, с пустыми и некорректными настройками окружения. Проверяются время
завершения, точное содержимое stdout и stderr, а также код завершения. При
проверке на машине сборки `strace` отслеживает, не начинают ли справка и версия
читать хранилище, вести журнал, создавать потоки, процессы или подключения. Исходники:
[check-release.sh](../scripts/check-release.sh), [check-cli.py](../scripts/check-cli.py).
