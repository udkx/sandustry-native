//! Отбор чанков, которые ядро может посчитать целиком.
//!
//! Главное правило проекта: браться только за то, за что отвечаем полностью.
//! Клетка, у типа которой есть мод-перехватчик, или клетка рядом со структурой,
//! требует вещей, которых в ядре нет и не будет — произвольного JS модов,
//! фильтров, коллекторов. Такие чанки целиком уходят старому коду.
//!
//! Проверка обязана быть дешёвой: она выполняется перед каждым чанком каждый
//! тик. Поэтому здесь только чтение байтов из массивов, которые игра и так
//! держит в разделяемой памяти.

use crate::physics::{Matter, MatterTable};
use crate::view::{is_element, World, ELEMENT_MIN};

/// Что известно о чанке до его обработки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Ядро берёт чанк целиком.
    Native,
    /// Чанк уходит в JS, вот почему.
    Skip(Reason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// В чанке есть тип элемента, которого нет в таблице материи.
    UnknownMatter,
    /// На тип элемента в чанке подписан мод-перехватчик.
    ModHook,
    /// В чанке или рядом стоит структура: фильтр, конвейер, коллектор.
    Structure,
    /// Таймер элемента истёк, и что с ним делать дальше — решает JS. Причина
    /// осталась в перечислении ради совместимости счётчиков: чанк целиком по
    /// ней больше не отдаётся, только отдельная клетка событием.
    Duration,
}

/// Внешние данные, по которым принимается решение.
///
/// Обе маски игра уже держит в разделяемой памяти, так что читаем мы их
/// напрямую, без единого вызова в JS.
pub struct Gate<'a> {
    /// Маска подписчиков мод-перехватчиков по типу элемента: ненулевой байт —
    /// значит на этот тип кто-то подписан и клетку трогать нельзя.
    pub mod_hooks: &'a [u8],
    /// Сетка структур: тайл 4x4 клетки, ненулевой байт — здесь машина.
    pub block_types: &'a [u8],
    /// Ширина сетки структур в тайлах.
    pub block_width: i32,
    /// Сколько клеток в стороне тайла структур.
    pub block_scale: i32,
}

impl<'a> Gate<'a> {
    #[inline(always)]
    fn has_structure(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        if self.block_types.is_empty() || self.block_scale <= 0 {
            return false;
        }
        // Границы расширены на клетку: структура за краем чанка всё равно
        // дотягивается до него своими портами.
        let tx0 = (x0 - 1).div_euclid(self.block_scale).max(0);
        let ty0 = (y0 - 1).div_euclid(self.block_scale).max(0);
        let tx1 = (x1 + 1).div_euclid(self.block_scale);
        let ty1 = (y1 + 1).div_euclid(self.block_scale);

        for ty in ty0..=ty1 {
            let row = ty * self.block_width;
            for tx in tx0..=tx1 {
                let i = (row + tx) as usize;
                if i < self.block_types.len() && self.block_types[i] != 0 {
                    return true;
                }
            }
        }
        false
    }
}

/// Осмотр чанка перед обработкой.
///
/// Проход по клеткам чанка нужен всё равно — физика их сейчас же и обойдёт, —
/// но здесь он дешевле: только чтение типа и двух байтов из масок, без
/// арифметики движения.
pub fn inspect(
    world: &World,
    table: &MatterTable,
    gate: &Gate,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Verdict {
    if gate.has_structure(x0, y0, x1, y1) {
        return Verdict::Skip(Reason::Structure);
    }

    for y in y0..y1 {
        let row = (y * world.width) as usize;
        for x in x0..x1 {
            let id = world.cells[row + x as usize];
            if !is_element(id) {
                continue;
            }
            let slot = (id - ELEMENT_MIN) as usize;
            if slot >= world.elements.kind.len() {
                return Verdict::Skip(Reason::UnknownMatter);
            }
            let kind = world.elements.kind[slot];
            if kind == 0 {
                continue;
            }
            let k = kind as usize;
            if k < gate.mod_hooks.len() && gate.mod_hooks[k] != 0 {
                return Verdict::Skip(Reason::ModHook);
            }
            if table.matter(kind) == Matter::Other {
                return Verdict::Skip(Reason::UnknownMatter);
            }
        }
    }
    Verdict::Native
}

/// Кэш вердиктов по чанкам.
///
/// Осмотр — это полный проход по клеткам чанка, и делать его каждый тик
/// означает добавить к работе ещё один скан: взяли чанк — сканируем дважды,
/// отдали в JS — тоже дважды. Именно на этом первая версия теряла больше, чем
/// выигрывала.
///
/// Состав чанка меняется медленно: материалы не появляются из ниоткуда, машины
/// строятся руками. Поэтому вердикт держится несколько тиков и обновляется по
/// кругу — так стоимость осмотра размазывается, а реакция на постройку машины
/// остаётся в пределах доли секунды.
pub struct VerdictCache {
    verdicts: Vec<u8>,
    age: Vec<u16>,
    lifetime: u16,
}

const VERDICT_UNKNOWN: u8 = 0;
const VERDICT_NATIVE: u8 = 1;
const VERDICT_SKIP_BASE: u8 = 2;

impl VerdictCache {
    pub fn new(chunks: usize, lifetime: u16) -> Self {
        VerdictCache {
            verdicts: vec![VERDICT_UNKNOWN; chunks],
            age: vec![0; chunks],
            lifetime,
        }
    }

    /// Вердикт, если он ещё свеж.
    pub fn get(&mut self, chunk: usize) -> Option<Verdict> {
        if chunk >= self.verdicts.len() {
            return None;
        }
        if self.age[chunk] == 0 {
            return None;
        }
        self.age[chunk] -= 1;
        match self.verdicts[chunk] {
            VERDICT_NATIVE => Some(Verdict::Native),
            v if v >= VERDICT_SKIP_BASE => Some(Verdict::Skip(match v - VERDICT_SKIP_BASE {
                0 => Reason::UnknownMatter,
                1 => Reason::ModHook,
                2 => Reason::Structure,
                _ => Reason::Duration,
            })),
            _ => None,
        }
    }

    pub fn put(&mut self, chunk: usize, verdict: Verdict) {
        if chunk >= self.verdicts.len() {
            return;
        }
        self.verdicts[chunk] = match verdict {
            Verdict::Native => VERDICT_NATIVE,
            Verdict::Skip(Reason::UnknownMatter) => VERDICT_SKIP_BASE,
            Verdict::Skip(Reason::ModHook) => VERDICT_SKIP_BASE + 1,
            Verdict::Skip(Reason::Structure) => VERDICT_SKIP_BASE + 2,
            Verdict::Skip(Reason::Duration) => VERDICT_SKIP_BASE + 3,
        };
        self.age[chunk] = self.lifetime;
    }

    /// Сброс — например, когда изменилась версия структур.
    pub fn clear(&mut self) {
        self.age.fill(0);
    }
}

/// Сводка за тик: сколько чанков ядро взяло и сколько вернуло, с причинами.
/// Нужна не для красоты — по ней видно, окупается ли затея на живом мире.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Чанки, за которые никто не брался: спят, и margin никого к ним не
    /// подтянул. Их не считает ни ядро, ни JS — как и в оригинале.
    pub sleeping: u32,
    pub native_chunks: u32,
    pub skipped_unknown: u32,
    pub skipped_hooks: u32,
    pub skipped_structures: u32,
    pub skipped_duration: u32,
    pub cells_touched: u32,
    /// Сколько раз вердикт взят из кэша, без прохода по клеткам.
    pub cached: u32,
    /// Сколько клеток реально сдвинулось. Без этого числа непонятно, работает
    /// физика или ядро исправно обходит мир, ничего в нём не меняя.
    pub moved: u32,
}

