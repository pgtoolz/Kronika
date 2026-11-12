# Совместимость веб-интерфейса с Safari: исследование

Ветка: `research/safari-compat` (от `main` @ `592a5e5`). Дата: 2026-11-12.

Статус: **исследование, без изменений в коде**. Выводы разделены на четыре уровня достоверности:
**[Код]** — подтверждено чтением исходников; **[Спека]** — подтверждено внешним источником
(MDN browser-compat-data, документация Tailwind, WebKit bug tracker, исходники React 19);
**[Воспроизведено]** — подтверждено прогоном в двух движках (см. §4.0);
**[Гипотеза]** — требует проверки на реальном Safari, приоритет для первого же прогона.

> **Update 2026-11-12, 09:40.** Основная причина расхождения Chrome/Firefox vs Safari
> больше не гипотеза: она воспроизведена в изолированном стенде
> `bins/kronika-web/ui/tests/safari-focus-probe.html` под Chrome 157 (Blink) и
> WebKit 26.0 (Playwright). Подробности — §4.0.

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

Ложный след: в собранном `kronika-ui.html.gz` есть 11 упоминаний `::-webkit-datetime-edit`,
`::-webkit-date-and-time-value`, `::-webkit-calendar-picker-indicator` и строка
`datetime-local` внутри таблицы типов React DOM. Всё это приходит из **Tailwind 4 preflight**
и внутренностей React, а не из нашего кода. Грепать бандл по «webkit» бессмысленно. **[Код]**

**Рекомендация:** не «чинить» проблему переходом на `<input type="time">`. Это регресс:
микросекундная точность метода (`HOUR_MICROS`, `hour-picker.tsx:11`), переключение
Browser time / UTC (`display-time.ts`), ручное разрешение DST-неоднозначностей
(`export-time.ts:126-142`, включая 30-минутный переход `Australia/Lord_Howe`), RU/EN-локализация
и контроль стилей — всё это потеряется, а на Safari < 14.1 станет ещё хуже.

---

## 2. Что представляет собой веб-морда

- Единственный фронтенд: `bins/kronika-web/ui/`. React 19.2.8, `@base-ui/react` 1.7.0
  (только Select/Portal), `@tanstack/react-table`, `@tanstack/react-virtual`, `uplot`, `lucide-react`.
- Сборка: **esbuild 0.28.2 + Tailwind CSS 4.3.3** (`scripts/build.mjs`), TypeScript 7.0.2 только
  для `tsc --noEmit`. `target: ["es2022"]`, `format: "iife"`, `minify: true`, `drop: ["console"]`.
  Ни Vite, ни Babel/SWC, ни **`browserslist`**, ни полифилов (`core-js` отсутствует). **[Код]**
- Результат — один самодостаточный `kronika-ui.html.gz` (1 538 018 байт raw, 285 330 байт CSS),
  два инлайн-`<script>`, шрифты `data:`. Раздаётся Rust-сервером
  (`bins/kronika-web/src/ui.rs:20`); CSP собирается в `build.rs:37-41`:
  `default-src 'none'; script-src 'sha256-...'; style-src 'unsafe-inline'; font-src data:; ...`
- Отдельный продукт: **офлайн HTML-отчёт** `crates/kronika-report` — другой шелл
  (`generator.rs:338-357`) с инлайн-base64 WASM и распаковкой через
  `new DecompressionStream("gzip")` + `WebAssembly.compile` + `BigInt(...)`. **[Код]**

---

## 3. Реальный минимальный Safari: 16.4

