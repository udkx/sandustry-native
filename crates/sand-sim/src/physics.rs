//! Движение элементов — построчный перенос физики Sandustry.
//!
//! Источники (в `sandustry2/deobf/simulation-worker/`):
//!
//! | здесь | там |
//! |---|---|
//! | [`update_element_cell`] | `matterPhysics_75089.js` → `ce` (экспорт `cJ`) |
//! | `vertical` | `newPosition_8405.js` → `D` (экспорт `hX`) |
//! | `trace` | `newPosition_8405.js` → `F` |
//! | `horizontal_ballistic` | `newPosition_8405.js` → `O`/`N` (экспорт `Ti`) |
//! | `horizontal_walk` | `newPosition_8405.js` → `B`/`L` (экспорт `dy`) |
//! | `on_blocked` | `newPosition_8405.js` → `X` (экспорт `hO`) |
//! | `can_keep_moving` | `newPosition_8405.js` → `W` |
//! | `diagonal_permission` | `newPosition_8405.js` → `$` |
//! | `act_on_cell` | `matterPhysics_75089.js` → `C`/`j` + `const_96948.js` → `D` |
//! | `update_solid` | `disableHorizontalMovement_96509.js` → `y` |
//!
//! Прошлая версия этого файла была реконструкцией по смыслу: падает — значит
//! правильно. Так не вышло. Схема движения в Sandustry устроена иначе, чем
//! кажется со стороны: **сыпучее не осыпается диагональным шагом**. Оно
//! получает горизонтальную скорость при ударе (`on_blocked`), а потом тратит её
//! в баллистическом шаге вбок, попутно сползая на клетку вниз. Отсюда и форма
//! кучи, и то, как она расползается. Диагональ в трассировке — это другое: она
//! срабатывает только на пути падения.
//!
//! Все места, где игра лезет в структуры и зоны, здесь сведены к «структур в
//! наших чанках нет» — отбор в `chunk.rs` гарантирует это до вызова физики. Что
//! не сводится (реакции элементов, таймеры, экономика) — уходит в JS событием,
//! а не считается приблизительно.

use crate::events::{Event, EventSink, Why};
use crate::view::{element_index, is_element, skip, terrain, CellId, World, ELEMENT_MIN, EMPTY};

/// `howler_90823.A.gravity` = `0.06 * 60 * 60`.
pub const GRAVITY: f32 = 0.06 * 60.0 * 60.0;
/// `howler_90823.A.upflow` = `60 * -0.36`. Отрицательная — газ едет вверх.
pub const UPFLOW: f32 = 60.0 * -0.36;

/// Состояние материи. Числа — из `enums_38163.es`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Matter {
    /// `Solid` (1) — это песок, золото, семена. Падает и разъезжается.
    Solid,
    Liquid,
    Gas,
    /// `Slushy` (6) — мокрый песок и residue: то же, что Solid, но вязче.
    Slushy,
    /// `Static` (5) и `Powder` (8). В игре у них нет обработчика движения
    /// (таблица `ge` не содержит таких ключей), поэтому они не двигаются вовсе
    /// — но и брать чанк с ними ядру ничто не мешает.
    Inert,
    /// `Particle` (3), `Wisp` (7) и всё, чего мы не знаем: у них своя
    /// баллистика и свои траектории. Уходят в JS.
    Other,
}

/// Таблица «тип элемента → как он себя ведёт».
///
/// Заполняется со стороны JS по `elementDefinitions`, включая элементы модов:
/// какие материалы существуют — вопрос данных, а не кода.
pub struct MatterTable {
    states: [Matter; 256],
    /// `oe` игры — `horizontalSpeed` из определения элемента. Единица у
    /// большинства, `0.1` у лавы: столько клеток вбок за тик ему позволено.
    horizontal_speed: [f32; 256],
    /// Личный потолок вертикальной скорости, если он у типа есть (240 у
    /// BurntResidue). Ноль — потолка нет.
    max_velocity_y: [f32; 256],
    /// Тип участвует хоть в одной реакции с другим типом. Столкновение таких
    /// ядро не считает: рецепты, вторичные продукты, звук и экономика — работа
    /// JS. Клетка уходит наружу событием.
    reactive: [bool; 256],
    /// Тип реагирует сам с собой (`chances.iY` игры).
    self_reactive: [bool; 256],
    /// Точная карта пар: бит на каждую пару типов. Её строит JS, спрашивая у
    /// самой игры (`secondaryResult.hu`), реагирует ли эта пара — включая
    /// смеси, добавленные модами. Восемь килобайт против гадания по типу.
    pairs: Box<[u8; 256 * 256 / 8]>,
}

impl MatterTable {
    pub fn new() -> Self {
        MatterTable {
            states: [Matter::Other; 256],
            horizontal_speed: [1.0; 256],
            max_velocity_y: [0.0; 256],
            reactive: [false; 256],
            self_reactive: [false; 256],
            pairs: Box::new([0; 256 * 256 / 8]),
        }
    }

