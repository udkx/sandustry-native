/* Проверка модуля без запуска игры.
 *
 * Собирает игрушечный мир той же формы, что у Sandustry — сетка
 * идентификаторов плюс структура массивов по элементам, — и смотрит три вещи:
 * песок долетает до дна, изменения видны в тех же самых массивах JS (то есть
 * копирования нет), и чанк с мод-перехватчиком ядро не трогает.
 */
'use strict';

const assert = require('assert');
const native = require('./sand.node');

const W = 32, H = 32, CHUNK = 32, CAP = 64;
const ELEMENT_MIN = 1000001;

function makeWorld() {
  const arrays = {
    cells: new Uint32Array(W * H),
    type: new Uint8Array(CAP),
    velocityX: new Float32Array(CAP),
    velocityY: new Float32Array(CAP),
    minVelocityY: new Float32Array(CAP),
    thresholdX: new Float32Array(CAP),
    thresholdY: new Float32Array(CAP),
    density: new Float32Array(CAP),
    isFreeFalling: new Uint8Array(CAP),
    hasBeenUpdated: new Uint8Array(CAP),
    skipPhysics: new Uint8Array(CAP),
    x: new Uint16Array(CAP),
    y: new Uint16Array(CAP),
    lastSideChecked: new Int16Array(CAP),
    movesYAxis: new Uint16Array(CAP),
    movesYAxisCount: new Uint16Array(CAP),
    modHooks: new Uint8Array(256),
    blockTypes: new Uint8Array(64),
    needsJsBuffer: new Int32Array(4096),
  };
  const sim = new native.NativeSim({
    width: W, height: H, chunkSize: CHUNK,
    blockWidth: 8, blockScale: 4,
    ...arrays,
  });
  // Тип 1 — сыпучее, тип 2 — жидкость с растеканием на 5 клеток.
  sim.setMatter(1, 1, 0);
  sim.setMatter(2, 2, 5);
  return { sim, arrays };
}

function put(arrays, slot, x, y, kind, density) {
  arrays.type[slot] = kind;
  arrays.density[slot] = density;
  arrays.x[slot] = x;
  arrays.y[slot] = y;
  arrays.cells[y * W + x] = ELEMENT_MIN + slot;
}

function run(sim, arrays, ticks) {
  for (let t = 0; t < ticks; t++) {
    // Игра гасит флаги в конце тика по списку тронутых элементов; здесь такого
    // списка нет, поэтому сбрасываем сами — иначе каждый второй кадр пройдёт
    // вхолостую.
    arrays.hasBeenUpdated.fill(t % 2 === 0 ? 1 : 0);
    sim.updateChunk(0, 0, t);
  }
}

// --- песок падает, и это видно в исходных массивах ------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15; // терраин-пол
  put(arrays, 0, 16, 0, 1, 1600);

  const before = arrays.cells.indexOf(ELEMENT_MIN);
  run(sim, arrays, 90);
  const after = arrays.cells.indexOf(ELEMENT_MIN);

  assert.strictEqual(before, 16, 'песчинка должна начинать сверху');
  assert.strictEqual(after, 30 * W + 16, `песчинка обязана лечь на пол, а лежит в ${after}`);
  assert.strictEqual(arrays.y[0], 30, 'координата в полях разошлась с сеткой');
  console.log('песок падает, массивы JS изменены на месте — копирования нет');
}

// --- чанк с мод-перехватчиком ядро не трогает -----------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  put(arrays, 0, 16, 0, 1, 1600);
  arrays.modHooks[1] = 1; // на песок подписан мод

  const result = sim.updateChunk(0, 0, 0);
  assert.strictEqual(result.native, false, 'чанк с хуком нельзя брать');
  assert.strictEqual(result.reason, 2, 'причина должна быть «мод-хук»');
  assert.strictEqual(arrays.cells[16], ELEMENT_MIN, 'клетку не должны были сдвинуть');
  console.log('чанк с мод-перехватчиком возвращён в JS нетронутым');
}

// --- чанк со структурой рядом тоже уходит в JS ----------------------------
{
  const { sim, arrays } = makeWorld();
  put(arrays, 0, 16, 4, 1, 1600);
  arrays.blockTypes[10] = 7; // машина в сетке структур

  const result = sim.updateChunk(0, 0, 0);
  assert.strictEqual(result.native, false, 'рядом структура — не наш чанк');
  assert.strictEqual(result.reason, 3, 'причина должна быть «структура»');
  console.log('чанк со структурой возвращён в JS');
}

// --- незнакомый материал -------------------------------------------------
{
  const { sim, arrays } = makeWorld();
  put(arrays, 0, 16, 4, 200, 1000); // тип, которого нет в таблице

  const result = sim.updateChunk(0, 0, 0);
  assert.strictEqual(result.native, false, 'незнакомый материал считать нельзя');
  assert.strictEqual(result.reason, 1, 'причина должна быть «незнакомый материал»');
  console.log('незнакомый материал возвращён в JS');
}

// --- статистика ----------------------------------------------------------
{
  const { sim, arrays } = makeWorld();
  for (let x = 0; x < W; x++) arrays.cells[31 * W + x] = 15;
  for (let i = 0; i < 20; i++) put(arrays, i, i + 5, 10, 1, 1600);
  run(sim, arrays, 30);

  const [nativeChunks, unknown, hooks, structures, cells] = sim.stats();
  assert.ok(nativeChunks > 0, 'ядро должно было взять хотя бы один чанк');
  assert.ok(cells > 0, 'и обработать клетки');
  console.log(
    `статистика: взято чанков ${nativeChunks}, отдано (материал/хук/структура) ` +
    `${unknown}/${hooks}/${structures}, обработано клеток ${cells}`
  );
}

console.log('\nвсе проверки прошли');
