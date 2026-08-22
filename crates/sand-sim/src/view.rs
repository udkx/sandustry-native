//! Доступ к состоянию симуляции Sandustry.
//!
//! Игра держит весь мир в одном SharedArrayBuffer: сетка идентификаторов клеток
//! плюс структура массивов по элементам. Здесь — вид на эту память, устроенный
//! так, чтобы одинаково работать и с настоящим буфером игры, и со снимком,
//! загруженным из файла. Отсюда срезы, а не указатели: владение остаётся
//! снаружи, мы только смотрим.
//!
//! Раскладка полей повторяет `createSimState` игры
//! (`deobf/simulation-worker/BYTES_PER_ELEMENT_27657.js`). Порядок и типы взяты
//! из разбора её кода и обязаны совпадать байт в байт — иначе мы будем читать
//! чужие данные и не заметим этого.

/// Идентификатор клетки: 0 — пусто, 1..1000 — терраин, 1001..1e6 — повреждённая
/// земля, от 1_000_001 — элементы.
pub type CellId = u32;

pub const EMPTY: CellId = 0;
pub const TERRAIN_MAX: CellId = 1000;
pub const DAMAGED_MIN: CellId = 1001;
pub const DAMAGED_MAX: CellId = 1_000_000;
pub const ELEMENT_MIN: CellId = 1_000_001;
pub const ELEMENT_MAX: CellId = 2_000_000;

/// Что возвращает чтение за краем мира.
///
/// Игра границ не проверяет вовсе: `getCellId` читает `cellIds[y*width + x]`, и
/// за верхним краем получает `undefined`, а за боковым — клетку соседней
/// строки. Первое ведёт себя как «занято и это не элемент», второе — редкий
/// случай на самой кромке карты. Мы воспроизводим первое поведение для всех
/// сторон: за краем стена, туда нельзя ни упасть, ни всплыть.
pub const OUT_OF_BOUNDS: CellId = TERRAIN_MAX;

#[inline(always)]
pub fn is_element(id: CellId) -> bool {
    (ELEMENT_MIN..=ELEMENT_MAX).contains(&id)
}

#[inline(always)]
pub fn is_terrain(id: CellId) -> bool {
    id != EMPTY && id <= TERRAIN_MAX
}

#[inline(always)]
pub fn element_index(id: CellId) -> usize {
    (id - ELEMENT_MIN) as usize
}

/// Типы терраина, которые важны физике. Числа — из `enums_38163.vZ`.
pub mod terrain {
    pub const EMPTY: u8 = 0;
    pub const BLOCK: u8 = 15;
    pub const SLIDING_BLOCK_LEFT: u8 = 17;
    pub const SLIDING_BLOCK_RIGHT: u8 = 18;
    pub const CONVEYOR_LEFT: u8 = 19;
    pub const CONVEYOR_RIGHT: u8 = 20;
    pub const SHAKER_LEFT: u8 = 21;
    pub const SHAKER_RIGHT: u8 = 22;

    /// `cellOps.XH` — терраин конвейера: он двигает элементы сам, поэтому
    /// диагональный сход с него запрещён.
    #[inline(always)]
    pub fn is_conveyor(t: u8) -> bool {
        matches!(t, CONVEYOR_LEFT | CONVEYOR_RIGHT | SHAKER_LEFT | SHAKER_RIGHT)
    }
}