    pub fn set(&mut self, kind: u8, matter: Matter, horizontal_speed: f32) {
        self.states[kind as usize] = matter;
        self.horizontal_speed[kind as usize] = horizontal_speed;
    }

    pub fn set_max_velocity_y(&mut self, kind: u8, max: f32) {
        self.max_velocity_y[kind as usize] = max;
    }

    pub fn set_reactive(&mut self, kind: u8, with_others: bool, with_self: bool) {
        self.reactive[kind as usize] = with_others;
        self.self_reactive[kind as usize] = with_self;
    }

    /// Пара типов, которая во что-то превращается. Симметрична: порядок
    /// столкновения роли не играет.
    pub fn set_reaction(&mut self, a: u8, b: u8) {
        for (x, y) in [(a, b), (b, a)] {
            let i = (x as usize) * 256 + y as usize;
            self.pairs[i >> 3] |= 1 << (i & 7);
        }
    }

    /// Реагируют ли эти двое. Точная карта пар, а если её не построили —
    /// грубая пометка по типу: лучше лишний раз отдать клетку в JS, чем тихо
    /// проглотить реакцию.
    #[inline(always)]
    pub fn reacts(&self, a: u8, b: u8) -> bool {
        let i = (a as usize) * 256 + b as usize;
        if self.pairs[i >> 3] & (1 << (i & 7)) != 0 {
            return true;
        }
        self.reactive[a as usize] || self.reactive[b as usize]
    }

    #[inline(always)]
    pub fn matter(&self, kind: u8) -> Matter {
        self.states[kind as usize]
    }

    #[inline(always)]
    pub fn horizontal_speed(&self, kind: u8) -> f32 {
        self.horizontal_speed[kind as usize]
    }

    #[inline(always)]
    pub fn max_velocity_y(&self, kind: u8) -> f32 {
        self.max_velocity_y[kind as usize]
    }

    #[inline(always)]
    pub fn reactive(&self, kind: u8) -> bool {
        self.reactive[kind as usize]
    }

    #[inline(always)]
    pub fn self_reactive(&self, kind: u8) -> bool {
        self.self_reactive[kind as usize]
    }
}

impl Default for MatterTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Генератор случайных чисел.
///
/// Игра зовёт `Math.random` в трёх местах физики: длина шага жидкости, разброс
/// отскока вязкого и выбор стороны для элемента без горизонтальной скорости.
/// Совпасть с ней число в число нельзя и не нужно — важно, чтобы распределение
/// было тем же. Свой xorshift вместо системного ГСЧ взят ради воспроизводимости:
/// с одним зерном прогон повторяется, и разбор расхождения не превращается в
/// охоту за призраком.
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(((seed as u32) << 1) | 1)
    }

    #[inline(always)]
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// `Math.random()`.
    #[inline(always)]
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// `simConfig.getRandomIntBetween(a, b)` — включая оба конца.
    #[inline(always)]
    pub fn int_between(&mut self, a: i32, b: i32) -> i32 {
        if b <= a {
            return a;
        }
        a + (self.next() % ((b - a + 1) as u32)) as i32
    }
}

/// Список слотов, которые ядро посчитало в этом кадре.
///
/// Игра помечает посчитанный элемент (`hasBeenUpdated = 1`) и кладёт его в
/// `store.world.updatedElementIndices`, а в конце тика гасит флаги по этому
/// списку. Ядро обязано делать то же самое, иначе одно из двух: не пометим —
/// элемент в перекрытии областей обхода пройдёт двойной путь за тик (рваные
/// струи); пометим, не сообщив, — флаг никто не погасит, и элемент залипнет
/// навсегда. Поэтому список ведётся здесь и отдаётся наружу, чтобы JS влил его
/// в свой.
#[derive(Default)]
pub struct Marks {
    slots: Vec<u32>,
}

impl Marks {
    pub fn new() -> Self {
        Marks { slots: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }

    pub fn slots(&self) -> &[u32] {
        &self.slots
    }

    #[inline(always)]
    fn mark(&mut self, world: &mut World, slot: usize) {
        world.elements.has_been_updated[slot] = 1;
        self.slots.push(slot as u32);
    }
}

/// Направление, в котором элемент падает: вниз для всего тяжёлого, вверх для
/// газа. В игре это строка `"down"`/`"up"` в конфиге материи.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Down,
    Up,
}

impl Dir {
    #[inline(always)]
    fn step(self) -> i32 {
        match self {
            Dir::Down => 1,
            Dir::Up => -1,
        }
    }

    #[inline(always)]
    fn is_up(self) -> bool {
        matches!(self, Dir::Up)
    }
}

