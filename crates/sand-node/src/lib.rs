//! Загрузка ядра в симуляционный воркер Sandustry.
//!
//! Все массивы приходят из JS как виды над той же разделяемой памятью, с
//! которой работает игра, — копирования нет ни на входе, ни на выходе. Пока
//! идёт вызов, вызывающий воркер стоит, но остальные продолжают писать в ту же
//! память, поэтому ядро обязано трогать только свою полосу чанков. За это
//! отвечает вызывающая сторона: она передаёт координаты чанка, которым владеет.

#![deny(clippy::undocumented_unsafe_blocks)]

use napi::bindgen_prelude::*;
use napi_derive::napi;

use sand_sim::chunk::{
    inspect, should_process, should_process_in_column, window, Gate, Stats, Verdict, VerdictCache,
};
use sand_sim::events::{Event, EventSink, Why};
use sand_sim::physics::{update_region, Marks, Matter, MatterTable, Rng};
use sand_sim::view::{Elements, World};

/// Результат обработки одного чанка.
#[napi(object)]
pub struct ChunkResult {
    /// Ядро посчитало чанк целиком.
    pub native: bool,
    /// Сколько клеток обработано (0, если чанк отдан в JS).
    pub touched: u32,
    /// Сколько клеток записано в буфер `needs_js` — их обязан досчитать
    /// старый код.
    pub deferred: u32,
    /// Почему чанк не взят: 0 — взят, 1 — незнакомый материал, 2 — мод-хук,
    /// 3 — структура рядом, 4 — у элемента крутится таймер.
    pub reason: u32,
}

/// Ядро, привязанное к массивам игры.
#[napi]
pub struct NativeSim {
    width: i32,
    height: i32,
    chunk_size: i32,

    cells: Uint32Array,
    kind: Uint8Array,
    velocity_x: Float32Array,
    velocity_y: Float32Array,
    min_velocity_x: Float32Array,
    min_velocity_y: Float32Array,
    threshold_x: Float32Array,
    threshold_y: Float32Array,
    density: Float32Array,
    is_free_falling: Uint8Array,
    has_been_updated: Uint8Array,
    skip_physics: Uint8Array,
    has_duration: Uint8Array,
    duration_left: Float32Array,
    pos_x: Uint16Array,
    pos_y: Uint16Array,
    last_side_checked: Int16Array,
    moves_y_axis: Uint16Array,
    moves_y_axis_count: Uint16Array,

    mod_hooks: Uint8Array,
    block_types: Uint8Array,
    block_width: i32,
    block_scale: i32,

    /// Сюда ядро складывает координаты клеток, которые не взялось считать:
    /// пары x, y подряд. Буфер выделяет JS один раз, чтобы на каждый тик не
    /// возникало аллокации.
    needs_js: Int32Array,

    /// Флаги «чанк считать в следующем кадре» — второй буфер игры. Ядро
    /// обязано их ставить, иначе подвинутые им чанки засыпают, и движение
    /// идёт рывками: несколько кадров работает, потом встаёт до случайного
    /// пробуждения со стороны JS.
    chunk_dirty_next: Uint8Array,
    /// Флаги текущего кадра — только чтение. По ним видно, считает ли игра этот
    /// чанк вообще: клетку спящего чанка трогать нельзя.
    chunk_should_update: Uint8Array,
    /// Таблица `terrainType`: по идентификатору клетки — тип терраина. Нужна,
    /// чтобы отличить блок от скользящего блока и от конвейера.
    terrain_type: Uint8Array,
    /// Список слотов, посчитанных ядром в этом кадре. JS вливает его в свой
    /// `updatedElementIndices`, и тогда флаги гасит игра — как для своих.
    updated_buffer: Int32Array,
    /// Сюда ядро выкладывает индексы клеток, которые изменило. По ним мост
    /// прогоняет настоящий `cellOps.Gz` — он обновит растр, тень и
    /// пост-обработку. Без этого мир считается верно, а выглядит рваным.
    changed_buffer: Int32Array,
    /// Сюда ядро складывает номера чанков колонки, за которые не взялось: их
    /// досчитает старый код.
    deferred_buffer: Int32Array,
    chunks_high: i32,

