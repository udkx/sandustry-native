//! Движение сыпучего и жидкостей — та же схема, что в `matterPhysics` игры.
//!
//! Скорость копится в `velocityY`, пройденный путь — в `thresholdY`, целая
//! часть порога даёт число клеток за тик. Такая схема позволяет частице
//! двигаться медленнее клетки за кадр, и повторить её обязательно: любое
//! «упрощение» здесь сразу меняет то, как выглядит песок.

use crate::events::{Event, EventSink};
use crate::view::{is_element, skip, World, ELEMENT_MIN};

/// Гравитация игры: `0.06 * 60 * 60` из её конфигурации.
pub const GRAVITY: f32 = 216.0;
/// Потолок вертикальной скорости для жидкостей.
pub const MAX_VELOCITY_Y: f32 = 240.0;
/// Дальше половины чанка за тик не двигаемся: на этом стоит вся схема
/// разделения работы между потоками.
pub const MAX_STEPS: i32 = 20;

/// Состояние материи. Пока — только то, что нужно первому этапу; остальное
/// уходит обратно в JS через `Event::NeedsJs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Matter {
    Powder,
    Liquid,
    /// Всё остальное: газы, статика, частицы, слизь, машины.
    Other,
}

/// Таблица «тип элемента → состояние материи». Заполняется со стороны JS из
/// `elementDefinitions`, включая элементы, добавленные модами, — поэтому она
/// данные, а не код.
pub struct MatterTable {
    states: [Matter; 256],
    dispersion: [u8; 256],
}

impl MatterTable {
    pub fn new() -> Self {
        MatterTable {
            states: [Matter::Other; 256],
            dispersion: [0; 256],
        }
    }

    pub fn set(&mut self, kind: u8, matter: Matter, dispersion: u8) {
        self.states[kind as usize] = matter;
        self.dispersion[kind as usize] = dispersion;
    }

    #[inline(always)]
    pub fn matter(&self, kind: u8) -> Matter {
        self.states[kind as usize]
    }

    #[inline(always)]
    pub fn dispersion(&self, kind: u8) -> i32 {
        self.dispersion[kind as usize] as i32
    }
}

impl Default for MatterTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Обновление одной клетки. Возвращает `true`, если клетка сдвинулась.
///
/// Функция намеренно ничего не знает про звук, экономику и моды: всё, что
/// должно случиться снаружи, уходит в очередь событий. Иначе на каждое
/// перемещение пришлось бы возвращаться в JS, и весь выигрыш съели бы переходы
/// между мирами.
pub fn update_cell(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    x: i32,
    y: i32,
    slot: usize,
) -> bool {
    let kind = world.elements.kind[slot];
    if world.elements.skip_physics[slot] >= skip::AGGRESSIVE {
        // Элемент забрала себе машина или конвейер — не трогаем.
        return false;
    }

    match table.matter(kind) {
        Matter::Powder => update_powder(world, sink, x, y, slot),
        Matter::Liquid => update_liquid(world, table, sink, x, y, slot, kind),
        Matter::Other => {
            sink.push(Event::NeedsJs { x, y });
            false
        }
    }
}

/// Разгон и путь по вертикали. Общая часть для сыпучего и жидкости.
#[inline(always)]
fn fall(world: &mut World, x: i32, y: i32, slot: usize) -> (i32, i32) {
    let mut vy = world.elements.velocity_y[slot] + GRAVITY / 60.0;
    if vy > MAX_VELOCITY_Y {
        vy = MAX_VELOCITY_Y;
    }
    let mut ty = world.elements.threshold_y[slot] + vy / 60.0;
    let steps = (ty as i32).min(MAX_STEPS);
    if steps > 0 {
        ty -= steps as f32;
    }
    world.elements.velocity_y[slot] = vy;
    world.elements.threshold_y[slot] = ty;

    let density = world.elements.density[slot];
    let mut cy = y;
    let mut travelled = 0;
    while travelled < steps && world.can_enter(x, cy + 1, density) {
        cy += 1;
        travelled += 1;
    }
    (cy, travelled)
}

/// Остановка: скорость и накопленный путь гасятся, иначе частица «помнит»
/// разгон и после удара стартует рывком.
#[inline(always)]
fn halt(world: &mut World, slot: usize) {
    world.elements.velocity_y[slot] = 0.0;
    world.elements.threshold_y[slot] = 0.0;
    world.elements.is_free_falling[slot] = 0;
}