/// Что делать с горизонтальной скоростью при ударе (`onBlocked` конфига).
#[derive(Clone, Copy)]
enum Blocked {
    /// Ничего: жидкость при ударе только плещет звуком, газ не делает и того.
    Silent,
    /// Раздать вбок часть вертикальной скорости — так сыпучее начинает
    /// расползаться после падения. `divisor` — во сколько раз меньше взять,
    /// `spread` — случайный разброс.
    Spread { divisor: f32, spread: Option<(f32, f32)> },
}

/// Конфиг вертикального движения — объекты `x`, `M`, `D`, `W` в коде игры.
struct Fall {
    dir: Dir,
    gravity_factor: f32,
    max_velocity_y: Option<f32>,
    always_allow_diagonal: bool,
    blocked: Blocked,
}

/// Итог шага: клетка либо осталась на месте, либо переехала, либо её судьбу
/// решает JS (`consumed` игры — «дальше не наше дело»).
enum Step {
    Stay,
    Moved(i32, i32),
    Consumed,
}

/// Разрешение на диагональный сход — перечисление `I` в `newPosition`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Diag {
    Both,
    None,
    Left,
    Right,
}

/// Тип элемента в клетке, если там элемент.
#[inline(always)]
fn kind_at(world: &World, x: i32, y: i32) -> Option<u8> {
    let id = world.cell(x, y);
    if !is_element(id) {
        return None;
    }
    world.elements.kind.get(element_index(id)).copied()
}

/// `j` игры: можно ли занять клетку — пусто или там элемент легче нас.
#[inline(always)]
fn free_for(world: &World, id: CellId, density: f32) -> bool {
    if id == EMPTY {
        return true;
    }
    if !is_element(id) {
        return false;
    }
    match world.elements.density.get(element_index(id)) {
        Some(&d) => density > d,
        None => false,
    }
}

/// `$` игры: разрешён ли диагональный сход мимо занятой клетки.
///
/// Ветка фильтров опущена — фильтр это структура, а чанки со структурами ядро
/// не берёт.
#[inline(always)]
fn diagonal_permission(world: &World, id: CellId, is_empty: bool, always: bool) -> Diag {
    if always {
        return Diag::Both;
    }
    let t = world.terrain_of(id);
    if !is_empty && (terrain::is_conveyor(t) || t == terrain::BLOCK) {
        return Diag::None;
    }
    if !is_empty {
        if t == terrain::SLIDING_BLOCK_LEFT {
            return Diag::Left;
        }
        if t == terrain::SLIDING_BLOCK_RIGHT {
            return Diag::Right;
        }
    }
    Diag::Both
}

#[inline(always)]
fn diag_allows(perm: Diag, d: i32) -> bool {
    perm == Diag::Both || (d == -1 && perm == Diag::Left) || (d == 1 && perm == Diag::Right)
}

/// `W` игры: есть ли куда двинуться — прямо или по диагонали.
///
/// Ответ нужен даже когда элемент никуда не едет: если ехать будет куда, чанк
/// обязан проснуться, иначе частица копит скорость в спящем чанке и мир
/// дёргается (§2 соглашений).
fn can_keep_moving(world: &World, slot: usize, x: i32, y: i32, fall: &Fall) -> bool {
    let ny = y + fall.dir.step();
    let density = world.elements.density[slot];
    let id = world.cell(x, ny);
    if free_for(world, id, density) {
        return true;
    }
    let perm = diagonal_permission(world, id, id == EMPTY, fall.always_allow_diagonal);
    if perm == Diag::None {
        return false;
    }
    if (perm == Diag::Both || perm == Diag::Left) && free_for(world, world.cell(x - 1, ny), density)
    {
        return true;
    }
    if (perm == Diag::Both || perm == Diag::Right) && free_for(world, world.cell(x + 1, ny), density)
    {
        return true;
    }
    false
}

/// `X` игры (экспорт `hO`): удар о препятствие раздаёт вбок часть вертикальной
/// скорости. Отсюда берётся всё горизонтальное движение сыпучего.
fn on_blocked(world: &mut World, rng: &mut Rng, slot: usize, blocked: Blocked) {
    let Blocked::Spread { divisor, spread } = blocked else {
        return;
    };
    if world.elements.is_free_falling[slot] != 1 {
        return;
    }
    let mut e = world.elements.velocity_y[slot].abs() / divisor;
    if let Some((min, max)) = spread {
        e *= min + rng.unit() * (max - min);
    }
    if world.elements.velocity_x[slot] == 0.0 {
        world.elements.velocity_x[slot] = if rng.unit() >= 0.5 { 1.0 } else { -1.0 };
    }
    world.elements.velocity_x[slot] = if world.elements.velocity_x[slot] < 0.0 { -e } else { e };
}

