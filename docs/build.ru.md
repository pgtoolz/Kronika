# Сборка из исходников

[English version](build.md) · [Установка](../INSTALL.ru.md)

Можно установить [готовую сборку](../INSTALL.ru.md) или собрать Kronika из
исходников по инструкции ниже.

<a id="toolchain"></a>
## Инструменты сборки

| Компонент | Версия или требование |
| --- | --- |
| Rust | `1.96.0`, закреплён в [rust-toolchain.toml](../rust-toolchain.toml). Версии зависимостей закреплены в `Cargo.lock`. |
| Системные инструменты | Компилятор C, GNU make, `pkg-config`; для статической сборки нужен `musl-gcc` вашей архитектуры. Пакеты Debian/Ubuntu: `build-essential musl-tools pkg-config`. |
| Файлы веб-интерфейса | Готовые HTML и WebAssembly уже хранятся в репозитории и используются при обычной сборке Cargo. |
| Пересборка веб-интерфейса | Node.js `22.21.1`, `wasm32-unknown-unknown` и закреплённые зависимости npm. |

<a id="native-binaries-для-linux"></a>
## Программы для Linux

Установив rustup и системные инструменты сборки, склонируйте репозиторий
и перейдите в него:

```sh
git clone https://github.com/pgtoolz/Kronika.git
cd Kronika
```

Для конкретной версии исходного кода выполните `git checkout FULL_COMMIT`
перед `cargo build`; для исходников опубликованного релиза — `git checkout v1.0.0`.

Выберите команду сборки для своей машины. Для Linux x86-64:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Программы появятся в `target/x86_64-unknown-linux-musl/release/` под именами
`kronika-collector`, `kronika-web`, `kronika-dump` и `kronika-report`.
По умолчанию Cargo собирает для `x86_64-unknown-linux-musl`, как указано в
[.cargo/config.toml](../.cargo/config.toml).

На машине Linux ARM64:

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
CC_aarch64_unknown_linux_musl=musl-gcc \
CFLAGS_aarch64_unknown_linux_musl=-mno-outline-atomics \
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Результат находится в `target/aarch64-unknown-linux-musl/release/`. Флаг
компилятора C встраивает атомарные операции ARMv8 непосредственно в программу.
Установите четыре программы, находясь в каталоге репозитория. Для ARM64
замените значение первой строки на `target/aarch64-unknown-linux-musl/release`:

```sh
binary_dir=target/x86_64-unknown-linux-musl/release
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 "$binary_dir/kronika-collector" "$binary_dir/kronika-web" \
  "$binary_dir/kronika-dump" "$binary_dir/kronika-report" /usr/local/bin/
```

Затем [запустите сбор](../INSTALL.ru.md#3-запуск-сборщика). Проверки и пересборка
веб-интерфейса ниже нужны для разработки и не требуются при обычной установке.

Чтобы собрать все программы репозитория, включая инструменты разработки,
на машине Linux x86-64 с инструментами GNU:

```sh
make build TARGET=x86_64-unknown-linux-gnu
```

Без явного `TARGET` команда `make build` выбирает платформу установленного rustc.

## Проверки

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
make fmt-check lint test
```

`lint` проверяет код правилами Dylint репозитория и Mordant, а затем Clippy;
предупреждения считаются ошибками. Зависимости перечислены в
[настройке Dylint](../scripts/check-dylints.sh). `test` запускает модульные и
интеграционные тесты. Проверки пользовательских сценариев BDD выполняются
отдельно в CI — автоматических проверках репозитория.

<a id="browser-assets"></a>
## Пересборка файлов веб-интерфейса

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

Эти команды собирают страницу веб-интерфейса, интерфейс отчёта и его
WebAssembly-программу, затем проверяют их соответствие файлам в репозитории.
Обработка запросов отчёта выполняется в основном потоке браузера. Команды определены в
[Makefile](../Makefile).

## Архив

Сохраните изменения коммитом и убедитесь, что рабочий каталог чист, затем
запустите:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

[Руководство по архивам](releases.ru.md) описывает поддерживаемые платформы,
имена архивов, сведения о сборке, контрольные суммы, вложенные документы и
автоматические проверки.
