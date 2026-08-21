//! Доступ к состоянию симуляции Sandustry.
//!
//! Игра держит весь мир в одном SharedArrayBuffer: сетка идентификаторов клеток
//! плюс структура массивов по элементам. Здесь — вид на эту память, устроенный
//! так, чтобы одинаково работать и с настоящим буфером игры, и со снимком,
//! загруженным из файла. Отсюда срезы, а не указатели: владение остаётся
//! снаружи, мы только смотрим.
//!
//! Раскладка полей повторяет `createSimState` игры. Порядок и типы взяты из
//! разбора её кода и обязаны совпадать байт в байт — иначе мы будем читать
//! чужие данные и не заметим этого.

/// Идентификатор клетки: 0 — пусто, 1..1000 — терраин, 1001..1e6 — повреждённая
/// земля, от 1_000_001 — элементы.
pub type CellId = u32;

pub const EMPTY: CellId = 0;
pub const TERRAIN_MAX: CellId = 1000;
pub const DAMAGED_MIN: CellId = 1001;
pub const ELEMENT_MIN: CellId = 1_000_001;

#[inline(always)]
pub fn is_element(id: CellId) -> bool {
    id >= ELEMENT_MIN
}

#[inline(always)]
pub fn is_terrain(id: CellId) -> bool {
    id != EMPTY && id <= TERRAIN_MAX
}

#[inline(always)]
pub fn element_index(id: CellId) -> usize {
    (id - ELEMENT_MIN) as usize
}

/// Поля элементов. Игра хранит их отдельными массивами, индексируемыми номером
/// слота, а не позицией — поэтому доступ к соседней клетке почти всегда бьёт
/// мимо кэша. Это её свойство, и мы его наследуем: любая попытка «улучшить»
/// раскладку здесь означала бы несовместимость с форматом сохранений.
pub struct Elements<'a> {
    pub kind: &'a mut [u8],
    pub velocity_x: &'a mut [f32],
    pub velocity_y: &'a mut [f32],
    pub min_velocity_y: &'a mut [f32],
    pub threshold_x: &'a mut [f32],
    pub threshold_y: &'a mut [f32],
    pub density: &'a mut [f32],
    pub is_free_falling: &'a mut [u8],
    pub has_been_updated: &'a mut [u8],
    pub skip_physics: &'a mut [u8],
    pub x: &'a mut [u16],
    pub y: &'a mut [u16],
    /// Знаковое поле: легко перепутать с беззнаковым и получить движение
    /// только в одну сторону.
    pub last_side_checked: &'a mut [i16],
    pub moves_y_axis: &'a mut [u16],
    pub moves_y_axis_count: &'a mut [u16],
}

/// Уровни `skipPhysics`: конвейеры и машины так забирают элемент под свой
/// контроль, полностью или частично.
pub mod skip {
    pub const NORMAL: u8 = 0;
    pub const SKIP: u8 = 1;
    pub const AGGRESSIVE: u8 = 2;
}

/// Вид на мир: сетка и элементы.
pub struct World<'a> {
    pub width: i32,
    pub height: i32,
    pub chunk_size: i32,
    pub cells: &'a mut [CellId],
    pub elements: Elements<'a>,
}

impl<'a> World<'a> {
    #[inline(always)]
    pub fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.width + x) as usize
    }

    #[inline(always)]
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.width && y < self.height
    }

    #[inline(always)]
    pub fn cell(&self, x: i32, y: i32) -> CellId {
        if self.in_bounds(x, y) {
            self.cells[self.idx(x, y)]
        } else {
            // За краем мира — как стена: туда нельзя ни упасть, ни всплыть.
            TERRAIN_MAX
        }
    }

    /// Плотность того, что лежит в клетке. Для пустоты — ноль, для терраина —
    /// бесконечность: его не вытеснить ничем.
    #[inline(always)]
    pub fn density_at(&self, x: i32, y: i32) -> f32 {
        let id = self.cell(x, y);
        if id == EMPTY {
            return 0.0;
        }
        if !is_element(id) {
            return f32::INFINITY;
        }
        let i = element_index(id);
        if i < self.elements.density.len() {
            self.elements.density[i]
        } else {
            f32::INFINITY
        }
    }

    /// Может ли элемент с такой плотностью занять клетку. Правило то же, что в
    /// `shouldSwapByDensity` игры, без ветки фильтров — она приедет вместе с
    /// поддержкой структур.
    #[inline(always)]
    pub fn can_enter(&self, x: i32, y: i32, density: f32) -> bool {
        let id = self.cell(x, y);
        if id == EMPTY {
            return true;
        }
        if !is_element(id) {
            return false;
        }
        density > self.density_at(x, y)
    }

    /// Перестановка двух клеток вместе с координатами в полях элементов.
    ///
    /// Координаты обязаны ехать следом: игра держит `x`/`y` в самих элементах и
    /// полагается на них в машинах, конвейерах и сохранении. Рассинхрон здесь
    /// не заметен сразу и всплывает потом как элемент, застрявший в стене.
    #[inline(always)]
    pub fn swap_cells(&mut self, ax: i32, ay: i32, bx: i32, by: i32) {
        let a = self.idx(ax, ay);
        let b = self.idx(bx, by);
        self.cells.swap(a, b);

        let ida = self.cells[a];
        if is_element(ida) {
            let i = element_index(ida);
            self.elements.x[i] = ax as u16;
            self.elements.y[i] = ay as u16;
        }
        let idb = self.cells[b];
        if is_element(idb) {
            let i = element_index(idb);
            self.elements.x[i] = bx as u16;
            self.elements.y[i] = by as u16;
        }
    }
}