    table: MatterTable,
    rng: Rng,
    marks: Marks,
    changed: Vec<u32>,
    /// Клетки, которые ядро считать не взялось: пары x, y. Их обязан досчитать
    /// старый код — иначе истёкшие таймеры и реакции просто пропадают.
    needs_js_cells: Vec<i32>,
    stats: Stats,
    /// Разбивка досчётов в JS по причинам — счётчик живёт рядом со Stats и
    /// гасится вместе с ним.
    needs_js_why: NeedsJsCounts,
    /// Вердикты по чанкам живут несколько тиков: осмотр стоит полного прохода
    /// по клеткам, и делать его каждый раз — значит добавить к работе лишний
    /// скан вместо того, чтобы что-то сэкономить.
    cache: VerdictCache,
    chunks_wide: i32,
}

#[napi]
impl NativeSim {
    /// Принимает массивы игры одним объектом: их полтора десятка, и позиционные
    /// аргументы здесь read-only превратились бы в источник ошибок.
    #[napi(constructor)]
    pub fn new(config: Object) -> Result<Self> {
        let world_w = config.get_named_property::<i32>("width").unwrap_or(0);
        let world_h = config.get_named_property::<i32>("height").unwrap_or(0);
        let chunk = config
            .get_named_property::<i32>("chunkSize")
            .unwrap_or(40)
            .max(1);
        let chunks_wide = (world_w + chunk - 1) / chunk;
        let chunks_high = (world_h + chunk - 1) / chunk;

        macro_rules! field {
            ($name:literal, $ty:ty) => {
                config.get_named_property::<$ty>($name).map_err(|e| {
                    Error::new(
                        Status::InvalidArg,
                        format!("не передано поле {}: {e}", $name),
                    )
                })?
            };
        }

        Ok(NativeSim {
            width: field!("width", i32),
            height: field!("height", i32),
            chunk_size: field!("chunkSize", i32),

            cells: field!("cells", Uint32Array),
            kind: field!("type", Uint8Array),
            velocity_x: field!("velocityX", Float32Array),
            velocity_y: field!("velocityY", Float32Array),
            min_velocity_x: field!("minVelocityX", Float32Array),
            min_velocity_y: field!("minVelocityY", Float32Array),
            threshold_x: field!("thresholdX", Float32Array),
            threshold_y: field!("thresholdY", Float32Array),
            density: field!("density", Float32Array),
            is_free_falling: field!("isFreeFalling", Uint8Array),
            has_been_updated: field!("hasBeenUpdated", Uint8Array),
            skip_physics: field!("skipPhysics", Uint8Array),
            has_duration: field!("hasDuration", Uint8Array),
            duration_left: field!("durationLeft", Float32Array),
            pos_x: field!("x", Uint16Array),
            pos_y: field!("y", Uint16Array),
            last_side_checked: field!("lastSideChecked", Int16Array),
            moves_y_axis: field!("movesYAxis", Uint16Array),
            moves_y_axis_count: field!("movesYAxisCount", Uint16Array),

            mod_hooks: field!("modHooks", Uint8Array),
            block_types: field!("blockTypes", Uint8Array),
            block_width: field!("blockWidth", i32),
            block_scale: field!("blockScale", i32),
            needs_js: field!("needsJsBuffer", Int32Array),
            chunk_dirty_next: field!("chunkShouldUpdateNext", Uint8Array),
            chunk_should_update: field!("chunkShouldUpdate", Uint8Array),
            terrain_type: field!("terrainType", Uint8Array),
            updated_buffer: field!("updatedBuffer", Int32Array),
            changed_buffer: field!("changedBuffer", Int32Array),
            deferred_buffer: field!("deferredBuffer", Int32Array),
            chunks_high,

            table: MatterTable::new(),
            rng: Rng::new(0x5eed),
            marks: Marks::new(),
            changed: Vec::with_capacity(1 << 16),
            needs_js_cells: Vec::with_capacity(1 << 12),
            stats: Stats::default(),
            needs_js_why: NeedsJsCounts::default(),
            cache: VerdictCache::new(chunks_wide as usize * chunks_high as usize, 30),
            chunks_wide,
        })
    }

    /// Заполнение таблицы материи. Зовётся из JS по `elementDefinitions`,
    /// включая элементы модов: какие материалы существуют — вопрос данных, а не
    /// кода. Всё, чего в таблице нет, ядро вернёт обратно в JS.
    #[napi]
    pub fn set_matter(&mut self, kind: u32, matter: u32, horizontal_speed: f64) {
        // Числа те же, что в перечислении состояний игры: Solid=1, Liquid=2,
        // Gas=4, Static=5, Slushy=6, Powder=8. Частицы (3) и wisp (7) остаются
        // за JS: у них баллистика и собственные траектории.
        //
        // Powder идёт в одну корзину со Static: обработчика движения у него в
        // игре нет, он просто стоит. Песок — это Solid.
        let m = match matter {
            1 => Matter::Solid,
            2 => Matter::Liquid,
            4 => Matter::Gas,
            5 | 8 => Matter::Inert,
            6 => Matter::Slushy,
            _ => Matter::Other,
        };
        self.table.set(kind as u8, m, horizontal_speed as f32);
    }