/// Столкновение с занятой клеткой — `onActOnCell` конфигов плюс `const_96948.D`.
///
/// Здесь проходит граница ответственности ядра. Обмен по плотности мы считаем
/// сами: он целиком описывается тем, что лежит в разделяемой памяти. Реакцию
/// (вода на лаве, огонь на лозе, рецепт смесителя) — не считаем никогда: за ней
/// тянутся вторичные продукты, звук, очки производства и события модов. Такая
/// клетка уходит в JS, и досчитывает её старый код.
fn act_on_cell(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    slot: usize,
    kind: u8,
    from: (i32, i32),
    target: (i32, i32),
) -> bool {
    let id = world.cell(target.0, target.1);
    if !is_element(id) {
        // Терраин: единственное взаимодействие с ним (`snapGridCellSize.v1`)
        // случается на VelocitySoaker, а это структура — в наших чанках её нет.
        return false;
    }
    let other = element_index(id);
    if other >= world.elements.kind.len() {
        return false;
    }

    // `me`/`oy` игры: элемент на месте? Рассинхрон сетки и полей означает
    // призрака — игра стирает такую клетку и идёт дальше.
    if world.elements.kind[other] == 0
        || world.elements.x[other] as i32 != target.0
        || world.elements.y[other] as i32 != target.1
    {
        // Призрака стираем через ту же точку записи: растр обязан узнать и об
        // этом, иначе на экране останется висеть его цвет.
        world.set_cell(target.0, target.1, EMPTY);
        return false;
    }
    if world.elements.skip_physics[other] >= skip::AGGRESSIVE {
        return false;
    }

    let other_kind = world.elements.kind[other];
    if kind == other_kind {
        // Одинаковые типы друг с другом не взаимодействуют — кроме тех, у кого
        // есть рецепт сам с собой.
        if table.self_reactive(kind) {
            sink.push(Event::NeedsJs { x: from.0, y: from.1, why: Why::Reaction });
            return true;
        }
        return false;
    }

    if table.reacts(kind, other_kind) {
        sink.push(Event::NeedsJs { x: from.0, y: from.1, why: Why::Reaction });
        return true;
    }

    // Обмен по плотности, `const_96948.D`.
    if table.matter(other_kind) == Matter::Inert {
        return false;
    }
    if world.elements.is_free_falling[other] == 1 {
        // Падающего не вытесняют: он занят своим движением и в этом кадре ещё
        // не там, где кажется.
        return false;
    }
    if world.elements.density[slot] <= world.elements.density[other] {
        return false;
    }

    world.swap_elements(slot, other, from, target);
    true
}

/// `F` игры: трассировка пути по вертикали на `delta` клеток.
///
/// Идём по одной клетке. Свободна — шагнули. Занята — сперва пробуем
/// провзаимодействовать, потом сойти по диагонали (в ту сторону, куда смотрит
/// горизонтальная скорость), и только если и это не вышло — считаем себя
/// упёршимися.
#[allow(clippy::too_many_arguments)]
fn trace(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    slot: usize,
    kind: u8,
    x0: i32,
    y0: i32,
    delta: i32,
    fall: &Fall,
) -> (Step, bool) {
    let step = fall.dir.step();
    let count = delta.abs();
    let (mut px, mut py) = (x0, y0);
    let mut blocked = false;

    for _ in 0..count {
        let ny = py + step;
        let id = world.cell(px, ny);
        if id == EMPTY {
            py = ny;
            continue;
        }

        if act_on_cell(world, table, sink, slot, kind, (px, py), (px, ny)) {
            return (Step::Consumed, false);
        }

        // Удар гасит десятую часть вертикальной скорости — даже если сход по
        // диагонали сейчас получится.
        world.elements.velocity_y[slot] *= 0.9;

        let perm = diagonal_permission(world, id, false, fall.always_allow_diagonal);
        let mut slid = false;
        let k = if world.elements.velocity_x[slot] < 0.0 { -1 } else { 1 };
        for c in 0..2 {
            let d = if c == 0 { k } else { -k };
            if !diag_allows(perm, d) {
                continue;
            }
            let nx = px + d;
            if world.cell(nx, ny) == EMPTY {
                px = nx;
                py = ny;
                slid = true;
                break;
            }
            if act_on_cell(world, table, sink, slot, kind, (px, py), (nx, ny)) {
                return (Step::Consumed, false);
            }
        }
        if slid {
            // Диагональный сход завершает трассировку: в оригинале цикл шагов
            // прерывается, а не продолжается с новой позиции.
            break;
        }
        blocked = true;
        break;
    }

    let moved = px != x0 || py != y0;
    let outcome = if moved { Step::Moved(px, py) } else { Step::Stay };
    (outcome, blocked)
}