Версии ниже — из MDN browser-compat-data (срез `main`, 2026-11-12) и официальной
страницы [Tailwind Compatibility](https://tailwindcss.com/docs/compatibility). **[Спека]**

| Фича | Где в проекте | Минимум Safari |
|---|---|---|
| `Object.hasOwn` (59 вхождений в бандле) | `api.ts`, `postgres-view.tsx`, `system-view.tsx`, `postgres-metrics.ts` … | **15.4** |
| `Array.prototype.at` | `export-dialog.tsx:131,360`, `api.ts`, `timeline.tsx` … | **15.4** |
| `:has()` (14 правил в бандле) | `styles.css:675,682,699,708-712,753` (подсветка строк) | **15.4** |
| `dvh/svh/lvh` (23 вхождения) | `styles.css:415,554,560,791,792,857,858` | **15.4** |
| `oklab()` (10 вхождений, Tailwind) | собранный CSS | **15.4** |
| `color-mix()` (72 вхождения) | `styles.css:405,679,680,704,708-714,723` | **16.2** |
| **`@property` (47 правил в собранном CSS)** | Tailwind 4 (`--tw-translate-x`, …) | **16.4** |
| **`DecompressionStream`** | отчёт `generator.rs:355` | **16.4** |
| **Tailwind CSS v4 (официальный базовый таргет)** | `package.json:35` | **16.4** |
| `Blob.stream()` | отчёт `generator.rs:355` | 14.1 |
| `flex gap` | повсюду | 14.1 |
| `PointerEvent` / `pointerdown` capture | `hour-picker.tsx:61`, `export-dialog.tsx:368` | 13 |
| `ResizeObserver` | `entity-table.tsx:273` | 13.1 |
| `BigInt` | отчёт `generator.rs:357` | 14 |
| `a[download]` | `export-download.ts:121` | 10.1 |
| `scrollbar-width` | `detail.tsx:142`, `uplot-chart.tsx:394`, … | **18.2** (косметика) |

**Следствия:**

1. **Safari < 15.4 — белый экран.** `Object.hasOwn` вызывается на основном пути загрузки
   данных (`api.ts:1131,1600`), `esbuild target=es2022` даунлевелит только *синтаксис*, но не
   добавляет полифилы для *встроенных методов*. Ошибка `Object.hasOwn is not a function`
   уронит рендер целиком — пользователь опишет это как «не работает пикер», хотя не работает всё.
2. **Safari 15.4–16.3 — частичная деградация вёрстки.** `@property` и часть `color-mix`
   не поддерживаются, Tailwind 4 официально не тестируется на этих версиях: ломаются
   тени/фоны поповера (`styles.css:723` — `color-mix` для `--color-shadow`), подсветка строк,
   градиенты. Пикер может открыться «невидимым» или без фона.
3. **Safari < 16.4 — не откроется офлайн-отчёт** (`DecompressionStream is not defined`).
   Это отдельный сценарий отказа, не связанный с веб-сервером.
4. Проект **нигде не декларирует** поддерживаемые браузеры: нет `browserslist`, в README/INSTALL
   нет требований к браузеру, слово «Safari» не встречается ни в одном файле репозитория. **[Код]**

---

## 4. Почему именно пикер ломается в Safari: ранжированные причины

### 4.0 Эмпирическое воспроизведение — **[Воспроизведено]**

Стенд `bins/kronika-web/ui/tests/safari-focus-probe.html` повторяет ровно три элемента
`hour-picker.tsx`: guard `onBlur` (`:101-104`), `pointerdown`-capture dismissal (`:51-54,61`)
и `requestAnimationFrame(...focus())` (`:50`). Портал эмулируется с учётом того, что **React 19
вешает `listenToAllSupportedEvents` на контейнер портала** (проверено в
`react-dom@19.2.8/cjs/react-dom-client.production.js`, commit-ветка `HostPortal`, `case 4`) —
поэтому `focusout` из поповера, лежащего в `document.body`, всё равно доходит до `onBlur`
корневого `div` через React-дерево. Стенд учитывает это отдельным слушателем на контейнере
портала. Клики настоящие (Playwright `page.click`), а не синтетические `element.click()`.

| Шаг | Chrome 157.0.8103.51 (Blink) | WebKit 26.0 (Playwright) |
|---|---|---|
| `activeElement` после клика по trigger | `trigger` | **`BODY`** |
| `activeElement` после rAF-фокуса | `hour` | `hour` |
| `pointerdown(day)` | цель внутри поповера → не закрываем | цель внутри поповера → не закрываем |
| `focusout(hour)` | `relatedTarget = day` → **guard passes** | **`relatedTarget = null` → guard fails** |
| `CLOSED` до `click` | нет | **да** |
| `day-CLICK-FIRED` / `setDay()` | **1** | **0** |

То есть отказ детерминированный и объясняется **одним-единственным отличием**: в WebKit клик по
`<button>` не перемещает фокус, поэтому у `focusout` не становится `relatedTarget`; guard
`event.relatedTarget instanceof Node && (...contains...)` проваливается, `setOpen(false)`
выполняется в фазе `pointerdown → focusout`, поповер размонтируется, и событие `click` по ячейке
уже некому принимать. В Blink кнопка получает фокус, `relatedTarget` указывает на неё, guard
пропускает закрытие — выбор дня/часа работает. Gecko в этом сценарии ведёт себя как Blink
(клик фокусирует кнопку; на macOS у Firefox есть исторический нюанс, стоит проверить отдельно).

**Побочный эффект:** на iOS Safari `focus()` внутри `requestAnimationFrame` ещё и игнорируется
(вызов вне синхронного потока клика), так что клавиатурная навигация
(`pickerFocusIndex`, `:192-198`) там недоступна даже если поповер не закрылся.

Воспроизвести локально (или просто открыть HTML в двух браузерах и сверить лог на странице):

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

### P0-1. Focus-семантика WebKit — **[Воспроизведено]**, код **[Код]**

WebKit отличается от Blink/Gecko тремя вещами **[Спека]**:

- клик по `<button>` / `<a>` **не передаёт фокус** (macOS Safari и iOS Safari, даже с `tabindex`;
  таблица совместимости в MDN `<button>`);
- `element.focus()` **игнорируется**, если вызван не в том же синхронном потоке, что и клик
  (`setTimeout`, `requestAnimationFrame` после ре-рендера React);
- `blur`/`focusout` при клике по пустой области может не срабатывать, а `relatedTarget` часто `null`.

Попадания в коде:

```ts
// hour-picker.tsx:50 — ровно паттерн «focus() в rAF», который iOS Safari игнорирует
const frame = requestAnimationFrame(() => hourCells.current[Math.max(0, selected)]?.focus())

// hour-picker.tsx:101-104 — закрытие по relatedTarget
onBlur={(event) => {
  if (event.relatedTarget instanceof Node && (root.current?.contains(event.relatedTarget)
      || popoverNode.current?.contains(event.relatedTarget))) return
  setOpen(false)
}}
```

Механизм отказа (подтверждён прогоном, §4.0): поповер порталится в `document.body`
(`hour-picker.tsx:124,147`), то есть **находится вне `root` в DOM**, но остаётся потомком
корневого `div` в React-дереве — поэтому его `focusout` всё равно вызывает `onBlur` корня.
При `relatedTarget === null` условие проваливается и `setOpen(false)` выполняется до `onClick`
ячейки → «открылся и сразу закрылся» / «клик по дню ничего не делает». На iOS rAF-фокус
дополнительно не срабатывает → недоступна клавиатурная навигация
(`pickerFocusIndex`, `hour-picker.tsx:192-198`, обработчик `onKeyDown` на `:144`).

**Диагностический маркер:** `export-dialog.tsx:363-370` (`DayPicker`) использует **только**
`pointerdown`-capture и не имеет `onBlur`. Если в Safari ломается верхний пикер дня/часа, а
календарь в диалоге экспорта работает — причина именно в `onBlur` + focus-семантике.

**Дополнительно (iOS):** позиционирование `hourPopoverPlacement` (`hour-picker.tsx:70-84,158-168`)
берёт `window.innerHeight` и `document.documentElement.clientWidth`. В iOS Safari это layout
viewport, а не visual viewport: при раскрытой нижней панели поповер уходит под тулбар, а
`max-h-[calc(100vh_-_20px)]` (`:124`) не спасает. Нужны `visualViewport` / `dvh`.

**Предлагаемый фикс (без смены архитектуры):**
- удалить `onBlur` у `hour-picker`; единственным источником закрытия оставить
  `pointerdown` capture (`:61`) + `Escape` (`:55-60`) — как в `DayPicker`;
- фокусировать первую ячейку синхронно в `show()` после монтирования (или в `useLayoutEffect`),
  не в `requestAnimationFrame`; при необходимости дать контейнеру поповера `tabIndex={-1}`;
- для iOS считать высоту по `window.visualViewport?.height ?? window.innerHeight`.

### P0-2. Синхронный `revokeObjectURL` ломает скачивание отчёта — **[Гипотеза]**, код **[Код]**

`export-download.ts:111-130`:

```ts
const objectUrl = urlRef.createObjectURL(blob)
...
link.click()
} finally {
  link?.remove()
  urlRef.revokeObjectURL(objectUrl)   // ← сразу после click(), в том же тике
}
```

В iOS Safari скачивание по blob URL через `a[download]` асинхронно, и немедленный `revoke`
отменяет загрузку; исторически iOS Safari ещё и игнорирует `download` для blob URL (выдаёт
«unknown»). Chrome так прощает, Safari — нет. Симптом: диалог экспорта показывает `saved`,
а файла нет. **[Спека]** — широко описанный практический кейс, требует проверки на устройстве.

Фикс: отложить `revokeObjectURL` (`setTimeout(..., 0)` минимум, надёжнее — через несколько
секунд / после `focus`/`pageshow`) и добавить fallback «открыть в новой вкладке» для iOS.

### P1-1. `Intl.DateTimeFormat`: не совместимость, а корректность — **[Гипотеза]**

`display-time.ts:45` строит все метки через один форматтер:

```ts
Intl.DateTimeFormat("en-US-u-ca-gregory-nu-latn", {
  day: "2-digit", hour: "2-digit", hourCycle: "h23",
  minute: "2-digit", month: "2-digit", second: "2-digit",
  timeZone, year: "numeric"
}).formatToParts(...)
```

`Intl.DateTimeFormat` — Safari 10+, `formatToParts` — 11+, `hourCycle` формально поддерживается,
но у WebKit/Blink исторически есть расхождения `h23` vs `h24`: полночь может отформатироваться
как `24:xx` вместо `00:xx`. Тогда `dayKey` (`:52`) и `monthKey` (`:82`) сдвигаются, часть часов
`hoursForDay` (`hour-picker.tsx:188-190`) пропадает из сетки — **виджет исправен, данные нет**.
Плюс `browserTimeZone()` (`:39-40`) полагается на `resolvedOptions().timeZone`, который в
Safari с усиленной приватностью/в нестандартных зонах может вернуть смещение вида `GMT+3`
или пустоту.

Проверить в Safari отдельным тестом: полночь `00:00`, граница `23:00–00:00`, DST-зоны
`America/Sao_Paulo`, `Australia/Lord_Howe`, `Pacific/Apia`, RU-локаль
(`calendarMonthLabel`, `:94-97`). Существующие юнит-тесты гоняются в Node/ICU, а не в WebKit.

### P1-2. Системная причина: Safari вообще никогда не тестировался — **[Код]**

- `scripts/report-browser-smoke.sh:18-28` ищет только
  `chromium-browser / chromium / google-chrome-stable / google-chrome` (`CHROME_BIN`).
- Браузерные тесты (`tests/production-artifact.browser.mjs`, `browser-smoke.mjs`,
  `browser-corrections.mjs`) — самописный CDP поверх WebSocket к Chromium.
- Нет Playwright / Vitest / Jest / WebKit-таргета, нет `.browserslistrc`,
  `docs/releases.md:143` явно требует «Chromium/Google Chrome» для release-чека,
  12 BDD `.feature` покрывают только бэкенд/CLI.

То есть любое утверждение «в Safari не работает» сегодня **не воспроизводимо в CI** — его нельзя
ни подтвердить, ни защитить от регрессии. Это и есть корневая проблема процесса.

### P2. Косметика — **[Спека]**

`scrollbar-width` (Safari 18.2+), `:has()`-подсветка строк, `color-mix`-тени, префлайт Tailwind с
`@supports (-webkit-hyphens:none)` (29 правил), `-webkit-appearance`. Ломают вид, не логику.
За исключением `:has()` на `.entity-row` — при его отсутствии **всё правило целиком**
отбрасывается (MDN: не-`forgiving` selector list), поэтому пропадает не одно свойство, а вся
подсветка критичных строк.

---

## 5. План проверки (что сделать до любых правок)

> Пункт «воспроизвести расхождение движков» уже закрыт: §4.0, Chrome 157 vs WebKit 26.0.
> Ниже — то, что остаётся сделать на **реальном** Safari/WebKit-устройстве.

1. **Зафиксировать симптом.** Точная версия Safari + macOS/iOS, скриншот, Console:
   `Object.hasOwn is not a function` → п.3; `DecompressionStream` → офлайн-отчёт;
   тишина + «открывается/закрывается» → P0-1; `saved`, но нет файла → P0-2.
2. **Проба в консоли реального Safari** (на странице веб-морды):
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
   `export-day-grid`). Важно: Playwright WebKit — это движок WebKit, а не Safari.app; финальное
   подтверждение — реальный Safari. На этой машине macOS 27.2, Safari установлен,
   `/System/Cryptexes/App/usr/bin/safaridriver` доступен (нужно включить
   Develop → Allow Remote Automation).
