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
    /// Сыпучее: падает, при упоре осыпается вбок. В игре это Solid и Powder.
    Powder,
    Liquid,
    /// Газ: всплывает сквозь всё, что плотнее.
    Gas,
    /// Вязкое: падает, но осыпается неохотно — мокрый песок, residue.
    Slushy,
    /// Не двигается вовсе. Обрабатывать нечего, но и мешать оно не мешает,
    /// поэтому чанк с ним брать можно.
    Static,
    /// Всё остальное: частицы с баллистикой, wisp, незнакомые типы модов.
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
        Matter::Powder => update_powder(world, sink, x, y, slot, 0),
        // Вязкое осыпается через раз: этого хватает, чтобы мокрый песок
        // держал склон круче сухого, как и в оригинале.
        Matter::Slushy => update_powder(world, sink, x, y, slot, 1),
        Matter::Liquid => update_liquid(world, table, sink, x, y, slot, kind),
        Matter::Gas => update_gas(world, table, sink, x, y, slot, kind),
        // Статике движение не положено — ни в оригинале, ни здесь.
        Matter::Static => false,
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
    stickiness: u16,
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

    // Вязкое пропускает часть попыток осыпаться — отсюда более крутой склон.
    if stickiness > 0 {
        let counter = world.elements.moves_y_axis_count[slot].wrapping_add(1);
        world.elements.moves_y_axis_count[slot] = counter;
        if counter & 1 == 0 {
            return false;
        }
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

    // Та же оговорка, что и у сыпучего: пока падение возможно, вбок не идём,
    // но чанк держим активным — частица разгоняется.
    let density = world.elements.density[slot];
    if world.can_enter(x, y + 1, density) {
        world.wake(x, y);
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

/// Газ: всплывает сквозь всё, что плотнее, и расходится вбок.
///
/// Подъём — это та же плавучесть, что и у тонущего песка, только знак другой:
/// лёгкое вытесняет тяжёлое, а не наоборот.
fn update_gas(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    x: i32,
    y: i32,
    slot: usize,
    kind: u8,
) -> bool {
    let density = world.elements.density[slot];

    // Вверх — если то, что там, тяжелее.
    if world.can_rise(x, y - 1, density) {
        world.swap_cells(x, y, x, y - 1);
        sink.push(Event::Moved { from: (x, y), to: (x, y - 1) });
        return true;
    }

    let dir = if world.elements.last_side_checked[slot] >= 0 { 1 } else { -1 };
    for d in [dir, -dir] {
        if world.can_rise(x + d, y - 1, density) {
            world.swap_cells(x, y, x + d, y - 1);
            world.elements.last_side_checked[slot] = d as i16;
            sink.push(Event::Moved { from: (x, y), to: (x + d, y - 1) });
            return true;
        }
    }

    // Подниматься некуда — расходимся вбок, заполняя объём.
    let reach = table.dispersion(kind).min(4);
    for d in [dir, -dir] {
        for step in 1..=reach {
            if world.can_rise(x + d * step, y, density) {
                world.swap_cells(x, y, x + d * step, y);
                world.elements.last_side_checked[slot] = d as i16;
                sink.push(Event::Moved { from: (x, y), to: (x + d * step, y) });
                return true;
            }
        }
    }
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
    let ltr = frame % 2 == 0;
    let mut touched = 0;
    let mut moved = 0usize;

    // Защита от повторной обработки — своя, по позициям внутри участка.
    //
    // Игровой флаг `hasBeenUpdated` для этого не годится: игра гасит его в
    // конце тика, пробегая по списку тронутых элементов, а наш список ей
    // неизвестен. Клетка, помеченная нами, осталась бы «уже обработанной»
    // навсегда и перестала двигаться вообще. Битовая карта на участок стоит
    // двести байт на чанк и живёт ровно один проход.
    let w = (x1 - x0) as usize;
    let h = (y1 - y0) as usize;
    let mut done = vec![0u64; (w * h + 63) / 64];

    for y in (y0..y1).rev() {
        for i in x0..x1 {
            let x = if ltr { i } else { x1 - 1 - (i - x0) };

            let local = (y - y0) as usize * w + (x - x0) as usize;
            if done[local >> 6] & (1u64 << (local & 63)) != 0 {
                continue;
            }

            let id = world.cells[world.idx(x, y)];
            if !is_element(id) {
                continue;
            }
            let slot = (id - ELEMENT_MIN) as usize;
            if slot >= world.elements.kind.len() || world.elements.kind[slot] == 0 {
                continue;
            }

            // Элемент, уже посчитанный игрой в этом кадре, трогать нельзя:
            // области обхода перекрываются, и второй раз за тик он пройдёт
            // двойной путь. На экране это выглядит как рваные струи — часть
            // частиц улетает вперёд, часть стоит.
            //
            // Флаг только читаем. Ставить его нельзя: гасит его игра, пробегая
            // по своему списку тронутых элементов, а нас в этом списке нет —
            // помеченное нами залипло бы навсегда.
            if world.elements.has_been_updated[slot] == 1 {
                continue;
            }

            touched += 1;
            let moved_to = update_cell_at(world, table, sink, x, y, slot);
            if moved_to.is_some() {
                moved += 1;
            }

            // Помечаем клетку, куда частица уехала: если она внутри участка и
            // обход ещё до неё дойдёт, второй раз за тик её трогать нельзя.
            if let Some((nx, ny)) = moved_to {
                if nx >= x0 && nx < x1 && ny >= y0 && ny < y1 {
                    let l = (ny - y0) as usize * w + (nx - x0) as usize;
                    done[l >> 6] |= 1u64 << (l & 63);
                }
            }
        }
    }
    MOVED.with(|m| m.set(moved));
    touched
}

thread_local! {
    /// Сколько клеток сдвинулось за последний проход — для диагностики.
    static MOVED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Сколько клеток сдвинул последний вызов `update_region`.
pub fn last_moved() -> usize {
    MOVED.with(|m| m.get())
}

/// Обновление клетки с возвратом новой позиции, если она сдвинулась.
fn update_cell_at(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    x: i32,
    y: i32,
    slot: usize,
) -> Option<(i32, i32)> {
    let before = (world.elements.x[slot], world.elements.y[slot]);
    if !update_cell(world, table, sink, x, y, slot) {
        return None;
    }
    let after = (world.elements.x[slot], world.elements.y[slot]);
    if after != before {
        Some((after.0 as i32, after.1 as i32))
    } else {
        None
    }
}