/// `D` игры (экспорт `hX`): разгон и вертикальный шаг.
#[allow(clippy::too_many_arguments)]
fn vertical(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    slot: usize,
    kind: u8,
    x: i32,
    y: i32,
    dt: f32,
    fall: &Fall,
) -> Step {
    let base = if fall.dir.is_up() { UPFLOW } else { GRAVITY };
    let accel = base * fall.gravity_factor;
    world.elements.velocity_y[slot] += accel * dt;

    if let Some(max) = fall.max_velocity_y {
        if fall.dir.is_up() {
            if world.elements.velocity_y[slot] < -max {
                world.elements.velocity_y[slot] = -max;
            }
        } else if world.elements.velocity_y[slot] > max {
            world.elements.velocity_y[slot] = max;
        }
    }

    world.elements.threshold_y[slot] += world.elements.velocity_y[slot] * dt;
    let ty = world.elements.threshold_y[slot];
    let up = fall.dir.is_up();

    // Порог не набран: за кадр не наберётся и клетки. Двигаться рано, но чанк
    // обязан остаться активным, пока элементу есть куда падать.
    if !(if up { ty <= -1.0 } else { ty >= 1.0 }) {
        if world.elements.is_free_falling[slot] == 1 || can_keep_moving(world, slot, x, y, fall) {
            if world.elements.is_free_falling[slot] == 0 {
                world.elements.is_free_falling[slot] = 1;
            }
            world.report_to_chunk(x, y);
        }
        return Step::Stay;
    }

    let delta = if up { ty.ceil() } else { ty.floor() } as i32;
    world.elements.threshold_y[slot] = ty % 1.0;

    let (outcome, blocked) = trace(world, table, sink, slot, kind, x, y, delta, fall);
    if matches!(outcome, Step::Consumed) {
        return Step::Consumed;
    }

    if blocked {
        on_blocked(world, rng, slot, fall.blocked);
        let min = world.elements.min_velocity_y[slot];
        if up {
            if world.elements.velocity_y[slot] > min {
                world.elements.velocity_y[slot] = min;
            }
            if world.elements.velocity_y[slot] < -60.0 {
                world.report_to_chunk(x, y);
            }
        } else {
            if world.elements.velocity_y[slot] < min {
                world.elements.velocity_y[slot] = min;
            }
            if world.elements.velocity_y[slot] > 60.0 {
                world.report_to_chunk(x, y);
            }
        }
        world.elements.is_free_falling[slot] = 0;
    } else if world.elements.is_free_falling[slot] == 0 {
        world.elements.is_free_falling[slot] = 1;
    }

    outcome
}

/// `N` игры (экспорт `Ti`): баллистический шаг вбок для сыпучего и вязкого.
///
/// Работает только когда элемент **не** в свободном падении: сначала приземлись,
/// потом расползайся. Скорость тратится с затуханием, а на каждом шаге элемент
/// пробует спуститься на клетку — так куча и оседает.
fn horizontal_ballistic(
    world: &mut World,
    slot: usize,
    x: i32,
    y: i32,
    dt: f32,
    damping: f32,
) -> Step {
    if world.elements.is_free_falling[slot] != 0 {
        return Step::Stay;
    }

    world.elements.threshold_x[slot] += world.elements.velocity_x[slot] * dt;
    world.elements.velocity_x[slot] *= damping.powf(60.0 * dt);

    let vx = world.elements.velocity_x[slot];
    if vx < 6.0 && vx > -6.0 {
        // Ниже шести клеток в секунду движение считается законченным — иначе
        // песчинки вечно дрожали бы на месте.
        world.elements.velocity_x[slot] = 0.0;
        world.elements.threshold_x[slot] = 0.0;
        return Step::Stay;
    }

    let tx = world.elements.threshold_x[slot];
    if tx < 1.0 && tx > -1.0 {
        world.report_to_chunk(x, y);
        return Step::Stay;
    }

    let steps = if tx < 0.0 { tx.ceil() } else { tx.floor() } as i32;
    world.elements.threshold_x[slot] = tx % 1.0;

    let dir = if steps < 0 { -1 } else { 1 };
    let target = x + steps;
    let mut cx = x;
    let mut cy = y;
    let mut last: Option<(i32, i32)> = None;

    while cx != target {
        cx += dir;
        if !world.is_empty(cx, cy) {
            break;
        }
        // Ступенька вниз по дороге: именно она превращает горизонтальный разлёт
        // в осыпающийся склон.
        if world.is_empty(cx, cy + 1) {
            cy += 1;
        }
        last = Some((cx, cy));
    }

    match last {
        Some((lx, ly)) => Step::Moved(lx, ly),
        None => Step::Stay,
    }
}

/// Как жидкость и газ ходят вбок (`P` и `V` в конфигах игры).
struct Walk {
    /// Жидкость вытесняет то, что легче, прямо в горизонтальном шаге.
    swap_by_density: bool,
    /// Газ так проныривает сквозь жидкость.
    swap_matter: Option<Matter>,
}

