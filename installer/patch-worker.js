/* Мост между игрой и нативным ядром.
 *
 * Патчит dist/js/simulation-worker.js внутри распакованного app.asar: каждый
 * обход чанка и колонки сначала предлагается ядру на Rust, и только если оно
 * за чанк не взялось — считает старый код. Отбор делает само ядро: типы
 * материи, мод-перехватчики, структуры рядом, таймеры.
 *
 * Патч намеренно консервативный. Если чего-то не хватает — модуля, массивов,
 * сетки структур — нативный путь просто не включается, и игра работает как
 * работала. Молчаливой подмены с непонятным поведением быть не должно.
 *
 * Вызывается из installer/install.js, самостоятельно не запускается.
 */
'use strict';

const fs = require('fs');

const MARK = '/* sandustry-native bridge */';

const BRIDGE = `${MARK}
var __nativeSim = null, __nativeReady = false, __nativeTried = false, __nativeFrame = 0, __nativeGridWaits = 0;
// A/B-замер: ядро включается и выключается по общим часам, фазами по N секунд.
// Канала между окном и воркерами у нас нет, а Date.now() у всех один — поэтому
// все восемь воркеров переключаются синхронно без единого сообщения. Ноль
// (по умолчанию) означает «ядро включено всегда», как в обычной игре.
var __nativeAbPeriod = 0;
try {
  __nativeAbPeriod = Math.max(0, parseInt(process.env.SANDUSTRY_NATIVE_AB || '0', 10) || 0);
} catch (_) {}
var __nativeAbOn = true, __nativeAbUntil = 0;

// Разбивка времени внутри native-фазы. Ядро считает быстро, но за тик его
// работу ещё надо разложить обратно в игру: перерисовать растр, досчитать
// отданные клетки, обойти отданные чанки старым кодом. Без этих цифр не
// понять, кто именно съедает выигрыш.
var __nativeMs = { core: 0, repaint: 0, needsJs: 0, deferred: 0, gz: 0, cJ: 0, chunks: 0 };
function __nativeMsReset() {
  __nativeMs.core = __nativeMs.repaint = __nativeMs.needsJs = __nativeMs.deferred = 0;
  __nativeMs.gz = __nativeMs.cJ = __nativeMs.chunks = 0;
}

// Фаза пересчитывается не чаще раза в 100 мс: вызов приходится на каждую
// колонку и каждый чанк, а Date.now() в таком месте — уже заметная статья.
function __nativeOn() {
  if (!__nativeAbPeriod) return true;
  var now = Date.now();
  if (now < __nativeAbUntil) return __nativeAbOn;
  var phase = Math.floor(now / 1000 / __nativeAbPeriod);
  var on = (phase % 2) === 0;
  if (on !== __nativeAbOn) {
    __nativeAbOn = on;
    // Счётчики привязываем к фазе: на выходе из native печатаем, что она
    // успела сделать, на входе — начинаем с нуля.
    if (!on && __nativeReady) __nativeSummary(null);
    else if (on && __nativeSim) { __nativeSim.resetStats(); __nativeMsReset(); }
    __nativeLog('фаза ' + (on ? 'native' : 'base') + ', период ' + __nativeAbPeriod + ' с');
  }
  __nativeAbUntil = now + 100;
  return on;
}
var __nativeUpdatedBuffer = null, __nativeChangedBuffer = null, __nativeWidth = 0;
var __nativeDeferredBuffer = null, __nativeNeedsJsBuffer = null;
// Клетку, ставшую пустой, игра перерисовывает без пересчёта тени — как в
// cellOps: источник переноса гасится с skipShadow, цель рисуется полностью.
var __NATIVE_SKIP_SHADOW = { skipShadow: true };

// Воркер пишет в свою консоль, которой не видно ни в stdout, ни в DevTools
// главного окна. Поэтому диагностика идёт ещё и в файл — иначе непонятно,
// подключилось ядро или молча отвалилось.
function __nativeLog(msg) {
  try { console.log('[native] ' + msg); } catch (_) {}
  try {
    // Каталог берём у системы: /tmp есть не везде, а на Windows его нет вовсе.
    var os = require('os'), pathMod = require('path');
    require('fs').appendFileSync(pathMod.join(os.tmpdir(), 'sandustry-native.log'),
      new Date().toISOString() + ' ' + msg + '\\n');
  } catch (_) {}
}

// Элементы, чьи реакции зашиты в саму игру (deobf: secondaryResult_42074 —
// смеси, const_71560 и const_13653 — лёд с водой и лавой). Пары ядро не
// считает никогда: за реакцией тянутся вторичные продукты, звук и экономика.
var __NATIVE_VANILLA_MIXES = [
  [3, 1],   // Water + Sand
  [3, 15],  // Water + Seed
  [3, 19],  // Water + Lava
  [3, 13],  // Water + Flame
  [12, 3],  // FreezingIce + Water
  [12, 19], // FreezingIce + Lava
];

// Список известных типов и признак того, что карта реакций построена по самой
// игре, а не по грубой пометке участников смесей.
var __nativeTypes = null, __nativeMixExact = false;

function __nativeReactions(state, types) {
  var pairs = 0;
  for (var i = 0; i < __NATIVE_VANILLA_MIXES.length; i++) {
    __nativeSim.setReaction(__NATIVE_VANILLA_MIXES[i][0], __NATIVE_VANILLA_MIXES[i][1]);
    pairs++;
  }

  // Смеси, добавленные модами: игра строит из них ту же карту, что и из своих.
  try {
    var mods = state.sandkit && state.sandkit.mods && state.sandkit.mods.elements;
    if (mods) {
      for (var key of Object.keys(mods)) {
        var def = mods[key];
        if (!def || !def.mixes) continue;
        for (var mix of def.mixes) {
          var other = mix.with !== undefined ? mix.with : mix.elementType;
          if (other === undefined) continue;
          __nativeSim.setReaction(def.elementType & 255, other & 255);
          pairs++;
        }
      }
    }
  } catch (err) {
    __nativeLog('смеси модов не прочитались: ' + (err && err.message));
  }

  // Точный ответ даёт сама игра: hu(state, a, b) — «реагирует ли эта пара».
  // Ссылка на модуль появляется после первого столкновения в физике, поэтому
  // до тех пор действует грубая пометка по типу, а карта уточняется потом.
  var mix = self.__mixModule;
  if (mix && typeof mix.hu === 'function') {
    for (var a = 0; a < types.length; a++) {
      for (var b = a; b < types.length; b++) {
        try {
          if (mix.hu(state, types[a], types[b])) {
            __nativeSim.setReaction(types[a] & 255, types[b] & 255);
            pairs++;
          }
        } catch (_) { /* пара не поддерживается — не реакция */ }
      }
    }
    __nativeMixExact = true;
    __nativeLog('карта реакций уточнена по самой игре');
  } else {
    // Пока карты нет, участники ванильных смесей помечены целиком: лучше
    // лишний раз отдать клетку в JS, чем тихо проглотить реакцию.
    var risky = new Set();
    for (var m of __NATIVE_VANILLA_MIXES) { risky.add(m[0]); risky.add(m[1]); }
    for (var t of risky) __nativeSim.setReactive(t & 255, true, false);
  }
  return pairs;
}

// Модуль смесей появляется в self.__mixModule только после первого вызова
// hu/I0 в физике — а ядро инициализируется раньше. Пока карты нет, мост метит
// участников смесей целиком, и проверка пары считает реагирующей любую пару,
// где встретился помеченный тип. Вода и песок есть почти везде, поэтому такая
// пометка отправляет в JS больше половины обойденных клеток: замер показал 1.6
// млн досчётов за интервал против 500 по всем остальным причинам вместе.
//
// Поэтому карту надо переспросить у игры, как только модуль появился, и снять
// грубые пометки — иначе они живут до конца сессии.
function __nativeRefine(state) {
  if (__nativeMixExact || !__nativeSim || !__nativeTypes) return;
  var mix = self.__mixModule;
  if (!mix || typeof mix.hu !== 'function') return;
  for (var m of __NATIVE_VANILLA_MIXES) {
    __nativeSim.setReactive(m[0] & 255, false, false);
    __nativeSim.setReactive(m[1] & 255, false, false);
  }
  var pairs = __nativeReactions(state, __nativeTypes);
  __nativeLog('карта реакций перестроена по игре: пар ' + pairs);
}

// Нативный модуль лежит вне архива: из asar его загрузить нельзя. Сначала
// пробуем найти его рядом с ресурсами игры — так путь переживает перенос
// каталога и обновление Steam; если не вышло, берём тот, что записал
// установщик.
function __nativeRequireCore() {
  var tries = [];
  try {
    if (process.resourcesPath) {
      tries.push(require('path').join(process.resourcesPath, 'sand-native', 'sand.node'));
    }
  } catch (_) {}
  tries.push(__NATIVE_MODULE_PATH__);
  for (var i = 0; i < tries.length; i++) {
    try { return require(tries[i]); } catch (err) { var last = err; }
  }
  __nativeLog('нативный модуль не найден: ' + tries.join(', ') +
    (last ? ' (' + last.message + ')' : ''));
  return null;
}

function __nativeInit(state) {
  __nativeTried = true;
  try {
    if (typeof require !== 'function') {
      __nativeLog('require недоступен: нужен nodeIntegrationInWorker');
      return;
    }
    const mod = __nativeRequireCore();
    if (!mod) return;
    const sim = state && state.shared && state.shared.sim;
    if (!sim || !sim.cellIds || !sim.elementData) {
      __nativeLog('состояние симуляции ещё не готово');
      return;
    }
    const e = sim.elementData;

    // Маска мод-перехватчиков. Достать её из замыкания модуля событий нельзя,
    // поэтому идём от обратного: если в этой сборке вообще зарегистрирован
    // хоть один перехватчик на элементы, нативный путь не включается совсем.
    // Лучше не ускориться, чем незаметно сломать мод.
    const hooks = new Uint8Array(256);
    const sandkit = state.sandkit;
    const risky = sandkit && sandkit.interceptors &&
      Object.keys(sandkit.interceptors).some((k) => k.startsWith('element:') || k.startsWith('cell:'));
    if (risky) {
      __nativeLog('есть мод-перехватчики физики — нативный путь выключен');
      return;
    }

    // Сетка структур живёт не в state.shared, а в замыкании своего модуля.
    // Ссылку на него патч захватывает при первом вызове getBlockTypeArray;
    // если физика ещё ни разу его не звала, пробуем позже, а не сдаёмся.
    const grid = self.__blockGrid;
    const blocks = grid && grid.getBlockTypeArray && grid.getBlockTypeArray();
    if (!blocks) {
      __nativeTried = false;   // повторим попытку на следующем чанке
      if (!__nativeGridWaits++) __nativeLog('жду появления сетки структур');
      return;
    }
    // Геометрию берём у самого модуля, а не считаем по догадке.
    const blockScale = grid.getTileShift ? (1 << grid.getTileShift()) : 4;
    const blockWidth = grid.getTileW ? grid.getTileW() : Math.ceil(sim.width / blockScale);

    // Список слотов, посчитанных ядром за тик. Игра гасит флаг hasBeenUpdated
    // по своему списку, и наш обязан в него влиться — иначе помеченный нами
    // элемент залипнет навсегда.
    __nativeUpdatedBuffer = new Int32Array(1 << 18);
    // Индексы клеток, которые ядро изменило: по ним мост прогоняет настоящий
    // cellOps.Gz. Сетку ядро правит само, но растр (mapData.data), тень и
    // пост-обработку рисует только игра — без этого шага мир считается верно,
    // а на экране выглядит рваным.
    __nativeChangedBuffer = new Int32Array(1 << 20);
    // Номера чанков колонки, за которые ядро не взялось.
    __nativeDeferredBuffer = new Int32Array(1024);
    // Клетки, которые ядро отдаёт игре поштучно: истёкшие таймеры, реакции.
    __nativeNeedsJsBuffer = new Int32Array(1 << 18);
    __nativeWidth = sim.width;

    __nativeSim = new mod.NativeSim({
      width: sim.width, height: sim.height, chunkSize: sim.chunkSize,
      cells: sim.cellIds,
      type: e.type,
      velocityX: e.velocityX, velocityY: e.velocityY,
      minVelocityX: e.minVelocityX, minVelocityY: e.minVelocityY,
      thresholdX: e.thresholdX, thresholdY: e.thresholdY,
      density: e.density,
      isFreeFalling: e.isFreeFalling,
      hasBeenUpdated: e.hasBeenUpdated,
      skipPhysics: e.skipPhysics,
      hasDuration: e.hasDuration, durationLeft: e.durationLeft,
      x: e.x, y: e.y,
      lastSideChecked: e.lastSideChecked,
      movesYAxis: e.movesYAxis, movesYAxisCount: e.movesYAxisCount,
      modHooks: hooks,
      blockTypes: blocks,
      blockWidth: blockWidth,
      blockScale: blockScale,
      needsJsBuffer: __nativeNeedsJsBuffer,
      // Флаги активности чанков. Текущий кадр ядро читает: клетку спящего
      // чанка трогать нельзя. Следующий — ставит само, иначе подвинутые им
      // чанки засыпают и физика идёт рывками.
      chunkShouldUpdate: sim.chunkShouldUpdate,
      chunkShouldUpdateNext: sim.chunkShouldUpdateNext,
      // Таблица терраина: по ней ядро отличает блок от скользящего блока и
      // от конвейера — с них диагональный сход запрещён.
      terrainType: sim.terrainType,
      updatedBuffer: __nativeUpdatedBuffer,
      changedBuffer: __nativeChangedBuffer,
      deferredBuffer: __nativeDeferredBuffer,
    });

    // Таблица материи приезжает из определений элементов игры, включая
    // добавленные модами. Определения тоже живут в замыкании своего модуля, а
    // не в состоянии: ссылку патч захватывает при первом обращении к таблице.
    //
    // Состояния материи в игре: Solid=1, Liquid=2, Particle=3, Gas=4,
    // Static=5, Slushy=6, Wisp=7, Powder=8. Песок у неё Solid, а не Powder —
    // перепутать здесь значит подсунуть песку чужую физику.
    const defs = self.__matterModule && self.__matterModule.m5;
    var known = 0;
    const types = [];
    if (defs) {
      for (const key of Object.keys(defs)) {
        const d = defs[key];
        if (!d) continue;
        const type = Number(key) & 255;
        const matter = d.matterType;
        if (matter && matter !== 3 && matter !== 7) { known++; types.push(type); }
        // Горизонтальная скорость — из самой игры. Своё число здесь ставить
        // нельзя: при пятёрке вместо единицы жидкость растекается впятеро
        // быстрее и вместо струй рисует длинные полки поперёк экрана.
        const speed = d.horizontalSpeed === undefined ? 1 : d.horizontalSpeed;
        __nativeSim.setMatter(type, matter, speed);
      }
    }
    // Единственный тип с личным потолком вертикальной скорости
    // (disableHorizontalMovement_96509: BurntResidue → 240).
    __nativeSim.setMaxVelocityY(14, 240);

    // Без таблицы ядро отклонит каждый чанк — но уже после осмотра, то есть
    // мы заплатим лишний проход по клеткам и не выиграем ничего. Лучше не
    // включаться совсем.
    if (known === 0) {
      __nativeLog('таблица материи пуста (определений: ' +
        (defs ? Object.keys(defs).length : 'нет') + ') — ядро не включаем');
      __nativeTried = false;   // определения могут появиться позже
      return;
    }

    __nativeTypes = types;
    const pairs = __nativeReactions(state, types);

    __nativeReady = true;
    __nativeLog('ядро подключено, мир ' + sim.width + 'x' + sim.height +
      ', известных материалов: ' + known + ', пар реакций: ' + pairs);
  } catch (err) {
    __nativeLog('ядро не подключилось: ' + (err && err.stack || err));
  }
}

// Перерисовка того, что ядро подвинуло.
//
// Сетка клеток — это ещё не мир: игра на каждую запись клетки обновляет растр
// (четыре байта RGBA в shared.mapData.data), тень и пост-обработку. Всё это
// живёт в замыканиях её модулей, поэтому ядро только копит список изменённых
// клеток, а перерисовывает их её же функция.
function __nativeRepaint(state) {
  // Получателя проверяем до того, как забрать список: takeChanged опустошает
  // очередь ядра безвозвратно. Уйти после неё — значит потерять все клетки,
  // которые ядро уже подвинуло: мир считается верно, а на экране остаётся
  // прошлый кадр, и никакой ошибки при этом не видно.
  const ops = self.__cellOps;
  if (!ops || typeof ops.Gz !== 'function') return;
  const n = __nativeSim.takeChanged();
  if (!n) return;
  const cells = state.shared.sim.cellIds;
  const w = __nativeWidth;
  __nativeMs.gz += n;
  for (let i = 0; i < n; i++) {
    const idx = __nativeChangedBuffer[i];
    const id = cells[idx];
    const x = idx % w, y = (idx / w) | 0;
    if (id === 0) ops.Gz(state, x, y, 0, __NATIVE_SKIP_SHADOW);
    else ops.Gz(state, x, y, id);
  }
}

// Клетки, за которые ядро не взялось, досчитывает старый код — поштучно, той
// же функцией, которой игра считает любую клетку с элементом.
//
// Там истёкшие таймеры и реакции элементов: за ними тянутся вторичные
// продукты, звук, очки производства и события модов. Не досчитать — значит
// тихо потерять их: семя не прорастёт, огонь не погаснет, вода на лаве не
// станет паром.
function __nativeFinishDeferred(state, dt) {
  // Порядок тот же и по той же причине: это клетки, которые ядро осознанно
  // вернуло игре. Забрать список и не досчитать — значит молча потерять
  // событие: семя не прорастёт, огонь не погаснет, вода на лаве не станет
  // паром.
  const matter = self.__matterModule;
  if (!matter || typeof matter.cJ !== 'function') return;
  const pairs = __nativeSim.takeNeedsJs();
  if (!pairs) return;
  const sim = state.shared.sim;
  const cells = sim.cellIds, types = sim.elementData.type;
  __nativeMs.cJ += pairs;
  for (let i = 0; i < pairs; i++) {
    const x = __nativeNeedsJsBuffer[i * 2], y = __nativeNeedsJsBuffer[i * 2 + 1];
    const id = cells[y * sim.width + x];
    if (id < 1000001) continue;              // клетка уже опустела
    const slot = id - 1000001;
    matter.cJ(state, id, slot, types[slot], x, y, dt);
  }
}

// Слоты, посчитанные ядром, — в список игры. Флаги гасит она сама в конце
// тика, пробегая по нему; нашего списка она не знает, поэтому вливаем свой.
function __nativeFlush(state, dt) {
  var __t0 = performance.now();
  __nativeFinishDeferred(state, dt);
  var __t1 = performance.now();
  __nativeRepaint(state);
  __nativeMs.needsJs += __t1 - __t0;
  __nativeMs.repaint += performance.now() - __t1;
  // И здесь так же: слоты, забранные из ядра и не влитые в список игры,
  // останутся помеченными навсегда — игра гасит hasBeenUpdated, только пройдя
  // по своему списку, и эти элементы выпадут из симуляции до конца сессии.
  const list = state.store && state.store.world && state.store.world.updatedElementIndices;
  if (!list) return;
  const n = __nativeSim.takeUpdated();
  if (!n) return;
  for (let i = 0; i < n; i++) list.push(__nativeUpdatedBuffer[i]);
}

// Направление обхода по X — то же, что у игры: она переворачивает его каждый
// тик, иначе сыпучее систематически сползает в одну сторону.
function __nativeLtr(state) {
  return (state.store && state.store.world &&
    state.store.world.matrixTraverseDirection) === 1;
}

// Обход целой колонки чанков. В режиме планировщика 1 игра ходит именно так,
// а не по отдельным чанкам, поэтому без этой обёртки ядро простаивает.
// Разбираем колонку на чанки сами: те, что ядро берёт, считаем здесь,
// остальные отдаём старому коду по одному.
function __nativeColumn(origColumn, origChunk, state, cx, even, dt, margin) {
  if (!__nativeTried) __nativeInit(state);
  if (!__nativeReady || !__nativeOn()) return origColumn(state, cx, even, dt, margin);

  // Колонка уходит в ядро одним вызовом. Раньше здесь был цикл с вызовом ядра
  // на каждый чанк, и это оказалось дороже самой физики: пять тысяч переходов
  // JS→native за тик на воркер, каждый с созданием объекта результата. На
  // пустом мире, где активных чанков ноль, воркеры всё равно жгли по 10 мс —
  // ровно на этих переходах.
  let deferred = 0;
  try {
    __nativeFrame++;
    if ((__nativeFrame % 2000) === 0) __nativeSummary(state);
    if (!__nativeMixExact && (__nativeFrame % 64) === 0) __nativeRefine(state);
    var __t0 = performance.now();
    deferred = __nativeSim.updateColumn(cx, margin | 0, __nativeLtr(state), dt);
    __nativeMs.core += performance.now() - __t0;
  } catch (err) {
    __nativeLog('сбой на колонке, дальше считает JS: ' + (err && err.message));
    // Ядро могло упасть на середине колонки, уже подвинув часть клеток.
    // Раскладываем накопленное: иначе растр останется в прошлом, а
    // помеченные слоты залипнут. Чанки, которые ядро успело посчитать, JS
    // пересчитает следом — один сдвоенный ход на аварийном пути дешевле
    // рассинхрона, который останется навсегда.
    try { __nativeFlush(state, dt); } catch (_) {}
    __nativeReady = false;
    return origColumn(state, cx, even, dt, margin);
  }
  __nativeFlush(state, dt);

  // Чанки, за которые ядро не взялось, обязан досчитать старый код — иначе они
  // выпадают из симуляции, и мир замирает по кусочкам. Порядок обхода передаём
  // тот же, что пришёл: Even=1, Odd=2. marginY здесь ноль — обход колонкой
  // сдвигает окно только по X.
  var __t1 = performance.now();
  for (let i = 0; i < deferred; i++) {
    origChunk(state, cx, __nativeDeferredBuffer[i], even ? 1 : 2, dt, margin, 0);
  }
  __nativeMs.deferred += performance.now() - __t1;
  __nativeMs.chunks += deferred;
}

function __nativeSummary(state) {
  const st = __nativeSim.stats();
  __nativeLog('тик ' + (state && state.store && state.store.meta && state.store.meta.tick) +
    ': взято чанков ' + st[0] +
    ', отдано материал/хук/структура/таймер ' +
    st[1] + '/' + st[2] + '/' + st[3] + '/' + (st[7] || 0) +
    ', клеток ' + st[4] + ', сдвинуто ' + (st[6] || 0) +
    ', спящих ' + (st[8] || 0) + ', вердиктов из кэша ' + (st[5] || 0) +
    ' | мс: ядро ' + __nativeMs.core.toFixed(0) +
    ', растр ' + __nativeMs.repaint.toFixed(0) +
    ', дозачёт ' + __nativeMs.needsJs.toFixed(0) +
    ', отданные чанки ' + __nativeMs.deferred.toFixed(0) +
    ' | вызовов Gz ' + __nativeMs.gz + ', cJ ' + __nativeMs.cJ +
    ', чанков в JS ' + __nativeMs.chunks +
    ' | досчёт застой/реакция/таймер/материя ' +
    (st[9] || 0) + '/' + (st[10] || 0) + '/' + (st[11] || 0) + '/' + (st[12] || 0));
  __nativeSim.resetStats();
}

// Спит ли чанк — то же условие, что в начале cellOps.E. Проверка стоит трёх
// чтений байта, а вызов ядра — перехода между мирами, поэтому спящий чанк
// отсекается здесь и в ядро не идёт вовсе.
function __nativeAsleep(sim, cx, cy, marginX, marginY) {
  const flags = sim.chunkShouldUpdate, w = sim.chunkWidth, h = sim.chunkHeight;
  if (flags[cy * w + cx] === 1) return false;
  if (marginX > 0 && cx + 1 < w && flags[cy * w + cx + 1] === 1) return false;
  if (marginY > 0 && cy + 1 < h && flags[(cy + 1) * w + cx] === 1) return false;
  if (marginX > 0 && marginY > 0 && cx + 1 < w && cy + 1 < h &&
      flags[(cy + 1) * w + cx + 1] === 1) return false;
  return true;
}

function __nativeChunk(orig, state, cx, cy, order, dt, marginX, marginY) {
  if (!__nativeTried) __nativeInit(state);
  if (__nativeReady && __nativeOn()) {
    if (__nativeAsleep(state.shared.sim, cx, cy, marginX | 0, marginY | 0)) return;
    try {
      __nativeFrame++;
      if ((__nativeFrame % 5000) === 0) __nativeSummary(state);
      if (__nativeSim.updateChunk(cx, cy, marginX | 0, marginY | 0, __nativeLtr(state), dt).native) {
        __nativeFlush(state, dt);
        return;
      }
    } catch (err) {
      __nativeLog('сбой на чанке, дальше считает JS: ' + (err && err.message));
      __nativeReady = false;
    }
  }
  return orig.apply(null, Array.prototype.slice.call(arguments, 1));
}

self.__nativeStats = function () {
  return __nativeSim ? __nativeSim.stats() : null;
};
`;