/// Поля элементов. Игра хранит их отдельными массивами, индексируемыми номером
/// слота, а не позицией — поэтому доступ к соседней клетке почти всегда бьёт
/// мимо кэша. Это её свойство, и мы его наследуем: любая попытка «улучшить»
/// раскладку здесь означала бы несовместимость с форматом сохранений.
pub struct Elements<'a> {
    pub kind: &'a mut [u8],
    pub velocity_x: &'a mut [f32],
    pub velocity_y: &'a mut [f32],
    pub min_velocity_x: &'a mut [f32],
    pub min_velocity_y: &'a mut [f32],
    pub threshold_x: &'a mut [f32],
    pub threshold_y: &'a mut [f32],
    pub density: &'a mut [f32],
    pub is_free_falling: &'a mut [u8],
    pub has_been_updated: &'a mut [u8],
    pub skip_physics: &'a mut [u8],
    /// `hasDuration` — у элемента крутится таймер: семена растут, огонь гаснет,
    /// ферма начисляет очки.
    pub has_duration: &'a mut [u8],
    /// Сколько таймеру осталось. Пока он тикает, элемент живёт обычной
    /// физикой — игра уменьшает счётчик и идёт дальше. В JS уходит только
    /// момент истечения: там превращение, удаление и события модов.
    pub duration_left: &'a mut [f32],
    pub x: &'a mut [u16],
    pub y: &'a mut [u16],
    /// Знаковое поле, и хранится в нём **X-координата последней проверенной
    /// стороны**, а не направление. Игра пишет туда `d.x[r]`
    /// (`newPosition_8405.js`, функция горизонтального шага), и по нему решает,
    /// будить ли чанк повторно.
    pub last_side_checked: &'a mut [i16],
    /// Y, на котором элемент топчется, и сколько ходов он там сделал. По
    /// счётчику игра испаряет застоявшуюся воду и пар.
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
    /// Флаги «чанк считается в этом кадре». Читать — обязательно: клетка из
    /// margin-полосы соседнего чанка обрабатывается, только если её собственный
    /// чанк активен (`cellOps.Do` → `shouldCellInChunkUpdate`).
    pub chunk_should_update: &'a [u8],
    /// Флаги «чанк считать в следующем кадре».
    ///
    /// Игра держит их двойным буфером и ставит при каждом изменении клетки, а
    /// также когда клетка только копит скорость и никуда не едет. Если их не
    /// ставить, чанк после обработки засыпает: движение идёт несколько кадров,
    /// потом встаёт, пока чанк не разбудит что-то извне.
    pub chunk_dirty_next: &'a mut [u8],
    pub chunk_width: i32,
    pub chunk_height: i32,
    /// Тип терраина по идентификатору клетки — массив `terrainType` игры.
    pub terrain_type: &'a [u8],
    /// Индексы клеток, которые ядро изменило, — `y * width + x`.
    ///
    /// Сетка клеток это ещё не мир. Игра на каждую запись клетки обновляет
    /// растр (`variantFromDataField1.RP` пишет четыре байта RGBA в
    /// `shared.mapData.data`), тень и пост-обработку. Ядро этого делать не
    /// может и не должно — цвета, варианты и материалы живут в замыканиях
    /// модулей игры. Поэтому оно копит список изменённых клеток, а мост после
    /// тика прогоняет по нему настоящий `cellOps.Gz`.
    ///
    /// Без этого списка мир считается верно, но выглядит сломанным: клетка,
    /// откуда элемент уехал, продолжает рисоваться старым цветом, а та, куда
    /// приехал, остаётся фоном. Снаружи это неотличимо от рваной физики.
    pub changed: &'a mut Vec<u32>,
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

    /// `cellAccess.getCellId`.
    #[inline(always)]
    pub fn cell(&self, x: i32, y: i32) -> CellId {
        if self.in_bounds(x, y) {
            self.cells[self.idx(x, y)]
        } else {
            OUT_OF_BOUNDS
        }
    }

    /// `cellOps.lV` → `cellAccess.isCellEmpty`.
    #[inline(always)]
    pub fn is_empty(&self, x: i32, y: i32) -> bool {
        self.cell(x, y) == EMPTY
    }

    /// `cellAccess.getTerrainType`: у обычного терраина тип равен его id, у
    /// повреждённой земли лежит в отдельном массиве. Повреждённую мы читать не
    /// умеем — для физики важно лишь то, что она непроходима, а конвейером или
    /// скользящим блоком повреждённая земля не бывает.
    #[inline(always)]
    pub fn terrain_of(&self, id: CellId) -> u8 {
        if id != EMPTY && id <= TERRAIN_MAX {
            let i = id as usize;
            if i < self.terrain_type.len() {
                return self.terrain_type[i];
            }
            // Снимки мира приходят без таблицы терраина: там id и есть тип.
            return id as u8;
        }
        terrain::EMPTY
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

    /// `cellAccess.shouldChunkUpdate` — активен ли чанк в текущем кадре.
    #[inline(always)]
    pub fn chunk_active(&self, cx: i32, cy: i32) -> bool {
        if cx < 0 || cy < 0 || cx >= self.chunk_width || cy >= self.chunk_height {
            return false;
        }
        let i = (cy * self.chunk_width + cx) as usize;
        self.chunk_should_update.get(i).copied().unwrap_or(0) == 1
    }

    /// `cellOps.Do` → `cellAccess.shouldCellInChunkUpdate`.
    ///
    /// Обход захватывает полосу соседнего чанка (margin), но клетки в ней
    /// считаются только если их собственный чанк активен. Без этой проверки
    /// ядро двигает то, что игра в этом кадре двигать не собиралась.
    #[inline(always)]
    pub fn cell_chunk_active(&self, x: i32, y: i32) -> bool {
        if self.chunk_should_update.is_empty() {
            // Снимок без флагов: считаем всё.
            return true;
        }
        self.chunk_active(x / self.chunk_size, y / self.chunk_size)
    }

    /// `cellOps.Y$` → `cellAccess.reportToChunkAtCellPos`.
    ///
    /// Соседей будим только с той стороны, где клетка стоит на самой кромке, и
    /// только по четырём сторонам — диагоналей игра здесь не трогает. Будить
    /// весь квадрат вокруг каждой песчинки значит держать мир вечно
    /// бодрствующим и потерять смысл этих флагов.
    #[inline(always)]
    pub fn report_to_chunk(&mut self, x: i32, y: i32) {
        if self.chunk_dirty_next.is_empty() || self.chunk_size <= 0 {
            return;
        }
        let cs = self.chunk_size;
        let (cx, cy) = (x / cs, y / cs);
        let (lx, ly) = (x % cs, y % cs);

        self.set_chunk_next(cx, cy);
        if lx == 0 {
            self.set_chunk_next(cx - 1, cy);
        }
        if lx == cs - 1 {
            self.set_chunk_next(cx + 1, cy);
        }
        if ly == 0 {
            self.set_chunk_next(cx, cy - 1);
        }
        if ly == cs - 1 {
            self.set_chunk_next(cx, cy + 1);
        }
    }

    #[inline(always)]
    fn set_chunk_next(&mut self, cx: i32, cy: i32) {
        if cx < 0 || cy < 0 || cx >= self.chunk_width || cy >= self.chunk_height {
            return;
        }
        let i = (cy * self.chunk_width + cx) as usize;
        if i < self.chunk_dirty_next.len() {
            self.chunk_dirty_next[i] = 1;
        }
    }

    /// `cellOps.Gz` → `setCell`: записать идентификатор и разбудить чанк.
    #[inline(always)]
    pub(crate) fn set_cell(&mut self, x: i32, y: i32, id: CellId) {
        if !self.in_bounds(x, y) {
            return;
        }
        let i = self.idx(x, y);
        self.cells[i] = id;
        self.changed.push(i as u32);
        self.report_to_chunk(x, y);
    }

    /// `cellOps.cZ` → `Qg`: перенос элемента в **пустую** клетку.
    ///
    /// Это не перестановка. Игра переставляет две занятые клетки только через
    /// обмен по плотности (`swap_elements`), и только с разрешения правил: не
    /// с падающим, не со статикой, не с элементом своего же типа. Подмена
    /// переноса перестановкой — ровно тот случай, когда мир выглядит живым,
    /// но ведёт себя не как оригинал.
    #[inline(always)]
    pub fn move_element(&mut self, slot: usize, from: (i32, i32), to: (i32, i32)) {
        let id = self.cell(from.0, from.1);
        self.elements.x[slot] = to.0 as u16;
        self.elements.y[slot] = to.1 as u16;
        self.set_cell(to.0, to.1, id);
        self.set_cell(from.0, from.1, EMPTY);
    }

    /// `cellOps.Hc` → обмен двух элементов местами.
    ///
    /// Вертикальная скорость того, кто продавливается вниз, обрезается до 60:
    /// иначе тяжёлое, набравшее ход в воздухе, проныривает сквозь жидкость
    /// целыми пачками клеток за тик.
    #[inline(always)]
    pub fn swap_elements(&mut self, slot_a: usize, slot_b: usize, a: (i32, i32), b: (i32, i32)) {
        let id_a = self.cell(a.0, a.1);
        let id_b = self.cell(b.0, b.1);

        if self.elements.velocity_y[slot_a] > 60.0 {
            self.elements.velocity_y[slot_a] = 60.0;
        }
        self.elements.x[slot_a] = b.0 as u16;
        self.elements.y[slot_a] = b.1 as u16;
        self.elements.x[slot_b] = a.0 as u16;
        self.elements.y[slot_b] = a.1 as u16;

        self.set_cell(a.0, a.1, id_b);
        self.set_cell(b.0, b.1, id_a);
    }

    /// `cellOps.U7` → удаление элемента: клетка становится пустой.
    #[inline(always)]
    pub fn remove_element(&mut self, x: i32, y: i32) {
        self.set_cell(x, y, EMPTY);
    }
}