4. **Регрессионные кейсы:** открытие/закрытие поповера кликом вне, выбор дня и часа мышью,
   Arrow/Home/End, Escape, месяц вперёд/назад, `00:00` и DST `23:00–00:00`, RU-локаль,
   узкий viewport 320 px, iOS с раскрытым тулбаром, экспорт и получение файла.

---

## 6. Приоритеты

| Приоритет | Действие | Риск |
|---|---|---|
| **P0 — подтверждено** | Убрать `onBlur` в `hour-picker.tsx`, перенести фокус из rAF в синхронный/`useLayoutEffect`, `visualViewport` для iOS | снимает основной симптом «пикер не работает»; причина доказана прогоном §4.0 |
| **P0** | Отложить `revokeObjectURL` в `export-download.ts` + iOS-fallback | чинит скачивание отчёта |
| **P1** | Задекларировать **Safari 16.4+** (iOS/iPadOS 16.4+, macOS 13.3+) в INSTALL/README, добавить `browserslist` и согласовать `esbuild target` | убирает неопределённость; при необходимости поддержки 15.x — точечные полифилы `Object.hasOwn`/`.at` |
| **P1** | WebKit-джоба в CI + `Intl`-тесты на полночь/DST | защита от регрессий |
| **P2** | `scrollbar-width` фолбэк, явные `@supports` для `color-mix`/`:has`, офлайн-отчёт: понятная ошибка вместо пустой страницы на Safari < 16.4 | косметика и UX |

