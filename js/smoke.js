/* Проверка модуля без запуска игры.
 *
 * Собирает игрушечный мир той же формы, что у Sandustry — сетка
 * идентификаторов плюс структура массивов по элементам, — и смотрит, что
 * держится граница с игрой: песок долетает до дна, изменения видны в тех же
 * самых массивах JS (то есть копирования нет), посчитанные слоты возвращаются
 * наружу списком, а чанк с мод-перехватчиком, структурой или таймером ядро не
 * трогает вовсе.
 */
'use strict';

const assert = require('assert');
const native = require('./sand.node');

const W = 32, H = 32, CHUNK = 32, CAP = 64;
const ELEMENT_MIN = 1000001;
const DT = 1 / 60;

/** Состояния материи игры (`enums_38163.es`). */
const SOLID = 1, LIQUID = 2, GAS = 4;

function makeWorld() {
  const arrays = {
    cells: new Uint32Array(W * H),
    type: new Uint8Array(CAP),
    velocityX: new Float32Array(CAP),
    velocityY: new Float32Array(CAP),
    minVelocityX: new Float32Array(CAP),
    minVelocityY: new Float32Array(CAP),
    thresholdX: new Float32Array(CAP),
    thresholdY: new Float32Array(CAP),
    density: new Float32Array(CAP),
    isFreeFalling: new Uint8Array(CAP),
    hasBeenUpdated: new Uint8Array(CAP),
    skipPhysics: new Uint8Array(CAP),
    hasDuration: new Uint8Array(CAP),
    durationLeft: new Float32Array(CAP),
    x: new Uint16Array(CAP),
    y: new Uint16Array(CAP),
    lastSideChecked: new Int16Array(CAP),
    movesYAxis: new Uint16Array(CAP),
    movesYAxisCount: new Uint16Array(CAP),
    modHooks: new Uint8Array(256),
    blockTypes: new Uint8Array(64),
    needsJsBuffer: new Int32Array(4096),
    // Флаги активности чанков: текущий кадр читаем, следующий пишем.
    chunkShouldUpdate: new Uint8Array(4).fill(1),
    chunkShouldUpdateNext: new Uint8Array(4),
    // Таблица терраина: в игре по id клетки лежит её тип, здесь — то же самое.
    terrainType: Uint8Array.from({ length: 1001 }, (_, i) => i & 255),
    // Сюда ядро выкладывает слоты, которые посчитало.
    updatedBuffer: new Int32Array(4096),
    // А сюда — индексы клеток, которые изменило: по ним игра перерисовывает
    // растр. Сетку ядро правит само, картинку — только через свою функцию.
    changedBuffer: new Int32Array(1 << 16),
    // Номера чанков колонки, за которые ядро не взялось.
    deferredBuffer: new Int32Array(256),
  };
  const sim = new native.NativeSim({
    width: W, height: H, chunkSize: CHUNK,
    blockWidth: 8, blockScale: 4,
    ...arrays,
  });
  // Песок в игре — Solid, а не Powder: у Powder обработчика движения нет вовсе.
  sim.setMatter(1, SOLID, 1);
  sim.setMatter(2, LIQUID, 1);
  sim.setMatter(3, GAS, 1);
  return { sim, arrays };
}

function put(arrays, slot, x, y, kind, density) {
  arrays.type[slot] = kind;
  arrays.density[slot] = density;
  arrays.x[slot] = x;
  arrays.y[slot] = y;
  arrays.cells[y * W + x] = ELEMENT_MIN + slot;
}

/** Тик так, как его сделает игра: посчитать, забрать список, погасить флаги. */
function run(sim, arrays, ticks) {
  for (let t = 0; t < ticks; t++) {
    sim.updateChunk(0, 0, 0, 0, t % 2 === 0, DT);
    // В игре здесь по списку изменённых клеток идёт `cellOps.Gz`, который
    // обновляет растр. Здесь достаточно убедиться, что список приходит.
    sim.takeChanged();
    const n = sim.takeUpdated();
    // Игра гасит флаги в конце тика по своему списку `updatedElementIndices`.
    // Здесь делаем то же самое — по списку, который вернуло ядро.
    for (let i = 0; i < n; i++) arrays.hasBeenUpdated[arrays.updatedBuffer[i]] = 0;
  }
}

// --- песок падает, и это видно в исходных массивах ------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15; // терраин-пол
  put(arrays, 0, 16, 0, 1, 150);

  const before = arrays.cells.indexOf(ELEMENT_MIN);
  run(sim, arrays, 90);
  const after = arrays.cells.indexOf(ELEMENT_MIN);

  assert.strictEqual(before, 16, 'песчинка должна начинать сверху');
  assert.strictEqual(after, 30 * W + 16, `песчинка обязана лечь на пол, а лежит в ${after}`);
  assert.strictEqual(arrays.y[0], 30, 'координата в полях разошлась с сеткой');
  console.log('песок падает, массивы JS изменены на месте — копирования нет');
}

// --- изменённые клетки возвращаются наружу для перерисовки ---------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);

  let changed = 0;
  for (let t = 0; t < 30 && changed === 0; t++) {
    sim.updateChunk(0, 0, 0, 0, true, DT);
    changed = sim.takeChanged();
  }
  assert.ok(changed >= 2, 'перемещение меняет две клетки: откуда и куда');
  const seen = new Set();
  for (let i = 0; i < changed; i++) seen.add(arrays.changedBuffer[i]);
  assert.ok(seen.size >= 2, 'в списке должны быть обе клетки');
  assert.strictEqual(sim.takeChanged(), 0, 'после выборки список должен опустеть');
  console.log('список изменённых клеток возвращается — растр перерисует игра');
}

