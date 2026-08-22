#!/usr/bin/env node
/* Установка нативного ядра в купленную копию Sandustry 0.5.2.
 *
 *   node installer/install.js              найти игру и поставить
 *   node installer/install.js --game ПУТЬ  указать каталог игры вручную
 *   node installer/install.js --revert     вернуть всё как было
 *
 * Что происходит: рядом с app.asar кладётся его копия app.asar.sandustry-native
 * .bak, архив распаковывается, в нём патчатся два файла — main.js и
 * dist/js/simulation-worker.js, — рядом с ресурсами появляется каталог
 * sand-native с нативным модулем, и архив собирается обратно.
 *
 * Откат — это возврат сохранённой копии, а не обратный патч: так не важно,
 * что именно успел сделать неудачный запуск.
 */
'use strict';

const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const asar = require('@electron/asar');

const { resolveAsar } = require('./find-game');
const { patchWorker } = require('./patch-worker');
const { patchMain } = require('./patch-main');

const BACKUP_SUFFIX = '.sandustry-native.bak';
const SUPPORTED_VERSION = '0.5.2';

/** Имя файла ядра для текущей платформы. */
function coreFileName() {
  switch (process.platform) {
    case 'darwin': return `sand-${process.arch}-darwin.node`;
    case 'win32': return `sand-${process.arch}-win32.node`;
    default: return `sand-${process.arch}-linux.node`;
  }
}

/**
 * Где лежит собранное ядро: сначала то, что скачано из релиза (prebuilt/),
 * потом собранное локально через cargo.
 */
function findCore(root) {
  const candidates = [
    path.join(root, 'prebuilt', coreFileName()),
    path.join(root, 'js', 'sand.node'),
  ];
  for (const c of candidates) if (fs.existsSync(c)) return c;
  throw new Error(
    'нативное ядро не найдено. Скачайте его из релиза в каталог prebuilt/ ' +
    'или соберите сами:\n' +
    '  cargo build --release -p sand-node\n' +
    `  cp target/release/${process.platform === 'win32' ? 'sand_node.dll' :
      process.platform === 'darwin' ? 'libsand_node.dylib' : 'libsand_node.so'} js/sand.node`);
}

/** Проверяет, что это та версия игры, на которой патч проверялся. */
function checkVersion(unpacked, force) {
  let version = null;
  try {
    version = JSON.parse(fs.readFileSync(path.join(unpacked, 'package.json'), 'utf8')).version;
  } catch (_) { /* нет package.json — разберёмся ниже */ }

  if (version === SUPPORTED_VERSION) return version;
  const message = `версия игры ${version || 'неизвестна'}, а патч проверялся на ${SUPPORTED_VERSION}`;
  if (!force) {
    throw new Error(`${message}.\nЕсли уверены — повторите с --force. Патч ищет ` +
      'конкретные места в коде игры: если они изменились, он просто не применится.');
  }
  console.warn(`  предупреждение: ${message}, ставим по --force`);
  return version;
}

/**
 * macOS проверяет подпись бандла, а мы поменяли его содержимое. Без повторной
 * подписи игра падает при старте с кодом 137 — без единого сообщения.
 */
function resign(asarPath) {
  if (process.platform !== 'darwin') return;
  const appBundle = asarPath.split('/Contents/Resources/')[0];
  if (!appBundle.endsWith('.app')) return;
  console.log(`  подписываю ${path.basename(appBundle)}`);
  execFileSync('codesign', ['--force', '--sign', '-', appBundle], { stdio: 'inherit' });
}

async function install({ game, force }) {
  const root = path.join(__dirname, '..');
  const asarPath = resolveAsar(game);
  const resources = path.dirname(asarPath);
  const backup = asarPath + BACKUP_SUFFIX;
  const core = findCore(root);

  console.log(`игра:  ${asarPath}`);
  console.log(`ядро:  ${core}`);

  if (fs.existsSync(backup)) {
    throw new Error('ядро уже установлено. Сначала снимите: node installer/install.js --revert');
  }

  const staging = fs.mkdtempSync(path.join(os.tmpdir(), 'sandustry-native-'));
  try {
    console.log('распаковываю app.asar');
    asar.extractAll(asarPath, staging);

    const version = checkVersion(staging, force);
    console.log(`  версия игры: ${version}`);

    // Модуль кладётся вне архива: из asar нативный модуль не загрузить.
    const coreDir = path.join(resources, 'sand-native');
    fs.mkdirSync(coreDir, { recursive: true });
    const coreTarget = path.join(coreDir, 'sand.node');
    fs.copyFileSync(core, coreTarget);
    if (process.platform === 'darwin') {
      // Без подписи macOS убивает процесс, который его загрузил.
      execFileSync('codesign', ['--force', '--sign', '-', coreTarget], { stdio: 'inherit' });
    }

    console.log('патчу main.js');
    patchMain(path.join(staging, 'main.js'));

    console.log('патчу воркер симуляции');
    const stats = patchWorker(path.join(staging, 'dist/js/simulation-worker.js'), coreTarget);
    console.log(`  перехвачено: чанков ${stats.chunkCalls}, колонок ${stats.columnCalls}, ` +
      `сетка структур ${stats.grid}, таблица элементов ${stats.matter}, смеси ${stats.mixes}`);

    console.log('сохраняю оригинал и собираю архив');
    fs.copyFileSync(asarPath, backup);
    // steamworks.js — нативный модуль: его .node и .dylib обязаны лежать на
    // диске рядом с архивом, иначе игра не свяжется со Steam и не запустится.
    await asar.createPackageWithOptions(staging, asarPath, {
      unpackDir: 'node_modules/steamworks.js/**',
    });

    resign(asarPath);

    console.log('\nготово. Запускайте игру как обычно — ядро включится само.');
    console.log('Моды Workshop при этом выключены: они несовместимы с ядром.');
    console.log('Нужны моды — добавьте в параметры запуска Steam:  %command% --with-mods');
    console.log('Снять патч:  node installer/install.js --revert');
  } finally {
    fs.rmSync(staging, { recursive: true, force: true });
  }
}

function revert({ game }) {
  const asarPath = resolveAsar(game);
  const backup = asarPath + BACKUP_SUFFIX;
  if (!fs.existsSync(backup)) {
    throw new Error(`сохранённой копии нет: ${backup}\n` +
      'Если игра ведёт себя странно, восстановите её через Steam: ' +
      '«Свойства» → «Установленные файлы» → «Проверить целостность».');
  }
  fs.copyFileSync(backup, asarPath);
  fs.rmSync(backup);
  fs.rmSync(path.join(path.dirname(asarPath), 'sand-native'), { recursive: true, force: true });
  resign(asarPath);
  console.log('патч снят, игра вернулась к оригиналу');
}

async function main() {
  const argv = process.argv.slice(2);
  const at = argv.indexOf('--game');
  const options = {
    game: at === -1 ? null : argv[at + 1],
    force: argv.includes('--force'),
  };
  try {
    if (argv.includes('--revert')) revert(options);
    else await install(options);
  } catch (err) {
    console.error(`\nне вышло: ${err.message}`);
    process.exit(1);
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(`\nне вышло: ${err && err.stack || err}`);
    process.exit(1);
  });
}