    /// Личный потолок вертикальной скорости типа (240 у BurntResidue).
    #[napi]
    pub fn set_max_velocity_y(&mut self, kind: u32, max: f64) {
        self.table.set_max_velocity_y(kind as u8, max as f32);
    }

    /// Участвует ли тип в реакциях. Столкновение реагирующих типов ядро не
    /// считает: за реакцией тянутся вторичные продукты, звук, очки
    /// производства и события модов — это работа JS.
    #[napi]
    pub fn set_reactive(&mut self, kind: u32, with_others: bool, with_self: bool) {
        self.table.set_reactive(kind as u8, with_others, with_self);
    }

    /// Пара типов, которая во что-то превращается. Список строит JS, спрашивая
    /// у самой игры (`secondaryResult.hu`), — включая смеси из модов.
    #[napi]
    pub fn set_reaction(&mut self, a: u32, b: u32) {
        self.table.set_reaction(a as u8, b as u8);
    }

    /// Обработка одного чанка.
    ///
    /// Чанк сначала осматривается: если в нём есть тип с мод-перехватчиком,
    /// незнакомый материал или рядом структура — ядро не берётся, и вызывающая
    /// сторона считает его как раньше.
    #[napi]
    pub fn update_chunk(
        &mut self,
        chunk_x: i32,
        chunk_y: i32,
        margin_x: i32,
        margin_y: i32,
        left_to_right: bool,
        dt: f64,
    ) -> ChunkResult {
        self.run_chunk(chunk_x, chunk_y, margin_x, margin_y, false, left_to_right, dt)
    }

    /// Целая колонка чанков за один вызов — `cellOps.k` целиком.
    ///
    /// Раньше мост звал ядро на каждый чанк отдельно, и это оказалось дороже
    /// самой физики: пять тысяч переходов JS→native за тик на воркер, каждый с
    /// созданием объекта результата. На пустом мире, где считать нечего,
    /// воркеры всё равно жгли по 10 мс — ровно на этих переходах. Колонка
    /// целиком превращает пять тысяч вызовов в несколько десятков.
    ///
    /// Возвращает, сколько чанков ядро не взяло; их номера — в `deferredBuffer`,
    /// и досчитать их обязан старый код.
    #[napi]
    pub fn update_column(
        &mut self,
        chunk_x: i32,
        margin_x: i32,
        left_to_right: bool,
        dt: f64,
    ) -> u32 {
        let mut deferred = 0usize;
        // Колонка обходится снизу вверх — так же, как у игры.
        for chunk_y in (0..self.chunks_high).rev() {
            let result = self.run_chunk(chunk_x, chunk_y, margin_x, 0, true, left_to_right, dt);
            if result.native {
                continue;
            }
            // SAFETY: буфер выделен JS один раз и живёт столько же, сколько
            // ядро; пока идёт вызов, вызывающий воркер стоит.
            let buffer = unsafe { self.deferred_buffer.as_mut() };
            if deferred < buffer.len() {
                buffer[deferred] = chunk_y;
                deferred += 1;
            }
        }
        deferred as u32
    }