**Чего не делать:** не заменять кастомный пикер на `<input type="time">` / `datetime-local`
(WebView-баг 261744, потеря микросекунд, DST-логики и тестового контракта); не добавлять
UA-sniffing под Safari — в бандле уже есть feature detection внутри `@base-ui`/floating-ui,
и вторая система детекции только усложнит поддержку.

---

## 7. Источники

- MDN browser-compat-data (`main`, срез 2026-11-12): `api/DecompressionStream.json`,
  `css/at-rules/property.json`, `css/selectors/has.json`, `css/types/color.json`,
  `css/types/length.json`, `css/properties/gap.json`, `css/properties/scrollbar-width.json`,
  `javascript/builtins/{Array,Object,BigInt}.json`, `api/{PointerEvent,Blob,ResizeObserver}.json`,
  `html/elements/input/{date,time,datetime-local,month,week}.json`, `html/elements/a.json`.
- [Tailwind CSS v4 — Compatibility](https://tailwindcss.com/docs/compatibility): Chrome 111 / **Safari 16.4** / Firefox 128.
- [WebKit bug 261744](https://webkit.org/b/261744) — `datetime-local` без time picker;
  [WebKit bug 200416](https://webkit.org/b/200416) — `month`/`week`.
- [张鑫旭 · 细说 iOS Safari 下 focus 的行为](https://www.zhangxinxu.com/wordpress/2020/10/ios-safari-input-button-focus/) —
  кнопки не получают фокус по клику, `focus()` вне потока клика не работает, `blur` не срабатывает.
- `react-dom@19.2.8/cjs/react-dom-client.production.js` — `listenToAllSupportedEvents(workInProgress.stateNode.containerInfo)`
  в commit-ветке `HostPortal`: порталы получают собственные слушатели, поэтому `focusout` из
  `document.body` доходит до `onBlur` корневого `div` через React-дерево.
- Playwright 1.56.1: Chrome 157.0.8103.51 (Blink, `channel: "chrome"`), WebKit 26.0
  (headless), реальные `page.click`. Стенд: `bins/kronika-web/ui/tests/safari-focus-probe.html`.
- Собственный код: `bins/kronika-web/ui/src/**`, `scripts/build.mjs`, `bins/kronika-web/build.rs`,
  `crates/kronika-report/src/generator.rs`, `scripts/report-browser-smoke.sh`, `docs/releases.md`.