// --- посчитанные слоты возвращаются наружу --------------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);

  let reported = 0;
  for (let t = 0; t < 30 && reported === 0; t++) {
    sim.updateChunk(0, 0, 0, 0, true, DT);
    reported = sim.takeUpdated();
  }
  assert.ok(reported > 0, 'ядро обязано вернуть список посчитанных слотов');
  assert.strictEqual(arrays.updatedBuffer[0], 0, 'в списке должен быть слот песчинки');
  assert.strictEqual(arrays.hasBeenUpdated[0], 1, 'и она должна быть помечена');
  console.log('список посчитанных слотов возвращается в JS — флаги погасит игра');
}

// --- спящий чанк не считается --------------------------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);
  arrays.chunkShouldUpdate.fill(0); // игра этот чанк в кадре не считает

  run(sim, arrays, 30);
  assert.strictEqual(arrays.cells[16], ELEMENT_MIN, 'клетку спящего чанка двигать нельзя');
  console.log('спящий чанк не тронут');
}

// --- чанк с мод-перехватчиком ядро не трогает -----------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);
  arrays.modHooks[1] = 1; // на песок подписан мод

  const result = sim.updateChunk(0, 0, 0, 0, true, DT);
  assert.strictEqual(result.native, false, 'чанк с хуком нельзя брать');
  assert.strictEqual(result.reason, 2, 'причина должна быть «мод-хук»');
  assert.strictEqual(arrays.cells[16], ELEMENT_MIN, 'клетку не должны были сдвинуть');
  console.log('чанк с мод-перехватчиком возвращён в JS нетронутым');
}

// --- чанк со структурой рядом тоже уходит в JS ----------------------------
{
  const { sim, arrays } = makeWorld();
  put(arrays, 0, 16, 4, 1, 150);
  arrays.blockTypes[10] = 7; // машина в сетке структур

  const result = sim.updateChunk(0, 0, 0, 0, true, DT);
  assert.strictEqual(result.native, false, 'рядом структура — не наш чанк');
  assert.strictEqual(result.reason, 3, 'причина должна быть «структура»');
  console.log('чанк со структурой возвращён в JS');
}

// --- незнакомый материал -------------------------------------------------
{
  const { sim, arrays } = makeWorld();
  put(arrays, 0, 16, 4, 200, 1000); // тип, которого нет в таблице

  const result = sim.updateChunk(0, 0, 0, 0, true, DT);
  assert.strictEqual(result.native, false, 'незнакомый материал считать нельзя');
  assert.strictEqual(result.reason, 1, 'причина должна быть «незнакомый материал»');
  console.log('незнакомый материал возвращён в JS');
}

// --- живой таймер не мешает физике, истёкший уходит в JS -----------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);
  arrays.hasDuration[0] = 1;
  arrays.durationLeft[0] = 10;   // семя растёт, огонь горит

  const result = sim.updateChunk(0, 0, 0, 0, true, DT);
  assert.strictEqual(result.native, true, 'чанк с живым таймером ядро берёт');
  assert.ok(arrays.durationLeft[0] < 10, 'и таймер обязан тикать');

  // Истёкший таймер — уже не физика: там превращение, звук и экономика.
  arrays.durationLeft[0] = 0.001;
  sim.updateChunk(0, 0, 0, 0, true, DT);
  const pending = sim.takeNeedsJs();
  assert.ok(pending > 0, 'истёкший таймер обязан уйти в JS');
  assert.strictEqual(arrays.needsJsBuffer[0], 16, 'в буфере — координата клетки');
  console.log('таймер тикает в ядре, истёкший отдаётся игре');
}

// --- спящий чанк пропускается сразу, без скана клеток ---------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);
  arrays.chunkShouldUpdate.fill(0);

  const result = sim.updateChunk(0, 0, 0, 0, true, DT);
  assert.strictEqual(result.native, true, 'спящий чанк ядро берёт на себя');
  assert.strictEqual(result.touched, 0, 'и не касается его клеток — как игра');
  const sleeping = sim.stats()[8];
  assert.ok(sleeping > 0, 'спящие чанки должны считаться отдельно');
  console.log('спящий чанк пропускается мгновенно, счётчик ведётся');
}

// --- колонка считается одним вызовом -------------------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 150);

  // Колонка целиком: пять тысяч переходов JS→native за тик — дороже самой
  // физики, поэтому обход колонкой живёт внутри ядра.
  for (let t = 0; t < 90; t++) {
    const deferred = sim.updateColumn(0, 0, t % 2 === 0, DT);
    assert.strictEqual(deferred, 0, 'обычную колонку ядро берёт целиком');
    sim.takeChanged();
    const n = sim.takeUpdated();
    for (let i = 0; i < n; i++) arrays.hasBeenUpdated[arrays.updatedBuffer[i]] = 0;
  }
  assert.strictEqual(arrays.cells[30 * W + 16], ELEMENT_MIN, 'песчинка обязана лечь на пол');
  console.log('колонка считается одним вызовом, песок долетает');
}

// --- статистика ----------------------------------------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  for (let i = 0; i < 20; i++) put(arrays, i, i + 5, 10, 1, 150);
  run(sim, arrays, 30);

  const [nativeChunks, unknown, hooks, structures, cells, , moved, duration] = sim.stats();
  assert.ok(nativeChunks > 0, 'ядро должно было взять хотя бы один чанк');
  assert.ok(cells > 0, 'и обработать клетки');
  assert.ok(moved > 0, 'и хоть что-то подвинуть');
  console.log(
    `статистика: взято чанков ${nativeChunks}, отдано (материал/хук/структура/таймер) ` +
    `${unknown}/${hooks}/${structures}/${duration}, клеток ${cells}, сдвинуто ${moved}`
  );
}

console.log('\nвсе проверки прошли');
