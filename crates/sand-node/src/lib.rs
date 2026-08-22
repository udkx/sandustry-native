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

use sand_sim::chunk::{inspect, Gate, Stats, Verdict, VerdictCache};
use sand_sim::events::{Event, EventSink};
use sand_sim::physics::{update_region, Matter, MatterTable};
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
    /// 3 — структура рядом.
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
    min_velocity_y: Float32Array,
    threshold_x: Float32Array,
    threshold_y: Float32Array,
    density: Float32Array,
    is_free_falling: Uint8Array,
    has_been_updated: Uint8Array,
    skip_physics: Uint8Array,
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
    chunks_high: i32,

    table: MatterTable,
    stats: Stats,
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
        let chunk = config.get_named_property::<i32>("chunkSize").unwrap_or(40).max(1);
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
            min_velocity_y: field!("minVelocityY", Float32Array),
            threshold_x: field!("thresholdX", Float32Array),
            threshold_y: field!("thresholdY", Float32Array),
            density: field!("density", Float32Array),
            is_free_falling: field!("isFreeFalling", Uint8Array),
            has_been_updated: field!("hasBeenUpdated", Uint8Array),
            skip_physics: field!("skipPhysics", Uint8Array),
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
            chunks_high,

            table: MatterTable::new(),
            stats: Stats::default(),
            cache: VerdictCache::new(chunks_wide as usize * chunks_high as usize, 30),
            chunks_wide,
        })
    }

    /// Заполнение таблицы материи. Зовётся из JS по `elementDefinitions`,
    /// включая элементы модов: какие материалы существуют — вопрос данных, а не
    /// кода. Всё, чего в таблице нет, ядро вернёт обратно в JS.
    #[napi]
    pub fn set_matter(&mut self, kind: u32, matter: u32, dispersion: u32) {
        // Числа те же, что в перечислении состояний игры: Solid=1, Liquid=2,
        // Gas=4, Static=5, Slushy=6, Powder=8. Частицы (3) и wisp (7) остаются
        // за JS: у них баллистика и собственные траектории.
        let m = match matter {
            1 | 8 => Matter::Powder,
            2 => Matter::Liquid,
            4 => Matter::Gas,
            5 => Matter::Static,
            6 => Matter::Slushy,
            _ => Matter::Other,
        };
        self.table.set(kind as u8, m, dispersion.min(255) as u8);
    }

    /// Обработка одного чанка.
    ///
    /// Чанк сначала осматривается: если в нём есть тип с мод-перехватчиком,
    /// незнакомый материал или рядом структура — ядро не берётся, и вызывающая
    /// сторона считает его как раньше.
    #[napi]
    pub fn update_chunk(&mut self, chunk_x: i32, chunk_y: i32, frame: i64) -> ChunkResult {
        let x0 = chunk_x * self.chunk_size;
        let y0 = chunk_y * self.chunk_size;
        let x1 = (x0 + self.chunk_size).min(self.width);
        let y1 = (y0 + self.chunk_size).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return ChunkResult { native: false, touched: 0, deferred: 0, reason: 1 };
        }

        // SAFETY: массивы — виды на память игры. Пока идёт этот вызов,
        // вызывающий воркер заблокирован, а чужие воркеры работают со своими
        // полосами чанков и в наш диапазон не пишут — это то же правило, по
        // которому игра сама делит работу между потоками.
        //
        // Виды собираются здесь, а не во вспомогательном методе: тот забирал бы
        // `self` целиком, и таблица материи со статистикой стали бы недоступны.
        let mut world = World {
            width: self.width,
            height: self.height,
            chunk_size: self.chunk_size,
            chunk_dirty_next: unsafe { self.chunk_dirty_next.as_mut() },
            chunk_width: self.chunks_wide,
            chunk_height: self.chunks_high,
            cells: unsafe { self.cells.as_mut() },
            elements: Elements {
                kind: unsafe { self.kind.as_mut() },
                velocity_x: unsafe { self.velocity_x.as_mut() },
                velocity_y: unsafe { self.velocity_y.as_mut() },
                min_velocity_y: unsafe { self.min_velocity_y.as_mut() },
                threshold_x: unsafe { self.threshold_x.as_mut() },
                threshold_y: unsafe { self.threshold_y.as_mut() },
                density: unsafe { self.density.as_mut() },
                is_free_falling: unsafe { self.is_free_falling.as_mut() },
                has_been_updated: unsafe { self.has_been_updated.as_mut() },
                skip_physics: unsafe { self.skip_physics.as_mut() },
                x: unsafe { self.pos_x.as_mut() },
                y: unsafe { self.pos_y.as_mut() },
                last_side_checked: unsafe { self.last_side_checked.as_mut() },
                moves_y_axis: unsafe { self.moves_y_axis.as_mut() },
                moves_y_axis_count: unsafe { self.moves_y_axis_count.as_mut() },
            },
        };
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
                },
            };
        }

        let mut sink = DeferSink {
            buffer: unsafe { self.needs_js.as_mut() },
            written: 0,
        };
        let touched = update_region(
            &mut world,
            &self.table,
            &mut sink,
            x0,
            y0,
            x1,
            y1,
            frame as u64,
        );
        let deferred = sink.written;

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
        ]
    }

    #[napi]
    pub fn reset_stats(&mut self) {
        self.stats = Stats::default();
    }

}

/// Приёмник событий, который складывает отложенные клетки в буфер JS.
///
/// Перемещения здесь намеренно игнорируются: принимающая сторона и так знает,
/// что чанк обработан, а список всех сдвигов на плотной сцене — это десятки
/// тысяч записей за тик, которые никто не читает.
struct DeferSink<'a> {
    buffer: &'a mut [i32],
    written: u32,
}

impl<'a> EventSink for DeferSink<'a> {
    fn push(&mut self, event: Event) {
        if let Event::NeedsJs { x, y } = event {
            let i = self.written as usize * 2;
            if i + 1 < self.buffer.len() {
                self.buffer[i] = x;
                self.buffer[i + 1] = y;
                self.written += 1;
            }
        }
    }
}
