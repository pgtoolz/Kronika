# Совместимость веб-интерфейса с Safari: исследование и план адаптации всего сайта

Ветка: `research/safari-compat` (от `main` @ `592a5e5`). Дата: 2026-11-12.
Коммиты: `3464758` (первый анализ), `f4a05db` (изолированное воспроизведение), текущая итерация — полный аудит сайта.

Статус: **исследование, без изменений в коде приложения**. Уровни достоверности:

| Метка | Значение |
|---|---|
| **[Код]** | подтверждено чтением исходников или разбором собранного `kronika-ui.html.gz` |
| **[Спека]** | подтверждено внешним источником (MDN BCD, WebKit release notes, документация Tailwind, RFC, исходники React 19 / uPlot) |
| **[Воспроизведено]** | подтверждено прогоном в Chrome/Blink и WebKit (§4.0, §4.1, §13) |
| **[Опровергнуто в стенде]** | утверждение из аудита **не подтвердилось** прогоном; остаётся гипотезой для реального Safari (§13.3) |
| **[Гипотеза]** | требует проверки на реальном Safari/iOS-устройстве |

> **Update 1.** Причина расхождения Chrome/Firefox vs Safari перестала быть гипотезой:
> воспроизведена в изолированном стенде
> `bins/kronika-web/ui/tests/safari-focus-probe.html` под Chrome 157 (Blink) и WebKit 26.0 (§4.0).
>
> **Update 2 (полный аудит).** Масштаб расширен с пикера на **весь сайт**: JS/DOM/API,
> CSS и вёрстка, сервер/CSP/транспорт/авторизация, экспорт и офлайн HTML-отчёт (§§7–12).
> Дополнительно прогнан **боевой артефакт** `kronika-ui.html.gz` в обоих движках с заголовками,
> повторяющими `bins/kronika-web/src/ui.rs` (§4.1, §13). Прогон подтвердил P0-1 на настоящем
> бандле, подтвердил, что CSP-хеши + gzip работают, и **опроверг/ослабил** две тревоги audita:
> «жёсткий фейл 304 из-за `Content-Length`/`Content-Encoding`» и «`SecurityError` от
> `localStorage` на `file://` в WebKit». Исправлены неточные подсчёты CSS и версионные пороги
> первой версии документа (§3.1).

---

## 1. Главный вывод: «Safari не работает с time picker» — не про нативный контрол

В Kronika **нет ни одного нативного date/time-инпута**. Ни `<input type="time">`, ни
`type="date"`, ни `type="datetime-local"`, ни `showPicker()`. **[Код]**

Оба «пикера» — кастомные поповеры React из обычных `<button>`:

| Что | Файл | Устройство |
|---|---|---|
| Основной выбор дня/часа | `bins/kronika-web/ui/src/hour-picker.tsx:98-147` | три кнопки prev/trigger/next + портал в `document.body` с сеткой дней `grid-cols-7` и сеткой часов `grid-cols-3` |
| Диалог экспорта From/To | `bins/kronika-web/ui/src/export-dialog.tsx:241-286` | кнопка дня + **текстовый** `<input>` без `type` (`inputMode="numeric"`) с ручным парсингом `HH:MM:SS` регуляркой (`export-time.ts:7-8`) |
| Календарь экспорта | `bins/kronika-web/ui/src/export-dialog.tsx:348-395` | ещё одна сетка кнопок |

Все `<input>` в UI: `type=text/password` (`login.tsx:64,68`), `type=search`
(`table-filter.tsx:72-90`), `type=checkbox` (`postgres-view.tsx:63`), `type=range`
(`uplot-chart.tsx:430-459`) и текстовый редактор часов экспорта. **[Код]**

Отказ от нативных контролов — **осознанное архитектурное решение, зафиксированное тестом**:
`tests/hour-picker.test.mjs:96-119` содержит
`assert.doesNotMatch(pickerSource, /<input\b|<select\b|type=["']date["']|type=["']time["']/)`,
а `tests/browser-corrections.mjs:81` проверяет, что внутри `hour-picker` нет `input, select`. **[Код]**

Причины такого решения видны из особенностей WebKit **[Спека]**:

