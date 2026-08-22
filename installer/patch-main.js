/* Патч главного процесса игры.
 *
 * Нативное ядро живёт в воркере симуляции, а воркер может загрузить нативный
 * модуль только при `nodeIntegrationInWorker`. Этот флаг даёт воркерам доступ
 * к Node — а вместе с ними и модам Workshop, которые в тех же воркерах
 * исполняются. Мод получил бы файловую систему целиком.
 *
 * Поэтому флаг и моды здесь взаимоисключающие: ядро работает, когда моды
 * выключены, и наоборот. Никакой сборки, где мод исполняется рядом с открытым
 * Node, этот установщик не производит.
 *
 * Вызывается из installer/install.js, самостоятельно не запускается.
 */
'use strict';

const fs = require('fs');

const MARK = '/* sandustry-native mode switch */';

/** Переключатель режима, вставляется в начало main.js. */
const SWITCH = `${MARK}
// Моды Workshop и нативное ядро несовместимы по устройству: ядру нужен доступ
// к Node из воркера симуляции, а моды исполняются в тех же воркерах и получили
// бы этот доступ вместе с ним.
//
// По умолчанию работает ядро. Запуск с модами (и без ускорения) — аргументом
// --with-mods или переменной SANDUSTRY_MODS=1; в Steam это задаётся в
// «Свойства» → «Параметры запуска»:  %command% --with-mods
const SANDUSTRY_NATIVE_WITH_MODS =
  process.argv.includes('--with-mods') || process.env.SANDUSTRY_MODS === '1';
console.log(SANDUSTRY_NATIVE_WITH_MODS
  ? '[sandustry-native] режим модов: ядро выключено, Workshop работает как обычно'
  : '[sandustry-native] режим ядра: моды Workshop выключены');
`;

/**
 * Патчит main.js: добавляет переключатель, флаг Node для воркеров и условие
 * на загрузку модов.
 *
 * Все три точки обязательны. Если хоть одна не нашлась, версия игры не та —
 * лучше не установиться совсем, чем оставить игру наполовину пропатченной.
 */
function patchMain(mainFile) {
  let code = fs.readFileSync(mainFile, 'utf8');
  if (code.includes(MARK)) return { skipped: true };

  // 1. Флаг Node для воркеров — только когда моды выключены.
  const webPrefs = `    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),`;
  if (!code.includes(webPrefs)) {
    throw new Error('webPreferences в main.js не найдены — версия игры не та');
  }
  code = code.replace(webPrefs, `${webPrefs}
      // Нужно, чтобы воркер симуляции мог загрузить нативное ядро. Флаг даёт
      // воркерам доступ к Node, поэтому он выключается вместе с включением
      // модов — см. переключатель в начале файла.
      nodeIntegrationInWorker: !SANDUSTRY_NATIVE_WITH_MODS,`);

  // 2. Моды Workshop грузятся только в своём режиме.
  const mods = `  if (PLATFORM_NAME === 'steam') {
    workshopDiscoveryResult = discoverNativeWorkshopMods();
    setupProtocolInterceptor();
  }`;
  if (!code.includes(mods)) {
    throw new Error('точка загрузки модов в main.js не найдена — версия игры не та');
  }
  code = code.replace(mods, `  if (PLATFORM_NAME === 'steam' && SANDUSTRY_NATIVE_WITH_MODS) {
    workshopDiscoveryResult = discoverNativeWorkshopMods();
    setupProtocolInterceptor();
  }`);

  // 3. Сам переключатель — после 'use strict', если он есть.
  const strict = code.match(/^\s*(['"])use strict\1;?\s*\n/);
  code = strict
    ? code.slice(0, strict[0].length) + SWITCH + code.slice(strict[0].length)
    : SWITCH + code;

  fs.writeFileSync(mainFile, code, 'utf8');
  return { patched: true };
}

module.exports = { patchMain, MARK };