/// `L` игры (экспорт `dy`): шаг вбок на одну-две клетки для жидкости и газа.
///
/// Упёрлись — разворачиваем горизонтальную скорость. Это и есть то плескание,
/// от которого вода в итоге выравнивается по уровню.
#[allow(clippy::too_many_arguments)]
fn horizontal_walk(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    slot: usize,
    kind: u8,
    x: i32,
    y: i32,
    walk: &Walk,
) -> Step {
    if world.elements.is_free_falling[slot] != 0 {
        return Step::Stay;
    }

    let reach = rng.int_between(1, 2);
    let signed = if world.elements.velocity_x[slot] < 0.0 { -reach } else { reach };
    let target = x + signed;
    let dir = if signed < 0 { -1 } else { 1 };

    let mut cx = x;
    let mut last: Option<i32> = None;

    while cx != target {
        cx += dir;
        if !world.is_empty(cx, y) {
            let id = world.cell(cx, y);
            let can_swap = walk.swap_by_density
                || match (walk.swap_matter, kind_at(world, cx, y)) {
                    (Some(m), Some(k)) => table.matter(k) == m,
                    _ => false,
                };
            if can_swap && is_element(id) {
                if act_on_cell(world, table, sink, slot, kind, (x, y), (cx, y)) {
                    bump_moves(world, slot, y, 10);
                    return Step::Consumed;
                }
                if walk.swap_by_density {
                    let other = element_index(id);
                    if other < world.elements.density.len()
                        && world.elements.density[slot] > world.elements.density[other]
                    {
                        world.report_to_chunk(x, y);
                    }
                }
            }
            break;
        }
        last = Some(cx);
    }

    if let Some(lx) = last {
        bump_moves(world, slot, y, 1);
        return Step::Moved(lx, y);
    }

    // Никуда не пошли: развернуть скорость и решить, будить ли чанк.
    let back = if world.elements.velocity_x[slot] < 0.0 { 1 } else { -1 };
    let density = world.elements.density[slot];
    if walk.swap_by_density && free_for(world, world.cell(x + back, y), density) {
        world.report_to_chunk(x, y);
    } else {
        let side = world.elements.last_side_checked[slot];
        if !(side != 0 && x == side as i32) {
            world.report_to_chunk(x, y);
        }
    }
    world.elements.last_side_checked[slot] = x as i16;

    let vx = world.elements.velocity_x[slot];
    world.elements.velocity_x[slot] = if vx == 0.0 { -1.0 } else { -vx };
    Step::Stay
}

/// Счётчик топтания на одной высоте: по нему игра испаряет воду и пар, которым
/// некуда деваться.
#[inline(always)]
fn bump_moves(world: &mut World, slot: usize, y: i32, by: u16) {
    if world.elements.moves_y_axis[slot] as i32 != y {
        world.elements.moves_y_axis[slot] = y as u16;
        world.elements.moves_y_axis_count[slot] = 0;
    }
    world.elements.moves_y_axis_count[slot] =
        world.elements.moves_y_axis_count[slot].saturating_add(by);
}

/// Сколько ходов на одном уровне игра терпит, прежде чем испарить элемент.
const STAGNANT_LIMIT: u16 = 1000;

const SOLID_FALL: Fall = Fall {
    dir: Dir::Down,
    gravity_factor: 1.0,
    max_velocity_y: None,
    always_allow_diagonal: false,
    blocked: Blocked::Spread { divisor: 10.0, spread: None },
};

const LIQUID_FALL: Fall = Fall {
    dir: Dir::Down,
    gravity_factor: 1.0,
    max_velocity_y: Some(240.0),
    // Жидкость сходит по диагонали всегда — иначе она не растекалась бы по
    // склону, а стояла бы на его вершине столбом.
    always_allow_diagonal: true,
    blocked: Blocked::Silent,
};

const SLUSHY_FALL: Fall = Fall {
    dir: Dir::Down,
    gravity_factor: 1.0,
    max_velocity_y: None,
    always_allow_diagonal: false,
    blocked: Blocked::Spread { divisor: 5.0, spread: Some((0.8, 1.2)) },
};

const GAS_FALL: Fall = Fall {
    dir: Dir::Up,
    gravity_factor: 1.0,
    max_velocity_y: None,
    always_allow_diagonal: true,
    blocked: Blocked::Silent,
};

const LIQUID_WALK: Walk = Walk { swap_by_density: true, swap_matter: None };
const GAS_WALK: Walk = Walk { swap_by_density: false, swap_matter: Some(Matter::Liquid) };