| Нативный тип | Safari | Комментарий |
|---|---|---|
| `type=date` | 14.1+ | до 2021 года десктопный Safari вообще не показывал пикер |
| `type=time` | 14.1+ | своя реализация, не стилизуется |
| `type=datetime-local` | 14.1+ | **показывает пикер даты, но не времени** — [WebKit bug 261744](https://webkit.org/b/261744), время вводится вручную «by design» |
| `type=month`, `type=week` | **не поддерживается до сих пор** | тип распознаётся, контрола нет — [WebKit bug 200416](https://webkit.org/b/200416) |
| `type=color` | 12.1+ | — |

> Скорее всего, именно баг 261744 и имелся в виду под «в Safari не работает time picker».
> **К Kronika он отношения не имеет.** Если в Safari ломается выбор часа — ломается кастомный
> поповер, и искать надо в focus/pointer-логике, а не в `input[type=time]`.

Ложный след: в собранном `kronika-ui.html.gz` ровно 11 упоминаний `::-webkit-datetime-edit`
(вместе с `::-webkit-date-and-time-value`, `::-webkit-calendar-picker-indicator`) и строка
`datetime-local` внутри таблицы типов React DOM. Всё это приходит из **Tailwind 4 preflight**
и внутренностей React, а не из нашего кода. **[Код]**, пересчитано в §8.1.

**Рекомендация:** не «чинить» проблему переходом на `<input type="time">`. Это регресс:
микросекундная точность метода (`HOUR_MICROS`, `hour-picker.tsx:11`), переключение
Browser time / UTC (`display-time.ts`), ручное разрешение DST-неоднозначностей
(`export-time.ts:126-142`, включая 30-минутный переход `Australia/Lord_Howe`), RU/EN-локализация
и контроль стилей — всё это потеряется, а на Safari < 14.1 станет ещё хуже.

---

## 2. Что представляет собой веб-морда

- Единственный фронтенд: `bins/kronika-web/ui/` — 50 `.ts` + 38 `.tsx`. React 19.2.8,
  `@base-ui/react` 1.7.0 (только Select/Portal), `@tanstack/react-table`,
  `@tanstack/react-virtual`, `uplot` 1.6.32, `lucide-react`.
- Сборка: **esbuild 0.28.2 + Tailwind CSS 4.3.3** (`scripts/build.mjs`), TypeScript 7.0.2 только
  для `tsc --noEmit`. `target: ["es2022"]`, `format: "iife"`, `minify: true`, `drop: ["console"]`.
  Ни Vite, ни Babel/SWC, ни **`browserslist`**, ни полифилов (`core-js` отсутствует). **[Код]**
  IIFE означает: любое необработанное исключение на верхнем уровне убивает **весь** рендер.
- Результат — один самодостаточный `kronika-ui.html.gz` (527 796 байт gz / 1 583 337 байт raw,
  из них `<style>` = 285 330 символов), два инлайн-`<script>`, 8 шрифтов `data:font/woff2`.
  Раздаётся Rust-сервером (`bins/kronika-web/src/ui.rs:20`); CSP собирается в `build.rs:37-41`:
  `default-src 'none'; script-src 'sha256-...' 'sha256-...'; style-src 'unsafe-inline'; font-src data:; ...`
- Отдельный продукт: **офлайн HTML-отчёт** `crates/kronika-report` — другой шелл
  (`generator.rs:338-357`) с инлайн-base64 WASM (2,3 МБ `kronika-report-wasm.wasm.gz`) и
  распаковкой через `new DecompressionStream("gzip")` + `WebAssembly.compile` + `BigInt(...)`.
  Открывается с `file://` (`tests/report-transport.test.ts:29,114-115`). **[Код]**

---

## 3. Реальный минимальный Safari

### 3.1 Исправления к первой версии документа

Первая версия содержала несколько неточностей; после пересчёта по собранному `<style>`
и сверки с WebKit release notes они исправлены. **[Код] + [Спека]**

| Было | Стало | Почему |
|---|---|---|
| `oklab()` — 10 вхождений, минимум 15.4 | **Все 10 — внутри `color-mix(in oklab, …)`, минимум 16.4** | standalone `lab()/lch()/oklch()/light-dark()` = **0**; `oklab` внутри `color-mix` появился в 16.4, базовый `color-mix()` — в 16.2 |
| `color-mix()` — 72 вхождения | **54 в `<style>`, 60 по всему HTML** | прежнее число считалось по другому шаблону/всему файлу, включая JS-строки |
| `dvh/svh/lvh` — 23 вхождения | **17 `dvh` в `<style>` (16 реальных + 1 коллизия в base64 шрифта), 23 по HTML; `svh` = 1 коллизия в base64, `lvh` = 0** | реально используются только `100dvh` (14) и `100vh` (6) |
| «Реальный минимум Safari: 16.4» как жёсткий порог CSS | **Жёсткий CSS-порог 15.4**, официальный таргет Tailwind v4 и порог `@property` — 16.4 | 15.4 принёс `@layer`, `:has()`, `dvh`, `:focus-visible`, `<dialog>`/`::backdrop`, `accent-color`; ниже 15.4 отбрасывается `@layer theme` → все `var(--color-*)` невалидны |
| `:has()` — только косметика | **Ломает вёрстку на < 15.4** | `styles.css:675,753` — не-`forgiving` selector list: неизвестный `:has()` отбрасывает **всё правило**, включая «обычную» половину |
| `backdrop-filter` как риск | **Ложный след** | 2/1 вхождения — имена свойств внутри `transition-property` Tailwind, а не декларации |

### 3.2 Пересчитанная таблица порогов

Считается по подстрокам внутри собранного `<style>` (285 330 символов), если не указано иное.
Версии — MDN browser-compat-data (срез `main`, 2026-11-12), WebKit release notes и
[Tailwind Compatibility](https://tailwindcss.com/docs/compatibility). **[Код] + [Спека]**

| Фича | CSS / HTML | Где в проекте | Минимум Safari |
|---|---|---|---|
| `Object.hasOwn` | 0 / **59** (51 в исходниках) | `api.ts:1131,1600`, `postgres-metrics.ts`, `system-view.tsx` … | **15.4** |
| `Array.prototype.at` | 0 / 15 (14 в исходниках) | `api.ts:240,934,1574`, `export-dialog.tsx:131,360` … | **15.4** |
| `AbortSignal.throwIfAborted()` / `.reason` | 0 / 13 | `wire.ts:7,12,21,27`, `api.ts:548,603,649,783,1294` | **15.4** |
| `MediaQueryList.addEventListener("change")` | — | `entity-table.tsx:41-42`, uPlot на этапе загрузки бандла | **14** |
| `<dialog>.showModal()`, `::backdrop`, `:focus-visible` | 3 / 3 `::backdrop`, 11 `:focus-visible` | `mcp-connect.tsx:50`, `styles.css:391` | **15.4** |
| `HTMLElement.inert` | 16 в бандле | `timeline.tsx:247` | **15.5** |
| `@layer` | **5** | Tailwind 4 (`properties, theme, base, components, utilities`) | **15.4** |
| `:has(` | **14** | `styles.css:675,682,699,708-712,753` | **15.4** |
| `dvh` | **17** / 23 | `styles.css:415,554,560,791,792,857-858`, `.h-dvh` (`app.tsx:1227`) | **15.4** (корректно с 17.4) |
| `color-mix(` | **54** / 60 | `styles.css:405,679-680,704,708-714,723` | **16.2** |
| `oklab` (внутри `color-mix`) | **10** | собранный CSS | **16.4** |
| **`@property`** | **47** (все `--tw-*`) | Tailwind 4, вынесены в конец CSS вне `@layer` | **16.4** |
| **Tailwind CSS v4 (официальный таргет)** | — | `package.json:35` | **16.4** |
| **`DecompressionStream` / `new Response(stream)`** | — | отчёт `generator.rs:355` | **16.4** |
| Wasm `i64` ↔ JS `BigInt` | — | `generator.rs:357` (5-й аргумент `ReportSession`) | **15** |
| BigInt-литералы `10n`, `0n` | 9 упоминаний `BigInt` / 3 литерала | `search.ts:532`, `model.ts:211-212` | **14** (синтаксис → parse error) |
| `Intl`: `hourCycle`, `notation`, `formatToParts` | — | `display-time.ts:45-46`, `model.ts:205-207` | 14.1 / 14.1 / 11 |
| `scrollbar-width`, `scrollbar-gutter` | 4 / 14, 1 / 2 | `styles.css:634`, `detail.tsx:142` … | **18.2** (косметика) |
| `overscroll-behavior` | 1 | `styles.css:633` | 16 (частично) |
| `focus({preventScroll:true})` | 13 в бандле | `clipboard.ts:41,56,78`, `app.tsx:352` … | 15 |
| `Blob.stream()`, `ResizeObserver`, `PointerEvent` | — | `generator.rs:355`, `entity-table.tsx:273`, `hour-picker.tsx:61` | 14.1 / 13.1 / 13 |
| `Response.body` (streaming) | — | `wire.ts:18`, `export-download.ts:66` | 14.1 |
| `String.replaceAll`, `Object.fromEntries`, `queueMicrotask` | 1 / 30 / 1 | `search.ts:603`, `api.ts`, `table-filter.tsx:108` | 13.1 / 12.1 / 12.1 |
| CSP hash-source, `a[download]`, `flex gap` | — | `build.rs:27-41`, `export-download.ts:121` | 10 / 10.1 / 14.1 |

**Итоговая лестница порогов:**

1. **< 14** — белый экран уже на парсинге (BigInt-литералы) или на оценке бандла
   (uPlot `matchMedia().addEventListener`).
2. **< 15.4** — белый экран / нестилизованный интерфейс: `Object.hasOwn`, `.at()`,
   `throwIfAborted()`, `showModal()`, отброшенный `@layer theme`.
3. **15.4–16.3** — интерфейс жив, но частично деградирован: `@property`, `oklab` внутри
   `color-mix`, статические hex-фолбэки «заморожены» под тёмную тему, `::backdrop` без
   кастом-пропертей, баги `dvh` на iOS.
4. **16.4+** — заявленный минимум (iOS/iPadOS 16.4+, macOS 13.3+), включая офлайн-отчёт.
5. **17.4** — исправления `dvh`/`svh` и наследования custom properties в `::backdrop`;
   **18.2** — `scrollbar-width`/`scrollbar-gutter` (косметика).

Проект **нигде не декларирует** поддерживаемые браузеры: нет `browserslist`, в README/INSTALL
нет требований к браузеру, до этого документа слово «Safari» не встречалось ни в одном файле
репозитория. **[Код]**

---

## 4. Почему именно пикер ломается в Safari

### 4.0 Изолированное воспроизведение — **[Воспроизведено]**

Стенд `bins/kronika-web/ui/tests/safari-focus-probe.html` повторяет ровно три элемента
`hour-picker.tsx`: guard `onBlur` (`:101-104`), `pointerdown`-capture dismissal (`:51-54,61`)
и `requestAnimationFrame(...focus())` (`:50`). Портал эмулируется с учётом того, что **React 19
вешает `listenToAllSupportedEvents` на контейнер портала** (проверено в
`react-dom@19.2.8/cjs/react-dom-client.production.js`, commit-ветка `HostPortal`, `case 4`) —
поэтому `focusout` из поповера, лежащего в `document.body`, всё равно доходит до `onBlur`
корневого `div` через React-дерево. Клики настоящие (Playwright `page.click`).

| Шаг | Chrome 157 (Blink) | WebKit 26.0 (Playwright) |
|---|---|---|
| `activeElement` после клика по trigger | `trigger` | **`BODY`** |
| `activeElement` после rAF-фокуса | `hour` | `hour` |
| `focusout(hour)` | `relatedTarget = day` → **guard passes** | **`relatedTarget = null` → guard fails** |
| `CLOSED` до `click` | нет | **да** |
| `day-CLICK-FIRED` / `setDay()` | **1** | **0** |

Локально:

```bash
npx --yes playwright@1.56.1 install webkit
npm install --no-save playwright@1.56.1
node --input-type=module -e '
import { chromium, webkit } from "playwright";
import { pathToFileURL } from "node:url";
const url = pathToFileURL("bins/kronika-web/ui/tests/safari-focus-probe.html").href;
for (const [name, launch] of [
  ["chrome", () => chromium.launch({ channel: "chrome", headless: true })],
  ["webkit", () => webkit.launch({ headless: true })],
]) {
  const browser = await launch();
  const page = await browser.newPage();
  await page.goto(url);
  await page.click("#trigger");
  await page.waitForTimeout(150);
  const before = await page.evaluate(() => window.probeState());
  await page.click("#day");
  const after = await page.evaluate(() => window.probeState());
  console.log(name, "dayClicks=", after.dayClicks, "closes=", after.closeCount - before.closeCount);
  await browser.close();
}'
```

Ожидается `chrome dayClicks= 1 closes= 0` и `webkit dayClicks= 0 closes= 1`.

### 4.1 Воспроизведение на **боевом артефакте** — **[Воспроизведено]**

Чтобы исключить «стенд не соответствует приложению», собранный `kronika-ui.html.gz`
раздавался локальным HTTP-стендом с заголовками, повторяющими `ui.rs`: `Content-Encoding: gzip`,
`Content-Length`, `ETag`, `Cache-Control: private,no-cache`, `Vary: Authorization, Accept-Encoding`,
`nosniff` и **реальный CSP** с двумя вычисленными хешами инлайн-скриптов
(`sha256-uL7mM3b8Rz1DuRM+VinskiGIP6GGOILRbyUkWRfIo1w=` — theme-скрипт, 113 симв.;
`sha256-fpzmor9o7io4TWplrzh23FmsTlxW0HrH1mBtoH5WWrI=` — бандл, 1 251 965 симв.).
`/auth/session` → 204, `/api/*` → пустой NDJSON, `/api/instance-label` → JSON.
Движки: Chrome 157.0.8103.51 (headless) и WebKit 26.0 (`webkit-2215`,
UA `Version/26.0 Safari/605.1.15`), macOS 27.2.

**Оба движка** смонтировали приложение без единого `pageerror`, console error или
`requestfailed`: `#root` содержит 1 ребёнка, `[data-testid="hour-picker-trigger"]` на месте,
`.app-shell` имеет высоту `800px` (значит `100dvh` применился), `--color-s1` = `#f8fafb`
(значит `@layer theme` применился). То есть **CSP-хеши поверх gzip работают**, и на современном
WebKit «белого экрана» нет — все ранее описанные отказы версионные (§3.2).

Затем в обоих движках открывался настоящий поповер и кликалась **реальная** доступная
ячейка дня (`[data-testid="day-cell"]:not([disabled])`):

| | Chrome 157 (Blink) | WebKit 26.0 |
|---|---|---|
| Поповер открыт, кнопок внутри | 1 / 33 (2 доступны) | 1 / 33 (2 доступны) |
| `activeElement` после открытия | `hour-cell` | `hour-cell` |
| `focusout` при клике по ячейке дня | `hour-cell → day-cell` | **`hour-cell → null`** |
| Поповер после клика | **открыт (1)** | **закрыт (0)** |
| `activeElement` после клика | `day-cell` | **`BODY`** |
| Ошибки в консоли | нет | нет |

То есть P0-1 — **не артефакт стенда, а реальный дефект продакшен-бандла**. Важное уточнение:
rAF-фокус в **десктопном** WebKit 26.0 отрабатывает (`activeElement = hour-cell`), поэтому
проблема `focus()` вне потока клика — отдельный, именно **iOS**-специфичный риск (§4 P0-1,
JS-09). Механизм отказа при этом ровно тот же: клик по `<button>` в WebKit не перемещает фокус →
`relatedTarget === null` → guard проваливается → `setOpen(false)` размонтирует портал до `click`.

### P0-1. Focus-семантика WebKit — **[Воспроизведено]** (§4.0, §4.1)

WebKit отличается от Blink/Gecko тремя вещами **[Спека]**:

- клик по `<button>` / `<a>` **не передаёт фокус** (macOS Safari и iOS Safari, даже с `tabindex`);
- `element.focus()` **игнорируется**, если вызван не в том же синхронном потоке, что и клик
  (`setTimeout`, `requestAnimationFrame` после ре-рендера React) — на iOS;
- `blur`/`focusout` при клике по пустой области может не срабатывать, а `relatedTarget` часто `null`.

Попадания в коде:

```ts
// hour-picker.tsx:50 — паттерн «focus() в rAF», который iOS Safari игнорирует
const frame = requestAnimationFrame(() => hourCells.current[Math.max(0, selected)]?.focus())

// hour-picker.tsx:101-104 — закрытие по relatedTarget
onBlur={(event) => {
  if (event.relatedTarget instanceof Node && (root.current?.contains(event.relatedTarget)
      || popoverNode.current?.contains(event.relatedTarget))) return
  setOpen(false)
}}
```

Поповер порталится в `document.body` (`hour-picker.tsx:124,147`), то есть **находится вне `root`
в DOM**, но остаётся потомком корневого `div` в React-дереве — поэтому его `focusout` всё равно
вызывает `onBlur` корня. При `relatedTarget === null` условие проваливается и `setOpen(false)`
выполняется до `onClick` ячейки → «открылся и сразу закрылся» / «клик по дню ничего не делает».

**Диагностический маркер:** `export-dialog.tsx:363-370` (`DayPicker`) использует **только**
`pointerdown`-capture и не имеет `onBlur`. Если в Safari ломается верхний пикер дня/часа, а
календарь в диалоге экспорта работает — причина именно в `onBlur` + focus-семантике.

**Дополнительно (iOS):** позиционирование `hourPopoverPlacement` (`hour-picker.tsx:70-84,158-168`)
берёт `window.innerHeight` и `document.documentElement.clientWidth`. В iOS Safari это layout
viewport, а не visual viewport: при раскрытой нижней панели поповер уходит под тулбар, а
`max-h-[calc(100vh_-_20px)]` (`:124`) не спасает. Нужны `visualViewport` / `dvh` (§8.3).

**Фикс:** удалить `onBlur` у `hour-picker`; единственным источником закрытия оставить
`pointerdown` capture (`:61`) + `Escape` (`:55-60`) — как в `DayPicker`; фокусировать первую
ячейку синхронно в `useLayoutEffect`, не в rAF; для iOS считать высоту по
`window.visualViewport?.height ?? window.innerHeight`.

### P0-2. Синхронный `revokeObjectURL` ломает скачивание отчёта — **[Гипотеза]**, код **[Код]**

`export-download.ts:111-130`: `createObjectURL → link.click() → finally { link.remove(); revokeObjectURL() }`
в том же тике. В iOS Safari скачивание по blob URL через `a[download]` асинхронно, и немедленный
`revoke` отменяет загрузку; исторически iOS Safari ещё и игнорирует `download` для blob URL.
Симптом: диалог экспорта показывает `saved`, а файла нет. Усугубляется EXP-01 (activation
потрачен на `await`). **[Спека]**, требует проверки на устройстве.

### P1-1. `Intl.DateTimeFormat`: не совместимость, а корректность — **[Гипотеза]**, частично **[Опровергнуто в стенде]**

`display-time.ts:45` строит все метки через один форматтер с `hourCycle: "h23"`. Риск `h23` vs
`h24`: полночь `24:xx` вместо `00:xx` сдвинула бы `dayKey` (`:52`) и `monthKey` (`:82`), и часть
часов пропала бы из сетки — **виджет исправен, данные нет**.

**Прогон §13:** и Chrome 157, и WebKit 26.0 вернули для `Date.UTC(2026,0,1,0,0)` ровно `00`.
На современном WebKit расхождения нет; риск остаётся для старых Safari/ICU и нестандартных зон
(`resolvedOptions().timeZone` может вернуть `GMT+3` или пустоту). Юнит-тесты проекта гоняются в
Node/ICU, а не в WebKit — держать отдельный браузерный тест на `00:00`, `23:00–00:00`,
`America/Sao_Paulo`, `Australia/Lord_Howe`, `Pacific/Apia`, RU-локаль всё равно стоит.

### P1-2. Системная причина: Safari вообще никогда не тестировался — **[Код]**

- `scripts/report-browser-smoke.sh:18-28` ищет только `chromium* / google-chrome*`.
- Браузерные тесты (`tests/production-artifact.browser.mjs`, `browser-smoke.mjs`,
  `browser-corrections.mjs`) — самописный CDP поверх WebSocket к Chromium.
- Нет Playwright / Vitest / Jest / WebKit-таргета, нет `.browserslistrc`,
  `docs/releases.md:143` явно требует «Chromium/Google Chrome» для release-чека.
- Подробнее и с конкретными файлами — §12.

### P2. Косметика пикера — **[Спека]**

`scrollbar-width` (18.2+), `:has()`-подсветка строк, `color-mix`-тени, префлайт Tailwind с
`@supports (-webkit-hyphens:none)` (29 правил), `-webkit-appearance`. Ломают вид, не логику,
за исключением не-`forgiving` selector list (§8.2): при отсутствии `:has()` отбрасывается
**всё правило целиком**, а не одно свойство.

---

## 5. План проверки на реальном Safari (что headless не покрыл)

> Прогон §4.1/§13 закрыл «воспроизвести расхождение движков» и «проверить CSP + gzip».
> Ниже — то, что остаётся сделать на **реальном** Safari / iOS-устройстве.

1. **Зафиксировать симптом.** Точная версия Safari + macOS/iOS, скриншот, Console:
   `Object.hasOwn is not a function` → §3.2; `DecompressionStream` → §11; тишина +
   «открывается/закрывается» → P0-1; `saved`, но нет файла → §10.
2. **Проба в консоли реального Safari:**
   ```js
   ({
     ua: navigator.userAgent,
     hasOwn: typeof Object.hasOwn, at: typeof [].at,
     colorMix: CSS.supports("background", "color-mix(in srgb, red, blue)"),
     hasSelector: CSS.supports("selector(:has(*))"),
     decompression: typeof DecompressionStream,
     innerHeight: innerHeight, visualHeight: visualViewport?.height,
     midnight: new Intl.DateTimeFormat("en-US-u-ca-gregory-nu-latn",
       { hour: "2-digit", hourCycle: "h23", timeZone: "UTC" })
       .format(new Date(Date.UTC(2026, 0, 1, 0, 0)))
   })
   ```
   Затем открыть пикер и снять `document.activeElement` + `relatedTarget` в `focusout`.
3. **Второй браузер в CI.** `@playwright/test` + `npx playwright install webkit`, переиспользовать
   существующие `data-testid` (`hour-picker-trigger`, `hour-popover`, `day-cell`, `hour-cell`,
   `export-day-grid`). Важно: Playwright WebKit — это движок WebKit, **не Safari.app**; финальное
   подтверждение — реальный Safari. На этой машине macOS 27.2, Safari установлен,
   `/System/Cryptexes/App/usr/bin/safaridriver` доступен, но Remote Automation выключен —
   запустить `safaridriver -p 4445` не удалось (`Connection refused`), поэтому все выводы
   получены на Playwright WebKit.
4. **Только на устройстве:** iOS `focus()` в rAF, `a[download]` blob + user activation,
   `inputMode` и auto-zoom, `viewport-fit=cover` / safe area, `dvh` с раскрытым тулбаром,
   `localStorage` на `file://` в Safari.app, ITP и `SameSite=Strict` через 30 дней.
5. **Регрессионные кейсы:** открытие/закрытие поповера кликом вне, выбор дня и часа мышью,
   Arrow/Home/End, Escape, месяц вперёд/назад, `00:00` и DST `23:00–00:00`, RU-локаль,
   узкий viewport 320 px, iOS с раскрытым тулбаром, экспорт и получение файла.

---

## 6. Сводная матрица приоритетов (весь сайт)

| # | Приоритет | Где | Симптом | Статус |
|---|---|---|---|---|
| 1 | **P0** | `hour-picker.tsx:101-104,50` | пикер закрывается до `click`, выбор дня/часа не работает | **[Воспроизведено]** §4.0, §4.1 |
| 2 | **P0** | `export-dialog.tsx:201-205`, `export-download.ts:128` | на iOS отчёт не скачивается | [Гипотеза] §10 |
| 3 | **P0** | `entity-table.tsx:41-42`, uPlot, `Object.hasOwn`, `throwIfAborted`, `showModal` | белый экран на Safari < 15.4 | [Код]+[Спека] §7 |
| 4 | **P0** | `styles.css:675,753`, `@layer theme`, `.h-dvh` | нет стилей / схлопнувшийся shell на Safari < 15.4 | [Код]+[Спека] §8 |
| 5 | **P1** | `index.html:5` + 5 `env()` | на iOS контент под «челкой»/Home Indicator | [Код]+[Спека] §8.3 |
| 6 | **P1** | `export-dialog.tsx:263`, `table-filter.tsx:72`, `styles.css:828,452,493` | iOS auto-zoom, `:`, `HH:MM:SS` невозможно набрать | [Гипотеза] §8.4 |
| 7 | **P1** | `clipboard.ts:35,41,49` | всегда показывается «скопируйте вручную» | [Гипотеза] §10 |
| 8 | **P1** | `request.rs:55-56`, сервер без TLS | HEAD/Range → 405, `navigator.clipboard` недоступен по LAN | [Код]+[Спека] §9 |
| 9 | **P1** | `generator.rs:355-357` | офлайн-отчёт не открывается на Safari < 16.4 | [Код]+[Спека] §11 |
| 10 | **P1** | CI: `scripts/report-browser-smoke.sh`, `docs/releases.md:143` | ни один Safari-баг не воспроизводим в CI | [Код] §12 |
| 11 | **P1** | README / INSTALL / `browserslist` | поддерживаемые версии не задекларированы | [Код] §12 |
| 12 | **P2** | `scrollbar-*`, `::backdrop`, hex-фолбэки `color-mix`, sticky | косметика 16.2–18.1 | [Код]+[Спека] §8 |
| 13 | ~~P0~~ → **гигиена** | `ui.rs:61-62,91-94` | 304 с пустым телом + `Content-Length`/`Content-Encoding` | **[Опровергнуто в стенде]** §13.3 |
| 14 | ~~P0~~ → **гипотеза** | `app.tsx:187` и др., `file://` | `SecurityError` от `localStorage` | **[Опровергнуто в стенде]** §13.3 |

### 6.1 Минимальные правки (готовые фрагменты, не применялись)

```tsx
// 1) hour-picker.tsx — удалить onBlur, оставить pointerdown + Escape;
//    перенести фокус в useLayoutEffect, считать высоту по visualViewport
- onBlur={(event) => { if (event.relatedTarget instanceof Node && ...) return; setOpen(false) }}
- const frame = requestAnimationFrame(() => hourCells.current[...]?.focus())
+ useLayoutEffect(() => { if (open) hourCells.current[...]?.focus() }, [open])
+ const viewportHeight = window.visualViewport?.height ?? window.innerHeight
```

```html
<!-- 2) index.html:5 -->
- <meta name="viewport" content="width=device-width, initial-scale=1">
+ <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
```

```ts
// 3) entity-table.tsx:41-42 и uPlot — фолбэк для MediaQueryList Safari < 14
if (typeof media.addEventListener === "function") media.addEventListener("change", listener)
else media.addListener(listener)
```

```ts
// 4) export-download.ts — не отзывать URL в том же тике
- urlRef.revokeObjectURL(objectUrl)
+ setTimeout(() => urlRef.revokeObjectURL(objectUrl), 60_000)
```

```tsx
// 5) export-dialog.tsx:263 — цифровая клавиатура iOS не содержит ":"
- inputMode="numeric"
+ inputMode="tel"   // + font-size:16px, чтобы не было auto-zoom
```

```ts
// 6) единый safeStorage() вместо прямого localStorage в useState-инициализаторах
const safeStorage = (): Storage => {
  try { return window.localStorage } catch { return memoryStorageShim }
}
useState(() => loadDisplayTimeZone(safeStorage()))
```

```css
/* 7) styles.css:675,753 — разделить не-forgiving selector list */
.process-workspace > .lensbar > .lens-tabs { background: var(--color-s1); ... }
.pg-table-workspace > .lensbar:has(> .process-summary-inline) > .lens-tabs { ... }
/* 8) .h-dvh и 100dvh — фолбэк */
.app-shell { height: 100vh; height: 100dvh; }
```

```rust
// 9) ui.rs — гигиена 304 (не является подтверждённым Safari-багом, но корректнее)
if not_modified { headers.remove(CONTENT_LENGTH); headers.remove(CONTENT_ENCODING); }
```

**Чего не делать:** не заменять кастомный пикер на `<input type="time">` / `datetime-local`
(WebView-баг 261744, потеря микросекунд, DST-логики и тестового контракта); не добавлять
UA-sniffing под Safari — в бандле уже есть feature detection внутри `@base-ui`/floating-ui.

---

## 7. Полный аудит: JS / DOM / API

Идентификаторы `JS-*` используются в §6. **[Код]** = прочитано в исходниках/бандле.

### 7.1 Блокирующие (белый экран)

| ID | Находка | Порог |
|---|---|---|
| JS-01 | `Object.hasOwn` — 51 вхождение в исходниках / **59 в бандле** (8 из `@base-ui`/floating-ui), в т.ч. `api.ts:1131,1600` на основном пути загрузки. `esbuild target=es2022` даунлевелит только *синтаксис*, но не добавляет полифилы для *встроенных методов*. `format:"iife"` → любой `TypeError` роняет весь рендер | 15.4 |
| JS-02 | `Array.prototype.at` — 14 / 15, `api.ts:240,934,1574`, `export-dialog.tsx:131,360`, `timeline.tsx:439` | 15.4 |
| JS-03 | `AbortSignal.throwIfAborted()` + `.reason` — 16 вызовов / 13 в бандле: `wire.ts:7,12,21,27` (`reader.cancel(signal.reason)`), `api.ts:548,603,649,783,1294`, `report-transport.ts:99,105,111`. `readNdjson` (`wire.ts:1`, вызывается из `api.ts:1745`) — горячий путь **всех** NDJSON-загрузок → каждый запрос reject'ится | 15.4 |
| JS-04 | `MediaQueryList.addEventListener("change")` без фолбэка: `entity-table.tsx:41-42` в `useSyncExternalStore`-подписчике и **uPlot на этапе оценки бандла** (`gz@1005070` `Ld(e,t,a){t.addEventListener(e,a,n)}`, вызвается `Np&&oR()` при загрузке, `gz@1005110`). `addListener` в бандле = **0**. `uplot-chart.tsx:242,320` при этом `ResizeObserver` аккуратно проверяет `typeof` | 14 |
| JS-05 | `dialog.current?.showModal()` (`mcp-connect.tsx:50`), `.close()` (`:53`), `onCancel` (`:67`), `.mcp-dialog::backdrop` (`styles.css:391`) | 15.4 |
| JS-06 | BigInt-**литералы** `10n`, `0n` (`search.ts:532-533`, `model.ts:211-212`) — на Safari < 14 это **ошибка парсинга**, т.е. хуже, чем runtime-исключение | 14 |
| JS-07 | `String.replaceAll` (`search.ts:603`), `Object.fromEntries` (30), `queueMicrotask` (`table-filter.tsx:108`), `globalThis` — всё ниже заявленного порога, кроме `replaceAll` на пути парсинга поисковых выражений | 13.1 |
| JS-21 | `localStorage` на `file://`: геттер `window.localStorage` вычисляется **до** входа в `try/catch` helper'ов — `app.tsx:187`, `export-dialog.tsx:66`, `inspector.tsx:190`. Хелперы (`display-time.ts:31-37`, `export-range.ts:72-88`, `inspector.tsx:153-160`) защищают только `getItem`. **[Опровергнуто в стенде]** для WebKit 26.0 (§13.3), остаётся гипотезой для Safari.app | — |

### 7.2 Focus, диалоги, события

| ID | Находка |
|---|---|
| JS-08 | `focus({preventScroll:true})` — 10 в исходниках / 13 в бандле (`clipboard.ts:41,56,78`, `export-dialog.tsx:96`, `inspector.tsx:199,215`, `detail-dismiss.ts:31`, `mcp-connect.tsx:51,56`, `app.tsx:352`). До Safari 15 опции игнорируются без исключения → нежелательные скачки прокрутки |
| JS-09 | **Все** отложенные фокусы в `requestAnimationFrame`: `app.tsx:352`, `hour-picker.tsx:50`, `inspector.tsx:199,215`, `detail-dismiss.ts:30-31`, `mcp-connect.tsx:55-56`; плюс `queueMicrotask` в `table-filter.tsx:108`. На iOS Safari `focus()` вне синхронного потока клика игнорируется. **Уточнено §4.1:** в десктопном WebKit 26.0 rAF-фокус работает |
| JS-10 | `focusout.relatedTarget === null` — **[Воспроизведено]** для `hour-picker.tsx:101-104`; те же грабли в `clipboard.ts:75-77` (fallback «ручного копирования» самоудаляется по клику) и `help.tsx:137` (`onBlur={close}`, частично прикрыто `pinned`/`activeElement`). React 19 вешает слушатели на контейнеры порталов → `focusout` доходит до `onBlur` (§4.0) |
| JS-11 | Focus-trap'ы (`export-dialog.tsx:127-140`, `inspector.tsx:218-228`, `table-filter.tsx:163-172`) сравнивают `document.activeElement` с `querySelectorAll('button…, input…')`. В macOS Safari с выключенным Full Keyboard Access Tab **неходит по кнопкам** → ловушка не срабатывает, фокус уходит в тулбар |
| JS-12 | `inert={compactPicker}` (`timeline.tsx:247`) — 15.5; у `@base-ui` есть `data-base-ui-inert` + `aria-hidden` фолбэк, у собственного `inert` — нет |
| JS-13 | `pointerdown`-capture dismissal (`hour-picker.tsx:51,61`, `help.tsx:89-95`, `export-dialog.tsx:364-369`) — Pointer Events с 13; `mousedown`/`touchstart`-фолбэка нет |
| JS-14 | `inspector.tsx:235-245` `beginResize`: `pointermove` на `window` без `setPointerCapture` (0 в бандле) + `.inspector-splitter` без `touch-action:none` (`styles.css:570-576`) → на iOS приходит `pointercancel` и прокручивается страница вместо ресайза |

### 7.3 Сеть, формы, Intl

| ID | Находка |
|---|---|
| JS-15 | `Response.body.getReader()` (`wire.ts:18`, `export-download.ts:66`) — стриминг с 14.1; **уже смягчено**: оба места проверяют `response.body === null` и уходят в `text()` / `arrayBuffer()`. Теряется только прогресс |
| JS-16 | `ResizeObserver` без guard в `entity-table.tsx:273` (13.1), в `uplot-chart.tsx:242` — с guard |
| JS-17 | `inputMode="numeric"` на текстовом `HH:MM:SS` (`export-dialog.tsx:263`): цифровая клавиатура iOS не содержит `:` → **[Гипотеза]** время экспорта невозможно набрать на iPhone |
| JS-18 | `Intl.NumberFormat` с `notation` = `"compact"` / `"scientific"` (`model.ts:205-207,216`) — 14.1; до этого опция игнорируется → огромные числа ломают вёрстку. `display-time.ts:40` (`resolvedOptions().timeZone`, фолбэк на `"UTC"`); DST-зондирование `export-time.ts:131-134` через `getTimezoneOffset()` с шагом 15 мин на ±2 дня — **[Гипотеза]** расхождения кэширования TZ-правил в WebKit |
| JS-19 | `history.pushState` (`app.tsx:1025-1027`) — **[Гипотеза]** исторический rate-limit Safari ~100 вызовов / 30 с; ветка `dragging` уже использует `replaceState` |
| JS-20 | `type="search"` (`table-filter.tsx:90`) → Safari рисует `::-webkit-search-cancel-button`, перекрывающий свою кнопку; `scrollIntoView({block:"nearest"})` (`events-view.tsx:93`) — до 14 объект трактуется как `alignToTop=true`; `autoComplete="off"` на форме при `username`/`current-password` на полях (`login.tsx:61,64,68`) подавляет Safari AutoFill |

### 7.4 Подтверждённо отсутствует (риска нет)

`findLast`, `toSorted`/`toReversed`/`toSpliced`, `structuredClone`, `Promise.any`,
`Intl.Segmenter`/`ListFormat`/`PluralRules`, `WeakRef`, `FinalizationRegistry` (кроме wasm-bindgen),
`Error.cause`, `Object.groupBy`, `AbortSignal.timeout/any`, `requestIdleCallback`,
`setPointerCapture`, `OffscreenCanvas`, `EventSource`/`WebSocket`/`sendBeacon`,
`showSaveFilePicker`, Popover API (атрибут не используется; React 19 просто регистрирует
`beforetoggle`/`toggle`), `element.scrollTo`, `-webkit-overflow-scrolling`, CSS `prefers-color-scheme`,
нативные date/time-инпуты. `CSS.supports` в вендоре вызван с `typeof CSS<"u"` guard;
`visualViewport` в `@base-ui` — через optional chaining; `ResizeObserver`/`IntersectionObserver`
в вендоре — через `typeof`. **[Код]**

---

## 8. Полный аудит: CSS, вёрстка, iOS

### 8.1 Пересчитанный инвентарь собранного `<style>` **[Код]**

| Фича | CSS | HTML | комментарий |
|---|---|---|---|
| `@property` | 47 | 47 | все `--tw-*`, 43 `syntax:"*"`, 16 `initial-value`, `inherits:false`; идут **вне** `@layer` |
| `@layer` | 5 | 5 | `properties, theme, base, components, utilities` |
| `@supports` | 29 | 29 | 27 × `color:color-mix(in lab,red,red)`, 2 префлайт-правила |
| `color-mix(` | **54** | 60 | базовая поддержка 16.2 |
| `oklab` | 10 | 10 | **только** внутри `color-mix(in oklab, …)` → 16.4 |
| `:has(` / `:is(` / `:where(` / `:focus-visible` | 14 / 22 / 6 / 11 | 14 / 23 / 12 / 11 | selector list не-`forgiving` |
| `dvh` / `100dvh` / `100vh` | 17 / 14 / 6 | 23 / 19 / 9 | 1 `dvh` и 1 `svh` — коллизии в base64 шрифтов |
| `env(` | 5 | 5 | все `env(safe-area-inset-*)` |
| `scrollbar-width` / `scrollbar-gutter` | 4 / 1 | 14 / 2 | 18.2; `gutter` — no-op при overlay-скроллбарах |
| `::backdrop` / `::-webkit-datetime-edit` | 3 / 11 | 3 / 11 | префлайт + `.mcp-dialog` |
| `backdrop-filter` / `-webkit-backdrop-filter` | 2 / 1 | 4 / 3 | **имена в `transition-property`**, не декларации |
| `@font-face` / `@media` / `@keyframes` | 8 / 49 / 3 | 8 / 49 / 3 | `data:font/woff2`, `font-display:swap`, `unicode-range` |
| `touch-action` / `overscroll-behavior` / `overflow-anchor` | 1 / 1 / 1 | 1 / 1 / 1 | — |
| `-webkit-overflow-scrolling` | **0** | 0 | с iOS 13 momentum по умолчанию |
| Zero: `@container`, `@starting-style`, `field-sizing`, `text-wrap:balance`, `subgrid`, `aspect-ratio`, `scrollbar-color`, `forced-colors`, `@scope`, `@view-transition` | 0 | 0 | риска нет |

**Нативной CSS-вложенности** в артефакте нет: `&` в исходнике (`styles.css:119,138,142-145,152,165,178-181`)
находится внутри `@utility` и полностью разворачивается Lightning CSS; 193 `&` в бандле —
экранированные имена arbitrary-variant классов. **[Код]**

### 8.2 Правила, которые отбрасываются целиком **[Код] + [Спека]**

- `styles.css:674-675` и `:752-753`: в одном selector list обычный селектор и `:has()`.
  На Safari < 15.4 неизвестный `:has()` **убивает всё правило** → у `.process-workspace` тоже
  пропадают `background`, `border`, `border-radius`, `padding`. Это уже не косметика.
- `styles.css:575-576` и `:667-668`: `:hover` + `:focus-visible` в одном списке → до 15.4
  пропадает и hover-половина (косметика).
- `@layer theme { :root,:host { --color-*, --text-*, --font-*, --spacing } }`: до 15.4 слой
  отбрасывается → **все** `var(--color-*)` невалидны → нестилизованное приложение.
  Tailwind-слой `properties` с `@supports` компенсирует отсутствие `@property`, только если
  сам `@layer` работает. Это и есть жёсткий CSS-порог 15.4.
- **Статические hex-фолбэки `color-mix` заморожены под тёмную тему** **[Код]**: Lightning CSS
  кладёт hex перед каждым `@supports`-вариантом (`styles.css:405 → #dfa73f73`, `:679 → #dfa73f1a`,
  `:680 → #ef65771f`, `:704 → #191b1a`, `:708-712 → #172221/#21211c/#271c23`,
  `:723 → #03060957`). Значения посчитаны из тёмного `@theme` и **игнорируют
  `[data-theme=light]`** (`:323-377`) → на Safari 16.2–16.3 тёмные заплатки в светлой теме.
  `styles.css:714` `background:currentColor` как фолбэк для
  `color-mix(in srgb, currentColor 5%, transparent)` (нужен 16.4) даёт **сплошной** фон.
- `styles.css:391` `.mcp-dialog::backdrop{background:var(--color-shadow-a)}` — WebKit
  наследует custom properties в `::backdrop` только с **17.4**; ниже `var()` guaranteed-invalid →
  прозрачный scrim под `showModal()`. **[Гипотеза]** для совсем старых версий: префлайт
  `*,:after,:before,::backdrop{…}` может быть отброшен целиком.
- `styles.css:387` `html{overflow-anchor:none}` — no-op, Safari scroll anchoring не реализовывал.

### 8.3 iOS: viewport и safe area **[Код] + [Спека]**

- `index.html:5` — `<meta name="viewport" content="width=device-width, initial-scale=1">`,
  **нет `viewport-fit=cover`**. WebKit возвращает `0` для `env(safe-area-inset-*)` без него,
  поэтому все 5 использований мертвы: `styles.css:415` (`padding-bottom`),
  `:821`, `:864` (`max(6px, env(...))` — здесь есть фолбэк), `:865`. Нижний `.export-dialog`,
  мобильный поиск и `.inspector` уходят под Home Indicator / «челку».
  **Прогон §13:** `env(safe-area-inset-bottom, 42px)` дал `0px` в **обоих** движках — `env()`
  поддерживается, но разрешается в ноль; на iOS без `viewport-fit=cover` ненулевым не станет.
- `.h-dvh` (`app.tsx:1227`) компилируется в `height:100dvh` **без фолбэка `100vh`**;
  единственная страховка — `body{min-height:100vh}` (`styles.css:388`). До 15.4 декларация
  отбрасывается и `overflow:hidden`-shell схлопывается. На iOS 15.4–17.3 — известные баги
  `dvh`/`svh` (исправлены в 17.4).
- Портальные поповеры используют `max-h-[calc(100vh_-_16px)]` (`help.tsx:149`),
  `max-h-[calc(100vh_-_20px)]` (`hour-picker.tsx:124`), `top-0 bottom-0` (`help.tsx:215`):
  `100vh` на iOS = large viewport → обрезка под тулбаром; `hourPopoverPlacement`
  (`hour-picker.tsx:160-168`) берёт `window.innerHeight`, а не `visualViewport.height`.
- Все поповеры порталятся в `document.body` (`activity.tsx:217`, `export-dialog.tsx:345`,
  `help.tsx:147`, `hour-picker.tsx:124`, `table-filter.tsx:108`, `timezone-select.tsx:24`,
  `inspector.tsx:109`) → `position:fixed` вырывается из `.app-shell{overflow:hidden}`;
  `transform`/`filter`/`will-change` на этом пути нет → ловушки containing block нет.
  Исключение: `.mobile-panel-head` (`styles.css:810`) и `[data-search-surface]` (`:822`) не
  порталятся — **[Гипотеза]** латентный риск при будущем `transform` на `.app-shell`.
- `overscroll-behavior:contain` только в `styles.css:633`; остальные `overflow:auto`-контейнеры
  на iOS < 16 «пружинят» цепочкой. `touch-action:none` (`.touch-none`, uPlot) блокирует pinch
  на графике; `-webkit-tap-highlight-color:transparent` убирает отклик на тап.

### 8.4 iOS: auto-zoom инпутов **[Код] + [Спека]**

В собранном CSS значения `font-size` — 9/10/11/12/13 px и `var(--text-*)`; **16 px нет**.
iOS Safari автоматически зумит при фокусе `<input>`/`<select>` с computed font-size < 16 px
и не возвращает масштаб. Затронуты: `export-dialog.tsx:256-264` `.export-clock … text-sm` (12 px),
`table-filter.tsx:72-77` `text-sm` (12 px), `styles.css:828` мобильный поиск (13 px),
`styles.css:452,493` нативные `<select>` (11 px). `login.tsx:64,68` наследуют UA 16 px — безопасны.
Совмещается с JS-17 (`inputMode="numeric"` без `:`). **[Гипотеза]** — требуется устройство.

### 8.5 Прочее **[Код]**

- `styles.css:152` `.pg-stretch{overflow:hidden}` + sticky `.entity-head` внутри `overflow:auto`
  (`entity-table.tsx:295`) и виртуальные строки с `transform:translateY` (`:346`) — **[Гипотеза]**
  кваirk WebKit: шапка уезжает или перерисовывается поверх.
- Тема: `index.html:6` `<meta name="color-scheme" content="dark light">`, но
  `styles.css:319-321` `:root,[data-theme="dark"]{color-scheme:dark}` — `:root` **всегда тёмный**,
  переключение делает JS (`app.tsx:355,1444` + инлайн-скрипт `index.html:9`). Без/до JS
  нативные select, скроллбары, `accent-color` и `::backdrop` тёмные на светлой ОС.
  CSS-`prefers-color-scheme` = 0. **[Гипотеза]**, косметика.
- 8 `@font-face` с `data:font/woff2;base64` и `font-display:swap` без gate на
  `document.fonts.ready`: `uplot-chart.tsx:260-262` и `entity-table.tsx:273` могут замерить
  метрики фолбэка до подмены шрифта. **[Гипотеза]**, косметика. CSP `font-src data:` это
  разрешает; **жёсткого CSP-фейла нет** (подтверждено прогоном §13).
- `@custom-variant coarse` (`styles.css:98`) + `entity-table.tsx:30`
  `matchMedia("(hover: none), (pointer: coarse)")`: iPad с трекпадом отдаёт `hover:hover` →
  28 px таргеты вместо 44 px. **[Гипотеза]**.

---

## 9. Полный аудит: сервер, CSP, транспорт, авторизация

### 9.1 CSP **[Код] + [Спека]**

`build.rs:27-41` хеширует **два** инлайн-скрипта после распаковки gzip и выдаёт
`default-src 'none'; script-src 'sha256-…' 'sha256-…'; style-src 'unsafe-inline'; font-src data:;
img-src data:; connect-src 'self'; base-uri 'none'; form-action 'none';
frame-ancestors 'none'; object-src 'none'`. WebKit поддерживает CSP2 hash-source с Safari 10;
gzip распаковывается до хеширования → **сочетание «хеш + gzip» корректно**.
**[Воспроизведено] §13:** с этими точными хешами оба движка исполнили оба скрипта,
нарушений CSP в консоли нет. Рисков здесь нет.

- `script-src` не содержит `'wasm-unsafe-eval'` → при раздаче отчёта под этим CSP
  `WebAssembly.compile` был бы заблокирован. Фактически у серверного UI WASM нет (0 упоминаний),
  а у офлайн-шелла **CSP нет вообще** (`kronika-report-shell.html.gz`), поэтому блокировки нет.
- `form-action 'none'` при `event.preventDefault()` (`login.tsx:21`) навигации не даёт, но
  **[Гипотеза]** Safari AutoFill может инициировать нативный submit и породить
  `securitypolicyviolation`.
- `index.html:7` SVG-favicon (`img-src data:` разрешает) — Safari не рисует SVG-фавиконки,
  нужен `apple-touch-icon` PNG. Косметика.
- Инлайн-скрипт темы `index.html:9` уже обёрнут в `try/catch` — грамотно переживает
  `SecurityError` на непрозрачных origin'ах.

### 9.2 HTTP / кэш **[Код]**

| ID | Находка |
|---|---|
| SRV-01 | `ui.rs:61-62` для HEAD **или** `not_modified` создаёт `Full::new(Bytes::new())`, при этом `:91-94` безусловно ставит `Content-Encoding: gzip` и `Content-Length` **от gz-представления**. 304 = пустое тело + заявленная длина 527 796 + `gzip`. Тест `src/tests/ui.rs:95-170` это явно ассертит. **[Опровергнуто в стенде]** как причина отказа (§13.3): RFC 9110 §§8.4/8.6 такие заголовки на 304 **допускают**, а контрольный прогон без них дал идентичный результат. Оставляем как гигиену |
| SRV-02 | `build.rs:24-26` даёт **разные сильные ETag** для gzip/identity, `Vary: Authorization, Accept-Encoding` (`ui.rs:28`) — это правильная защита от классического «перепутанного» кэша WebKit. Рекомендация: добавить `Vary: Cookie` / `no-store` на HTML, т.к. состояние авторизации влияет на представление |
| SRV-03 | `Cache-Control: private,no-cache` (`ui.rs:96`) допускает bfcache с устаревшим HTML после обновления сервера; `?build=` есть только у API-URL. **[Код]** |
| SRV-04 | `request.rs:55-56` — **только GET**, HEAD и `Range` на `/api/*` (включая `/api/export`) → 405; `Accept-Ranges` нигде нет. **[Спека]** iOS Safari download manager / Quick Look использует HEAD + докачку → отказ |
| SRV-05 | `streaming.rs:114-120` тоже отдаёт 304 с пустым телом; `nosniff` стоит только на UI (`ui.rs:98`), а не на API/NDJSON (`response.rs:27-34`) |
| SRV-06 | `encoding.rs:59-65` корректно выбирает gzip из `Accept-Encoding: gzip, deflate, br`; `WWW-Authenticate: Basic` подавляется при `X-Kronika-UI: 1` (`session.ts:59,84`), что **убирает модальное окно 401 Safari** — уже реализованная удачная mitigation |

**Прогон §13 (кэш):** ни Chrome 157, ни WebKit 26.0 **не отправили `If-None-Match`** при
`page.reload()` с `private,no-cache`, `private,max-age=0` и `private,max-age=60`. То есть в
headless-стенде естественный conditional-путь не воспроизводится вообще; все 304 в прогоне
были форсированы заголовком. Это важно: делать выводы о поведении Safari при релоаде по этому
стенду нельзя.

### 9.3 Контекст, куки, логин **[Код] + [Спека]**

- `server.rs:36` — HTTP/1.1, `INSTALL.md:140` / `bins/kronika-web/README.md:24` предлагают
  `http://<server-ip>:8080`. Для Safari это **не secure context**: `navigator.clipboard` и
  `crypto.subtle` недоступны. (В стэнде на `127.0.0.1` `navigator.clipboard` был `object` —
  loopback считается безопасным, LAN-IP — нет.) Самоподписанный сертификат iOS Safari
  отклоняет без обхода; нужен прокси с доверенным сертификатом.
- `auth.rs:165-168`: `{value}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000[; Secure]`.
  `Secure` ставится только при https-`Origin` (`request.rs:222-239`). ITP вычищает
  first-party данные через ~30 дней без взаимодействия, а `SameSite=Strict` не отдаёт куку при
  переходе с внешнего сайта (ссылка из почты/чата) → ложный re-login. Рекомендация: `Lax`.
- `request.rs:222-226` требует `Origin` на POST/DELETE `/auth/session`; отсутствие / `null` /
  дубли → 400 `invalid_origin`. **[Гипотеза]** прокси или приватный режим могут вырезать заголовок.
- `login.tsx:61,64,68,71`: `<form autoComplete="off">` конфликтует с `username` /
  `current-password`, нет `action`/`method`, логин идёт через `fetch` (`session.ts:44`) →
  Safari AutoFill и сохранение пароля подавлены. `autoFocus` на iOS игнорируется.
- `session.ts:96-98`: `btoa(String.fromCharCode(...new TextEncoder().encode(...)))` — UTF-8
  байты против ISO-8859-1 нативного Basic → **[Гипотеза]** расхождение для не-ASCII логинов.

---

## 10. Полный аудит: экспорт, скачивание, буфер обмена

| ID | Находка | Статус |
|---|---|---|
| EXP-01 | `export-dialog.tsx:201-205`: `const artifact = await fetchExportArtifact(...)`, и только потом `triggerHtmlDownload(...)`. **User activation потребляется `await`**; iOS Safari требует скачивание внутри жеста. Независимо от EXP-02 | [Код]+[Спека], **[Гипотеза]** на устройстве |
| EXP-02 | `export-download.ts:111-130`: `createObjectURL → link.click() → finally { link.remove(); revokeObjectURL() }` — синхронный revoke в том же тике; iOS Safari асинхронно обрабатывает blob-`download` и исторически игнорирует имя файла («Unknown») | [Код]+[Спека], **[Гипотеза]** |
| EXP-03 | `export-download.ts:46-48,60-82`: клиент сам собирает `new Blob(chunks)` из `getReader()`, **отбрасывая** нативную обработку `Content-Disposition: attachment` (`export.rs:282`). Весь отчёт держится в памяти → риск OOM-kill на iOS | [Код] |
| EXP-04 | `export-download.ts:100-108` парсит `filename*=UTF-8''…`, но сервер отдаёт только ASCII `kronika-%Y-%m-%d-%H%M%S-…-utc.html` (`export.rs:265-268`) → ветка мертва, риска кодировки нет | [Код] |
| EXP-05 | `clipboard.ts:11-13` `navigator.clipboard.writeText` — secure context + user gesture; на `http://LAN` его нет, срабатывает фолбэк (так и задумано) | [Код]+[Спека] |
| EXP-06 | `clipboard.ts:35` `exactCopyEvent = event.defaultPrevented && event.clipboardData.getData("text/plain") === text`: WebKit **[Гипотеза]** возвращает `""` при чтении `clipboardData` внутри `copy` → даже удачный `document.execCommand("copy")` (`:49`) признаётся неудачей → **всегда** показывается «скопируйте вручную» | [Код]+[Гипотеза] |
| EXP-07 | `clipboard.ts:21-27,41`: невидимый `opacity:0; pointerEvents:none; readOnly` textarea + `focus({preventScroll:true})` + `setSelectionRange`. **[Гипотеза]** macOS Safari требует видимое выделение для `execCommand("copy")` | [Код]+[Гипотеза] |

---

## 11. Полный аудит: офлайн HTML-отчёт

Отдельный шелл `crates/kronika-report/assets/kronika-report-shell.html.gz` (505 КБ gz) — это
тот же UI-бандл плюс инлайн-WASM-рантайм. Открывается с `file://`.

| ID | Находка | Порог |
|---|---|---|
| RPT-01 | `generator.rs:355-357`: `new Blob([g]).stream().pipeThrough(new DecompressionStream("gzip"))` → `new Response(stream).arrayBuffer()` → `WebAssembly.compile(r)` → `initEmbedded(m)`. `DecompressionStream` и `new Response(stream)` — **16.4**; `Blob.stream()` 14.1, `WebAssembly.compile` 11. На Safari < 16.4 — пустая страница с `DecompressionStream is not defined` | **16.4** |
| RPT-02 | `BigInt("…")` как 5-й аргумент `ReportSession` — wasm-bindgen мапит Rust `i64` → JS BigInt, нужна **Wasm i64↔BigInt интеграция = Safari 15** (не путать с самим BigInt = 14) | 15 |
| RPT-03 | `generator.rs:18-26,345-351`: WASM 2,3 МБ gz + ZMS + IDX инлайнятся base64 (`BASE64_INPUT_BYTES = 12*1024`); `atob()` на мегабайтных строках + потолок памяти iOS-таба → **[Гипотеза]** отказ/убийство на больших отчётах. Используется `WebAssembly.compile` из `Uint8Array`, а не `compileStreaming` — `fetch` не нужен, `file://` в принципе работает | — |
| RPT-04 | Непрозрачный origin `file://`: `window.localStorage` в Safari historically кидает `SecurityError`. Геттер вызывается **до** входа в `try/catch` helper'ов: `app.tsx:187` `useState(() => loadDisplayTimeZone(localStorage))`, `export-dialog.tsx:66`, `inspector.tsx:190`. **[Опровергнуто в стенде §13.3]** для Playwright WebKit 26.0; остаётся гипотезой для Safari.app. Защитный `safeStorage()` всё равно стоит добавить | — |
| RPT-05 | В шелле **нет CSP** (0 `content-security-policy`) → WASM ничем не блокируется, но и защита отсутствует. `style-src`-подобных ограничений нет | — |
| RPT-06 | Сетевых вызовов в report-режиме нет: `report-transport.ts:26` использует `https://kronika-report.invalid/` только как базу для `new URL`, `report-session.ts:21-23` подменяет `apiFetch`, `build.mjs:225-246` явно запрещает `fetch(` в report-бандле → CORS/`file://`-ограничения не применяются. **[Код]**, риска нет | — |
| RPT-07 | Закоммиченный `.gz` — **шаблон** с неподставленными плейсхолдерами. **[Воспроизведено §13.3]:** при открытии напрямую оба движка кидают `Error: Kronika report runtime is missing`, `#root` пуст. Для тестов нужен сгенерированный отчёт, а не сырой ассет | — |

---

## 12. Полный аудит: процесс, CI, документация

| ID | Находка |
|---|---|
| PRC-01 | Chromium-only: `scripts/report-browser-smoke.sh:18-30` ищет только `chromium*`/`google-chrome*` (и использует `setsid`, которого нет на macOS); `.github/workflows/ci.yml:132`; `tests/browser-smoke.mjs:13`; `tests/production-artifact.browser.mjs:8440-8466`; `docs/releases.md:143` требует «Chromium/Google Chrome». **Ни одного macOS/WebKit runner'а** |
| PRC-02 | Нет `browserslist`; `build.mjs:110` `target:["es2022"]` и `package.json:16-18` (`node>=22`) ограничивают только тулинг. Ни `README.md`, ни `INSTALL.md`, ни `docs/build.md`, ни `bins/kronika-web/README.md` не упоминают Safari. Поддерживаемая матрица не задекларирована |
| PRC-03 | `tests/safari-focus-probe.html` — ручной стенд, не входит в `npm test` (`package.json:10` glob). Его надо либо подключить в CI, либо заменить `@playwright/test`-спекой на реальном артефакте |
| PRC-04 | `.github/workflows/ci.yml:25,83` / `docs/build.md:15` фиксируют Node 22.21.1; `Makefile:31-41` `ui-check`/`report-assets-check` проверяют байт-в-байт воспроизводимость артефактов, но **никогда не исполняют их в WebKit** |
| PRC-05 | Реальный Safari.app в автоматизации не участвует: `safaridriver` есть, но Remote Automation отключён (`Connection refused` на `:4445`); Playwright WebKit ≠ Safari.app |

**Процессный вывод:** любое утверждение «в Safari не работает» сегодня невозможно ни
подтвердить, ни защитить от регрессии в CI. Именно поэтому исследование и существует в виде
отдельной ветки, а не в виде регрессионного теста.

---

## 13. Итоги эмпирического прогона боевого артефакта

Стенд: Node HTTP-сервер с заголовками `ui.rs`, CSP с реальными хешами, gzip-тело;
Chrome 157.0.8103.51 и WebKit 26.0 (`webkit-2215`), macOS 27.2. Скрипты — `/tmp/kronika-safari-probe/run-artifact*.mjs`
(в репозиторий не коммитятся, чтобы не тянуть `playwright` в зависимости проекта).

### 13.1 Что подтвердилось

| Проверка | Chrome 157 | WebKit 26.0 | Вывод |
|---|---|---|---|
| 200 + `Content-Encoding: gzip` + CSP hash-source | рендер, 0 ошибок | рендер, 0 ошибок | CSP + gzip **не проблема** |
| `#root` / `hour-picker-trigger` / `.app-shell` | 1 / есть / `800px` | 1 / есть / `800px` | `100dvh` и `@layer theme` применяются |
| `--color-s1` | `#f8fafb` | `#f8fafb` | слой `theme` не отброшен |
| `Object.hasOwn`, `.at`, `throwIfAborted`, `signal.reason` | function / function / function / true | то же | современный WebKit закрывает ES2022-порог |
| `CSS.supports`: `color-mix` srgb/oklab, `:has`, `dvh`, `scrollbar-width`, `scrollbar-gutter`, `:focus-visible` | все true | все true | пороги 16.2/16.4/15.4/18.2 пройдены |
| `DecompressionStream`, `showModal`, `inert`, `visualViewport`, `navigator.clipboard` | есть | есть | — |
| Полночь `hourCycle:"h23"` | `00` | `00` | P1-1 на современном WebKit **не воспроизводится** |
| `env(safe-area-inset-bottom, 42px)` | `0px` | `0px` | `env()` разрешается в ноль → нужен `viewport-fit=cover` |
| **Клик по ячейке дня в реальном поповере** | `focusout → day-cell`, поповер открыт | **`focusout → null`, поповер закрыт** | **P0-1 подтверждён на бандле** |
| rAF-фокус при открытии | `hour-cell` | `hour-cell` | дефект `focus()` вне клика — **iOS-специфичен** |

### 13.2 Что не удалось проверить в headless

Естественный conditional reload (`If-None-Match` не отправлялся ни при `no-cache`, ни при
`max-age=0/60`), реальные жесты iOS, `a[download]`, ITP/куки, auto-zoom, `visualViewport` с
раскрытым тулбаром, Safari.app (Remote Automation выключен), Gecko (движок Playwright Firefox
не установлен; попытка `playwright install firefox` и прямого запуска `/Applications/Firefox.app`
не увенчалась успехом — вывод о Firefox основан на спеке и наблюдении пользователя).

### 13.3 Что опровергнуто или ослаблено — важно не тащить в план как «железный баг»

1. **304 + `Content-Length` + `Content-Encoding: gzip` (SRV-01).** При форсированном
   conditional-запросе WebKit 26.0 показал пустой документ длиной 39 байт, Chrome —
   `net::ERR_ABORTED`. Контрольный прогон **без** этих двух заголовков дал **идентичный**
   результат (`webkit:buggy304` = `webkit:clean304` = 39 байт; `chrome` = `ERR_ABORTED` в обоих).
   Значит отказ вызван самим форсированным 304 без валидной кэш-записи, а не заголовками.
   RFC 9110 §§8.4, 8.6 такие заголовки на 304 **разрешают** (при условии, что `Content-Length`
   равен длине тела гипотетического 200). **Статус: гигиена + гипотеза для реального Safari,
   не P0-хардфейл.** Убрать заголовки на 304 всё равно стоит.
2. **`SecurityError` от `localStorage` на `file://` (RPT-04 / JS-21).** Прямая проверка
   `localStorage.getItem("x")` на `file://` вернула `ok:object` **в обоих** движках.
   Safari.app может отличаться, но в Playwright WebKit 26.0 отказа нет. Дополнительно:
   закоммиченный report-shell — шаблон и падает раньше (`Kronika report runtime is missing`),
   поэтому путь React с `useState`-инициализатором в этом прогоне не исполнялся вообще.
   **Статус: гипотеза, требующая сгенерированного отчёта + реального Safari.**
3. **`oklab()` как самостоятельная фича с порогом 15.4** — исправлено в §3.1: 10 вхождений
   исключительно внутри `color-mix(in oklab, …)`, порог 16.4.
4. **`svh`/`lvh`, 72 `color-mix`, 23 `dvh`** — исправлено в §3.1/§8.1 точным пересчётом.
5. **`backdrop-filter` как риск** — это имена свойств в `transition-property`, деклараций нет.

---

## 14. Источники

- Собственный код: `bins/kronika-web/ui/src/**`, `scripts/build.mjs`,
  `bins/kronika-web/{build.rs,src/ui.rs,src/request.rs,src/auth.rs,src/encoding.rs,src/response.rs,src/streaming.rs,src/export.rs,src/server.rs}`,
  `crates/kronika-report/src/generator.rs`, `scripts/report-browser-smoke.sh`,
  `docs/releases.md`, `.github/workflows/ci.yml`, `Makefile`.
- Собранные артефакты (декомпрессия + подсчёт подстрок): `bins/kronika-web/ui/kronika-ui.html.gz`
  (1 583 337 байт, `<style>` 285 330 симв.) и
  `crates/kronika-report/assets/kronika-report-shell.html.gz`.
- MDN browser-compat-data (`main`, срез 2026-11-12): `api/DecompressionStream.json`,
  `api/AbortSignal.json`, `api/HTMLDialogElement.json`, `api/MediaQueryList.json`,
  `api/ResizeObserver.json`, `api/Clipboard.json`, `api/Navigator.json`, `api/PointerEvent.json`,
  `api/Blob.json`, `css/at-rules/property.json`, `css/at-rules/layer.json`,
  `css/selectors/has.json`, `css/types/color.json`, `css/types/length.json`,
  `css/properties/{gap,scrollbar-width,scrollbar-gutter,overscroll-behavior,accent-color}.json`,
  `javascript/builtins/{Array,Object,BigInt,String}.json`,
  `html/elements/input/{date,time,datetime-local,month,week}.json`, `html/elements/a.json`.
- WebKit release notes: Safari 15.4 (`:has()`, `@layer`, `dvh`, `:focus-visible`, `<dialog>`),
  16.2 (`color-mix()`), 16.4 (`@property`, `color-mix(in oklab, currentColor …)`),
  17.4 (`::backdrop` custom properties, `dvh`/`svh`), 18.2 (`scrollbar-width`/`scrollbar-gutter`).
- [Tailwind CSS v4 — Compatibility](https://tailwindcss.com/docs/compatibility): Chrome 111 / **Safari 16.4** / Firefox 128.
- [WebKit bug 261744](https://webkit.org/b/261744) — `datetime-local` без time picker;
  [WebKit bug 200416](https://webkit.org/b/200416) — `month`/`week`.
- [张鑫旭 · 细说 iOS Safari 下 focus 的行为](https://www.zhangxinxu.com/wordpress/2020/10/ios-safari-input-button-focus/) —
  кнопки не получают фокус по клику, `focus()` вне потока клика не работает, `blur` не срабатывает.
- `react-dom@19.2.8/cjs/react-dom-client.production.js` — `listenToAllSupportedEvents(workInProgress.stateNode.containerInfo)`
  в commit-ветке `HostPortal`: порталы получают собственные слушатели, поэтому `focusout` из
  `document.body` доходит до `onBlur` корневого `div` через React-дерево.
- RFC 9110 §§8.4, 8.6 — `Content-Encoding` и `Content-Length` в ответе 304.
- Playwright 1.56.1: Chrome 157.0.8103.51 (Blink, `channel: "chrome"`), WebKit 26.0
  (`webkit-2215`, headless), реальные `page.click`. Стенды:
  `bins/kronika-web/ui/tests/safari-focus-probe.html` (закоммичен) и
  `/tmp/kronika-safari-probe/run-artifact.mjs`, `run-artifact2.mjs`, `run-304-control.mjs` (локальные).