impl Stats {
    pub fn record(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Native => self.native_chunks += 1,
            Verdict::Skip(Reason::UnknownMatter) => self.skipped_unknown += 1,
            Verdict::Skip(Reason::ModHook) => self.skipped_hooks += 1,
            Verdict::Skip(Reason::Structure) => self.skipped_structures += 1,
            Verdict::Skip(Reason::Duration) => self.skipped_duration += 1,
        }
    }

    pub fn total_chunks(&self) -> u32 {
        self.native_chunks
            + self.skipped_unknown
            + self.skipped_hooks
            + self.skipped_structures
            + self.skipped_duration
    }
}

/// Окно обхода чанка — арифметика из `cellOps.E` и `cellOps.k`.
///
/// Игра обходит не сам чанк, а окно, сдвинутое на `margin`: так граница между
/// полосами потоков каждый кадр приходится на разное место, и шов не
/// застывает. Обходить строго по границам чанка — значит оставить полосу
/// шириной `margin` на стыке активного и спящего чанка не посчитанной никем.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Window {
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

/// Стоит ли вообще браться за чанк — ранний выход из `cellOps.E`.
///
/// Спящий чанк игра пропускает мгновенно, не касаясь его клеток. Но если
/// окно сдвинуто margin'ом, часть активного соседа заезжает внутрь спящего —
/// тогда чанк всё-таки обходится. Без этой проверки ядро сканирует спящие
/// чанки целиком (дорого) и при этом теряет полосу на стыке (неверно).
pub fn should_process(world: &World, cx: i32, cy: i32, margin_x: i32, margin_y: i32) -> bool {
    if world.chunk_active(cx, cy) {
        return true;
    }
    if margin_x > 0 && world.chunk_active(cx + 1, cy) {
        return true;
    }
    if margin_y > 0 && world.chunk_active(cx, cy + 1) {
        return true;
    }
    if margin_x > 0 && margin_y > 0 && world.chunk_active(cx + 1, cy + 1) {
        return true;
    }
    false
}

/// То же для обхода колонкой (`cellOps.k`): margin там только по X, и сосед
/// берётся с той стороны, куда сдвинуто окно.
pub fn should_process_in_column(world: &World, cx: i32, cy: i32, margin_x: i32) -> bool {
    if world.chunk_active(cx, cy) {
        return true;
    }
    if margin_x == 0 {
        return false;
    }
    let neighbour = if margin_x > 0 { cx + 1 } else { cx - 1 };
    world.chunk_active(neighbour, cy)
}

/// Окно обхода. `margin_y = 0` даёт окно обхода колонкой — она сдвигает только
/// по горизонтали.
pub fn window(world: &World, cx: i32, cy: i32, margin_x: i32, margin_y: i32) -> Window {
    let cs = world.chunk_size;

    let left = cx * cs + margin_x;
    let right = cx * cs + cs + margin_x;
    // Нулевой чанк начинается с края мира: сдвигать его влево некуда.
    let x0 = if cx == 0 { 0 } else { left.min(world.width) };
    let x1 = right.min(world.width);

    let top = cy * cs + margin_y;
    let bottom = cy * cs + cs + margin_y;
    let y0 = if cy == 0 { 0 } else { top.min(world.height) };
    let y1 = bottom.min(world.height);

    Window { x0: x0.max(0), y0: y0.max(0), x1, y1 }
}