    /// Общее тело обоих входов: окно, ранний выход, осмотр, физика.
    #[allow(clippy::too_many_arguments)]
    fn run_chunk(
        &mut self,
        chunk_x: i32,
        chunk_y: i32,
        margin_x: i32,
        margin_y: i32,
        in_column: bool,
        left_to_right: bool,
        dt: f64,
    ) -> ChunkResult {

        // SAFETY: массивы — виды на память игры. Пока идёт этот вызов,
        // вызывающий воркер заблокирован, а чужие воркеры работают со своими
        // полосами чанков и в наш диапазон не пишут — это то же правило, по
        // которому игра сама делит работу между потоками.
        //
        // Виды собираются здесь, а не во вспомогательном методе: тот забирал бы
        // `self` целиком, и таблица материи со статистикой стали бы недоступны.
        let mut world = unsafe {
            World {
                width: self.width,
                height: self.height,
                chunk_size: self.chunk_size,
                chunk_dirty_next: self.chunk_dirty_next.as_mut(),
                chunk_should_update: self.chunk_should_update.as_ref(),
                terrain_type: self.terrain_type.as_ref(),
                changed: &mut self.changed,
                chunk_width: self.chunks_wide,
                chunk_height: self.chunks_high,
                cells: self.cells.as_mut(),
                elements: Elements {
                    kind: self.kind.as_mut(),
                    velocity_x: self.velocity_x.as_mut(),
                    velocity_y: self.velocity_y.as_mut(),
                    min_velocity_x: self.min_velocity_x.as_mut(),
                    min_velocity_y: self.min_velocity_y.as_mut(),
                    threshold_x: self.threshold_x.as_mut(),
                    threshold_y: self.threshold_y.as_mut(),
                    density: self.density.as_mut(),
                    is_free_falling: self.is_free_falling.as_mut(),
                    has_been_updated: self.has_been_updated.as_mut(),
                    skip_physics: self.skip_physics.as_mut(),
                    has_duration: self.has_duration.as_mut(),
                    duration_left: self.duration_left.as_mut(),
                    x: self.pos_x.as_mut(),
                    y: self.pos_y.as_mut(),
                    last_side_checked: self.last_side_checked.as_mut(),
                    moves_y_axis: self.moves_y_axis.as_mut(),
                    moves_y_axis_count: self.moves_y_axis_count.as_mut(),
                },
            }
        };

        // Спящий чанк игра пропускает мгновенно, не касаясь его клеток. Пока
        // ядро этого не делало, оно сканировало сотни тысяч клеток впустую и
        // при этом теряло полосу на стыке с активным соседом.
        let awake = if in_column {
            should_process_in_column(&world, chunk_x, chunk_y, margin_x)
        } else {
            should_process(&world, chunk_x, chunk_y, margin_x, margin_y)
        };
        if !awake {
            self.stats.sleeping += 1;
            return ChunkResult { native: true, touched: 0, deferred: 0, reason: 0 };
        }

        let win = window(&world, chunk_x, chunk_y, margin_x, margin_y);
        if win.is_empty() {
            return ChunkResult { native: true, touched: 0, deferred: 0, reason: 0 };
        }
        let (x0, y0, x1, y1) = (win.x0, win.y0, win.x1, win.y1);

        let gate = Gate {
            mod_hooks: self.mod_hooks.as_ref(),
            block_types: self.block_types.as_ref(),
            block_width: self.block_width,
            block_scale: self.block_scale,
        };

        let slot = (chunk_y * self.chunks_wide + chunk_x).max(0) as usize;
        let verdict = match self.cache.get(slot) {
            Some(v) => {
                self.stats.cached += 1;
                v
            }
            None => {
                let v = inspect(&world, &self.table, &gate, x0, y0, x1, y1);
                self.cache.put(slot, v);
                v
            }
        };
        if let Verdict::Skip(reason) = verdict {
            self.stats.record(verdict);
            return ChunkResult {
                native: false,
                touched: 0,
                deferred: 0,
                reason: match reason {
                    sand_sim::Reason::UnknownMatter => 1,
                    sand_sim::Reason::ModHook => 2,
                    sand_sim::Reason::Structure => 3,
                    sand_sim::Reason::Duration => 4,
                },
            };
        }

        // Копим в вектор, а не пишем сразу в буфер JS: колонка обходит десятки
        // чанков за один вызов, и каждый следующий затирал бы предыдущий.
        let before = self.needs_js_cells.len();
        let mut sink = VecSink { cells: &mut self.needs_js_cells, why: &mut self.needs_js_why };
        let touched = update_region(
            &mut world,
            &self.table,
            &mut sink,
            &mut self.rng,
            &mut self.marks,
            x0,
            y0,
            x1,
            y1,
            left_to_right,
            dt as f32,
        );
        let deferred = (self.needs_js_cells.len() - before) as u32 / 2;

        self.stats.record(verdict);
        self.stats.cells_touched += touched as u32;
        self.stats.moved += sand_sim::last_moved() as u32;

        ChunkResult {
            native: true,
            touched: touched as u32,
            deferred,
            reason: 0,
        }
    }

    /// Клетки, за которые ядро не взялось, — в буфер `needsJsBuffer`, парами
    /// x, y. Возвращает число пар.
    ///
    /// Досчитать их обязан старый код: там истёкшие таймеры, реакции элементов
    /// и всё, за чем тянутся вторичные продукты, звук и экономика. Не досчитать
    /// — значит тихо потерять эти события: семя не прорастёт, огонь не погаснет,
    /// вода на лаве не станет паром.
    #[napi]
    pub fn take_needs_js(&mut self) -> u32 {
        // SAFETY: тот же вид на память игры, что и в `update_chunk`.
        let buffer = unsafe { self.needs_js.as_mut() };
        let n = self.needs_js_cells.len().min(buffer.len());
        buffer[..n].copy_from_slice(&self.needs_js_cells[..n]);
        self.needs_js_cells.clear();
        (n / 2) as u32
    }