fn update_powder(
    world: &mut World,
    sink: &mut dyn EventSink,
    x: i32,
    y: i32,
    slot: usize,
) -> bool {
    let (cy, travelled) = fall(world, x, y, slot);
    if travelled > 0 {
        world.swap_cells(x, y, x, cy);
        world.elements.is_free_falling[slot] = 1;
        sink.push(Event::Moved { from: (x, y), to: (x, cy) });
        return true;
    }

    // Осыпаться вбок можно, только упёршись. Ноль пройденных клеток ещё не
    // значит «заблокировано»: частица могла просто не набрать скорости на
    // целую клетку. Без этой проверки песчинка уезжает по диагонали в первом
    // же тике падения вместо того, чтобы падать.
    let density = world.elements.density[slot];
    if world.can_enter(x, y + 1, density) {
        return false;
    }

    // Сторона выбирается по сохранённому направлению, а не случайно: так куча
    // осыпается ровно, а результат воспроизводим.
    let dir = if world.elements.last_side_checked[slot] >= 0 { 1 } else { -1 };
    for d in [dir, -dir] {
        if world.can_enter(x + d, y + 1, density) {
            world.swap_cells(x, y, x + d, y + 1);
            world.elements.last_side_checked[slot] = d as i16;
            halt(world, slot);
            sink.push(Event::Moved { from: (x, y), to: (x + d, y + 1) });
            return true;
        }
    }

    halt(world, slot);
    false
}

fn update_liquid(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    x: i32,
    y: i32,
    slot: usize,
    kind: u8,
) -> bool {
    let (cy, travelled) = fall(world, x, y, slot);
    if travelled > 0 {
        world.swap_cells(x, y, x, cy);
        world.elements.is_free_falling[slot] = 1;
        sink.push(Event::Moved { from: (x, y), to: (x, cy) });
        return true;
    }

    // Та же оговорка, что и у сыпучего: пока падение возможно, вбок не идём.
    let density = world.elements.density[slot];
    if world.can_enter(x, y + 1, density) {
        return false;
    }
    let dir = if world.elements.last_side_checked[slot] >= 0 { 1 } else { -1 };

    // Сначала диагональ — жидкость должна стекать по склону, а не
    // размазываться по его вершине.
    for d in [dir, -dir] {
        if world.can_enter(x + d, y + 1, density) {
            world.swap_cells(x, y, x + d, y + 1);
            world.elements.last_side_checked[slot] = d as i16;
            halt(world, slot);
            sink.push(Event::Moved { from: (x, y), to: (x + d, y + 1) });
            return true;
        }
    }

    // Растекание вбок на длину, заданную материалом.
    let reach = table.dispersion(kind);
    for d in [dir, -dir] {
        let mut best = 0;
        for step in 1..=reach {
            if world.can_enter(x + d * step, y, density) {
                best = step;
                // Нашли обрыв — дальше идти незачем, сваливаемся сюда.
                if world.can_enter(x + d * step, y + 1, density) {
                    break;
                }
            } else {
                break;
            }
        }
        if best > 0 {
            world.swap_cells(x, y, x + d * best, y);
            world.elements.last_side_checked[slot] = d as i16;
            halt(world, slot);
            sink.push(Event::Moved { from: (x, y), to: (x + d * best, y) });
            return true;
        }
    }

    halt(world, slot);
    false
}

/// Сброс флагов «обработано» перед проходом.
///
/// Игра гасит их в конце тика, пробегая по списку тронутых элементов. При
/// работе со снимком такого списка нет, и без сброса каждый второй кадр
/// проходит вхолостую: флаг из снимка совпадает с чётностью кадра, и весь мир
/// считается уже обновлённым.
pub fn reset_updated(world: &mut World, frame: u64) {
    let parity = (frame & 1) as u8;
    world.elements.has_been_updated.fill(parity ^ 1);
}

/// Проход по прямоугольнику клеток.
///
/// Снизу вверх, направление по X чередуется по кадру — иначе сыпучее
/// систематически сползает в одну сторону, и это видно глазом.
pub fn update_region(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    frame: u64,
) -> usize {
    let parity = (frame & 1) as u8;
    let ltr = frame % 2 == 0;
    let mut touched = 0;

    for y in (y0..y1).rev() {
        for i in x0..x1 {
            let x = if ltr { i } else { x1 - 1 - (i - x0) };
            let id = world.cells[world.idx(x, y)];
            if !is_element(id) {
                continue;
            }
            let slot = (id - ELEMENT_MIN) as usize;
            if slot >= world.elements.kind.len() || world.elements.kind[slot] == 0 {
                continue;
            }
            if world.elements.has_been_updated[slot] == parity {
                continue;
            }
            world.elements.has_been_updated[slot] = parity;
            touched += 1;
            update_cell(world, table, sink, x, y, slot);
        }
    }
    touched
}