/// `ce` игры (экспорт `cJ`) — обработка одной клетки с элементом.
///
/// Возвращает `true`, если элемент переехал.
#[allow(clippy::too_many_arguments)]
pub fn update_element_cell(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    marks: &mut Marks,
    slot: usize,
    x: i32,
    y: i32,
    dt: f32,
) -> bool {
    let kind = world.elements.kind[slot];
    if world.elements.skip_physics[slot] >= skip::AGGRESSIVE {
        // Элемент забрала машина или конвейер.
        return false;
    }
    // Тик таймера — `ue` игры. Пока он не истёк, элемент живёт обычной физикой,
    // и чанк надо держать активным. Истёк — дальше не наше дело: там
    // превращение семени, гаснущий огонь, очки производства и события модов.
    //
    // Раньше здесь стоял отказ на всякий элемент с таймером, и это отдавало в
    // JS больше чанков, чем структуры: в обжитом мире таких элементов треть.
    if world.elements.has_duration[slot] == 1 {
        world.elements.duration_left[slot] -= dt;
        if world.elements.duration_left[slot] > 0.0 {
            world.report_to_chunk(x, y);
        } else {
            sink.push(Event::NeedsJs { x, y, why: Why::Duration });
            return false;
        }
    }

    let matter = table.matter(kind);
    match matter {
        // У Static и Powder в игре нет обработчика движения — они просто стоят.
        Matter::Inert => return false,
        Matter::Other => {
            sink.push(Event::NeedsJs { x, y, why: Why::UnknownMatter });
            return false;
        }
        _ => {}
    }
    if world.elements.skip_physics[slot] >= skip::SKIP {
        return false;
    }

    // Плотно упакованный элемент. Если вокруг такие же (снизу, снизу по бокам и
    // по бокам), двигаться некуда, и игра не тратит на него ход: только копит
    // скорость с затуханием. На плотной сцене это большая часть всей работы — и
    // главная причина, по которой оригинал вообще успевает в кадр.
    let below = if matter == Matter::Gas { -1 } else { 1 };
    if kind_at(world, x, y + below) == Some(kind)
        && kind_at(world, x - 1, y + below) == Some(kind)
        && kind_at(world, x + 1, y + below) == Some(kind)
        && kind_at(world, x - 1, y) == Some(kind)
        && kind_at(world, x + 1, y) == Some(kind)
    {
        if world.elements.has_been_updated[slot] == 1 {
            return false;
        }
        if !world.cell_chunk_active(x, y) {
            return false;
        }
        world.elements.is_free_falling[slot] = 0;
        let accel = if matter == Matter::Gas { UPFLOW } else { GRAVITY };
        world.elements.velocity_y[slot] += accel * dt;
        world.elements.threshold_y[slot] += world.elements.velocity_y[slot] * dt;
        let ty = world.elements.threshold_y[slot];
        if ty >= 1.0 || ty <= -1.0 {
            world.elements.threshold_y[slot] = ty % 1.0;
            world.elements.velocity_y[slot] *= 0.9;
            let min = world.elements.min_velocity_y[slot];
            let past = if accel > 0.0 {
                world.elements.velocity_y[slot] < min
            } else {
                world.elements.velocity_y[slot] > min
            };
            if past {
                world.elements.velocity_y[slot] = min;
            }
        }
        return false;
    }

    match matter {
        Matter::Solid => update_solid(world, table, sink, rng, marks, slot, kind, x, y, dt),
        Matter::Slushy => update_slushy(world, table, sink, rng, marks, slot, kind, x, y, dt),
        Matter::Liquid => update_liquid(world, table, sink, rng, marks, slot, kind, x, y, dt),
        Matter::Gas => update_gas(world, table, sink, rng, marks, slot, kind, x, y, dt),
        Matter::Inert | Matter::Other => false,
    }
}

/// Общий хвост всех материй: если элемент сдвинулся — перенести его и пометить
/// посчитанным (`cellOps.cZ`).
fn commit(world: &mut World, marks: &mut Marks, slot: usize, from: (i32, i32), to: (i32, i32)) -> bool {
    if from == to {
        return false;
    }
    world.move_element(slot, from, to);
    marks.mark(world, slot);
    true
}

/// Общее начало: элемент уже посчитан в этом кадре или его чанк спит.
#[inline(always)]
fn can_process(world: &World, slot: usize, x: i32, y: i32) -> bool {
    world.elements.has_been_updated[slot] != 1 && world.cell_chunk_active(x, y)
}

/// `disableHorizontalMovement_96509.js` → `y`: песок, золото, семена.
#[allow(clippy::too_many_arguments)]
fn update_solid(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    marks: &mut Marks,
    slot: usize,
    kind: u8,
    x: i32,
    y: i32,
    dt: f32,
) -> bool {
    if !can_process(world, slot, x, y) {
        return false;
    }
    // Лаунчеры (`getTypeFromIndex.v`) требуют структуру под элементом — в наших
    // чанках их нет.

    let mut fall = SOLID_FALL;
    let personal = table.max_velocity_y(kind);
    if personal > 0.0 {
        fall.max_velocity_y = Some(personal);
    }

    let (mut px, mut py) = (x, y);
    match vertical(world, table, sink, rng, slot, kind, x, y, dt, &fall) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    match horizontal_ballistic(world, slot, px, py, dt, 0.9) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    commit(world, marks, slot, (x, y), (px, py))
}