function patchWorker(workerFile, modulePath) {
  let code = fs.readFileSync(workerFile, 'utf8');
  if (code.includes(MARK)) return { skipped: true };

  const calls = code.match(/\(0,[a-zA-Z$_]+\.lR\)\(/g);
  if (!calls || calls.length === 0) {
    throw new Error('вызовы updateChunk не найдены — версия игры не та, ' +
      'патч не применён');
  }

  // Ссылка на модуль с определениями элементов: обращения к его таблице
  // остались неминифицированными, чем и пользуемся.
  const matterRefs = code.match(/([a-zA-Z$_]+)\.m5\[/g) || [];
  code = code.replace(/([a-zA-Z$_]+)\.m5\[/g, '(self.__matterModule=$1,$1.m5)[');

  // Сетка структур: наружу не выставлена, но её зовут из физики — цепляемся к
  // первому же обращению.
  const gridCalls = code.match(/\(0,([a-zA-Z$_]+)\.getBlockTypeArray\)\(\)/g) || [];
  code = code.replace(
    /\(0,([a-zA-Z$_]+)\.getBlockTypeArray\)\(\)/g,
    '(self.__blockGrid=$1,(0,$1.getBlockTypeArray)())'
  );

  // Модуль смесей: по нему ядро узнаёт, какие пары элементов реагируют —
  // включая добавленные модами.
  const mixCalls = code.match(/\(0,([a-zA-Z$_]+)\.(hu|I0)\)\(/g) || [];
  code = code.replace(
    /\(0,([a-zA-Z$_]+)\.(hu|I0)\)\(/g,
    '(self.__mixModule=$1,(0,$1.$2))('
  );

  code = code.replace(/\(0,([a-zA-Z$_]+)\.lR\)\(/g, '__nativeChunk((self.__cellOps=$1).lR,');

  // Режим планировщика 1 обходит мир целыми колонками — эту точку тоже надо
  // перехватить, иначе ядро не получает работы вовсе.
  const columnCalls = code.match(/\(0,([a-zA-Z$_]+)\.l6\)\(/g) || [];
  code = code.replace(/\(0,([a-zA-Z$_]+)\.l6\)\(/g, '__nativeColumn((self.__cellOps=$1).l6,$1.lR,');

  const bridge = BRIDGE.replace('__NATIVE_MODULE_PATH__', JSON.stringify(modulePath));
  fs.writeFileSync(workerFile, bridge + code, 'utf8');

  return {
    chunkCalls: calls.length,
    columnCalls: columnCalls.length,
    grid: gridCalls.length,
    matter: matterRefs.length,
    mixes: mixCalls.length,
  };
}

module.exports = { patchWorker, MARK };
