/* Поиск установленной Sandustry.
 *
 * Игру ставят из Steam, и путь у всех разный: своя библиотека Steam, свой диск,
 * свой язык системы. Ищем не «где положено», а по факту — файл
 * resources/app.asar внутри каталога игры.
 *
 * Ничего не патчим и не пишем: этот модуль только отвечает на вопрос «где».
 */
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

/** Каталоги библиотек Steam, типовые для каждой платформы. */
function steamRoots() {
  const home = os.homedir();
  switch (process.platform) {
    case 'darwin':
      return [path.join(home, 'Library/Application Support/Steam/steamapps/common')];
    case 'win32': {
      // Диск с системой не единственный: библиотеки часто выносят на большой.
      const drives = ['C:', 'D:', 'E:', 'F:'];
      const suffixes = [
        'Program Files (x86)\\Steam\\steamapps\\common',
        'Program Files\\Steam\\steamapps\\common',
        'SteamLibrary\\steamapps\\common',
      ];
      const out = [];
      for (const d of drives) for (const s of suffixes) out.push(path.join(d + '\\', s));
      return out;
    }
    default:
      return [
        path.join(home, '.steam/steam/steamapps/common'),
        path.join(home, '.local/share/Steam/steamapps/common'),
        path.join(home, '.var/app/com.valvesoftware.Steam/data/Steam/steamapps/common'),
      ];
  }
}

/** Где внутри каталога игры лежит app.asar — зависит от платформы. */
function asarCandidates(gameDir) {
  return [
    path.join(gameDir, 'resources', 'app.asar'),                       // Windows, Linux
    path.join(gameDir, 'Sandustry.app/Contents/Resources/app.asar'),   // macOS
    path.join(gameDir, 'Contents/Resources/app.asar'),                 // указали сам .app
    path.join(gameDir, 'app.asar'),                                    // указали resources
  ];
}

/**
 * Путь к app.asar по каталогу игры (или по самому .asar).
 * Возвращает null, если ничего похожего нет.
 */
function asarIn(target) {
  if (!target) return null;
  if (target.endsWith('.asar') && fs.existsSync(target)) return target;
  for (const candidate of asarCandidates(target)) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

/**
 * Ищет установку сама. Возвращает путь к app.asar или null.
 *
 * Имя каталога у игры одно и то же во всех библиотеках Steam, но регистр на
 * Linux имеет значение, поэтому сверяем без учёта регистра.
 */
function autodetect() {
  for (const root of steamRoots()) {
    let entries;
    try {
      entries = fs.readdirSync(root, { withFileTypes: true });
    } catch (_) {
      continue;   // библиотеки на этом пути нет — обычное дело
    }
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.toLowerCase() !== 'sandustry') continue;
      const found = asarIn(path.join(root, entry.name));
      if (found) return found;
    }
  }
  return null;
}

/**
 * Разрешает путь к игре: сначала то, что указали руками, потом автопоиск.
 * Бросает с внятным текстом, если не нашли, — установщику остаётся только
 * показать сообщение.
 */
function resolveAsar(explicit) {
  if (explicit) {
    const found = asarIn(path.resolve(explicit));
    if (found) return found;
    throw new Error(
      `в ${explicit} нет resources/app.asar — это не каталог с установленной Sandustry`);
  }
  const found = autodetect();
  if (found) return found;
  throw new Error(
    'Sandustry не найдена автоматически. Укажите каталог игры явно:\n' +
    '  node installer/install.js --game "путь к каталогу игры"\n' +
    'В Steam он открывается через «Свойства» → «Установленные файлы» → ' +
    '«Обзор».');
}

module.exports = { resolveAsar, autodetect, asarIn };