/// Slushy — мокрый песок и residue: то же, что Solid, но отскок слабее, а
/// затухание горизонтальной скорости медленнее.
#[allow(clippy::too_many_arguments)]
fn update_slushy(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    marks: &mut Marks,
    slot: usize,
    kind: u8,
    x: i32,
    y: i32,
    dt: f32,
) -> bool {
    if !can_process(world, slot, x, y) {
        return false;
    }

    let (mut px, mut py) = (x, y);
    match vertical(world, table, sink, rng, slot, kind, x, y, dt, &SLUSHY_FALL) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    match horizontal_ballistic(world, slot, px, py, dt, 0.95) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    commit(world, marks, slot, (x, y), (px, py))
}

#[allow(clippy::too_many_arguments)]
fn update_liquid(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    marks: &mut Marks,
    slot: usize,
    kind: u8,
    x: i32,
    y: i32,
    dt: f32,
) -> bool {
    if !can_process(world, slot, x, y) {
        return false;
    }

    // Застоявшаяся жидкость испаряется, а испарение — это уже экономика:
    // счётчик резервуара, удаление элемента, событие. Отдаём в JS.
    if world.elements.moves_y_axis_count[slot] > STAGNANT_LIMIT {
        sink.push(Event::NeedsJs { x, y, why: Why::Stagnant });
        return false;
    }

    let (mut px, mut py) = (x, y);
    match vertical(world, table, sink, rng, slot, kind, x, y, dt, &LIQUID_FALL) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    // Медленные жидкости (лава) ходят вбок не каждый кадр, а по накоплению:
    // `horizontalSpeed` из определения элемента.
    let speed = table.horizontal_speed(kind);
    let mut may_walk = true;
    if speed < 1.0 && world.elements.is_free_falling[slot] == 0 {
        world.elements.threshold_x[slot] += speed;
        if world.elements.threshold_x[slot] < 1.0 {
            may_walk = false;
            world.report_to_chunk(px, py);
        } else {
            world.elements.threshold_x[slot] -= 1.0;
        }
    }

    if may_walk {
        match horizontal_walk(world, table, sink, rng, slot, kind, px, py, &LIQUID_WALK) {
            Step::Consumed => return false,
            Step::Moved(nx, ny) => {
                px = nx;
                py = ny;
            }
            Step::Stay => {}
        }
    }

    commit(world, marks, slot, (x, y), (px, py))
}

#[allow(clippy::too_many_arguments)]
fn update_gas(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    marks: &mut Marks,
    slot: usize,
    kind: u8,
    x: i32,
    y: i32,
    dt: f32,
) -> bool {
    if !can_process(world, slot, x, y) {
        return false;
    }

    if world.elements.moves_y_axis_count[slot] > STAGNANT_LIMIT {
        sink.push(Event::NeedsJs { x, y, why: Why::Stagnant });
        return false;
    }

    let (mut px, mut py) = (x, y);
    match vertical(world, table, sink, rng, slot, kind, x, y, dt, &GAS_FALL) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    match horizontal_walk(world, table, sink, rng, slot, kind, px, py, &GAS_WALK) {
        Step::Consumed => return false,
        Step::Moved(nx, ny) => {
            px = nx;
            py = ny;
        }
        Step::Stay => {}
    }

    commit(world, marks, slot, (x, y), (px, py))
}

/// Проход по прямоугольнику клеток — тело циклов `E`/`k` из `cellOps`.
///
/// Снизу вверх, по X — в сторону, которую задаёт `matrixTraverseDirection`
/// игры: она переворачивается каждый тик, иначе сыпучее систематически сползает
/// в одну сторону, и это видно глазом.
#[allow(clippy::too_many_arguments)]
pub fn update_region(
    world: &mut World,
    table: &MatterTable,
    sink: &mut dyn EventSink,
    rng: &mut Rng,
    marks: &mut Marks,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    left_to_right: bool,
    dt: f32,
) -> usize {
    let mut touched = 0;
    let mut moved = 0usize;

    for y in (y0..y1).rev() {
        let row = (y * world.width) as usize;
        let (from, to, step) = if left_to_right { (x0, x1, 1) } else { (x1 - 1, x0 - 1, -1) };
        let mut x = from;
        while x != to {
            let id = world.cells[row + x as usize];
            if is_element(id) {
                let slot = (id - ELEMENT_MIN) as usize;
                if slot < world.elements.kind.len() && world.elements.kind[slot] != 0 {
                    touched += 1;
                    if update_element_cell(world, table, sink, rng, marks, slot, x, y, dt) {
                        moved += 1;
                    }
                }
            }
            x += step;
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

/// Сброс флагов «обработано» — для прогонов на снимке, где списка игры нет.
///
/// В живой игре так делать нельзя: флаги гасит она сама, пробегая по
/// `updatedElementIndices`, куда попадает и наш список из [`Marks`].
pub fn reset_updated(world: &mut World) {
    world.elements.has_been_updated.fill(0);
}