    /// Клетки, изменённые ядром с прошлого вызова, — в буфер `changedBuffer`.
    ///
    /// Каждое число — индекс `y * width + x`. Мост обязан прогнать по ним
    /// `cellOps.Gz`: сетку ядро уже обновило, но растр, тень и пост-обработку
    /// игра рисует только через эту функцию. Пропустить шаг — значит получить
    /// верную физику, которая на экране выглядит как рваные струи и застывшая
    /// материя.
    #[napi]
    pub fn take_changed(&mut self) -> u32 {
        // SAFETY: тот же вид на память игры, что и в `update_chunk`.
        let buffer = unsafe { self.changed_buffer.as_mut() };
        let n = self.changed.len().min(buffer.len());
        for (i, &cell) in self.changed.iter().take(n).enumerate() {
            buffer[i] = cell as i32;
        }
        self.changed.clear();
        n as u32
    }

    /// Слоты, посчитанные ядром с прошлого вызова, — в буфер `updatedBuffer`.
    ///
    /// Вызывающая сторона обязана звать это после каждого тика и вливать
    /// результат в `store.world.updatedElementIndices`. Иначе флаг
    /// `hasBeenUpdated`, который ядро поставило подвинутому элементу, не
    /// погасит никто, и элемент залипнет навсегда: воркеры под завязку, а часть
    /// материи стоит.
    #[napi]
    pub fn take_updated(&mut self) -> u32 {
        // SAFETY: буфер выделен JS один раз и живёт столько же, сколько ядро;
        // пока идёт вызов, вызывающий воркер стоит.
        let buffer = unsafe { self.updated_buffer.as_mut() };
        let slots = self.marks.slots();
        let n = slots.len().min(buffer.len());
        for (i, &slot) in slots.iter().take(n).enumerate() {
            buffer[i] = slot as i32;
        }
        let written = n as u32;
        self.marks.clear();
        written
    }

    /// Сводка за сессию: сколько чанков ядро взяло и сколько вернуло, по
    /// причинам. По ней видно, окупается ли затея на живом мире.
    #[napi]
    pub fn stats(&self) -> Vec<u32> {
        vec![
            self.stats.native_chunks,
            self.stats.skipped_unknown,
            self.stats.skipped_hooks,
            self.stats.skipped_structures,
            self.stats.cells_touched,
            self.stats.cached,
            self.stats.moved,
            self.stats.skipped_duration,
            self.stats.sleeping,
            self.needs_js_why.stagnant,
            self.needs_js_why.reaction,
            self.needs_js_why.duration,
            self.needs_js_why.unknown,
        ]
    }

    #[napi]
    pub fn reset_stats(&mut self) {
        self.stats = Stats::default();
        self.needs_js_why = NeedsJsCounts::default();
    }
}

/// Приёмник событий, который копит отложенные клетки парами x, y.
///
/// Перемещения здесь намеренно игнорируются: о них принимающая сторона узнаёт
/// из списка изменённых клеток, а список всех сдвигов на плотной сцене — это
/// десятки тысяч записей за тик, которые никто не читает.
struct VecSink<'a> {
    cells: &'a mut Vec<i32>,
    why: &'a mut NeedsJsCounts,
}

impl<'a> EventSink for VecSink<'a> {
    fn push(&mut self, event: Event) {
        if let Event::NeedsJs { x, y, why } = event {
            self.cells.push(x);
            self.cells.push(y);
            match why {
                Why::Stagnant => self.why.stagnant += 1,
                Why::Reaction => self.why.reaction += 1,
                Why::Duration => self.why.duration += 1,
                Why::UnknownMatter => self.why.unknown += 1,
            }
        }
    }
}

/// Из-за чего клетки уходят на досчёт в JS.
///
/// Каждая стоит игре полутора микросекунд, и на живом мире их миллионы —
/// дороже, чем вся физика в ядре. Разбивка показывает, какую ветку чинить.
#[derive(Clone, Copy, Debug, Default)]
struct NeedsJsCounts {
    stagnant: u32,
    reaction: u32,
    duration: u32,
    unknown: u32,
}
