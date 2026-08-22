//! Базовые проверки поведения. Не «код написан», а «песок падает, вода течёт».
//!
//! После переноса физики из игры к ним добавились проверки соглашений: чужой
//! флаг не трогаем, спящий чанк не считаем, реакции отдаём в JS. Каждое из этих
//! правил однажды стоило цикла отладки в живом мире — тест дешевле.

use crate::events::{Event, EventQueue};
use crate::physics::{update_element_cell, update_region, Marks, Matter, MatterTable, Rng};
use crate::view::{Elements, World, ELEMENT_MIN};

/// Типы элементов в тестах — те же роли, что в игре.
const SAND: u8 = 1;
const WATER: u8 = 2;
const STEAM: u8 = 3;
const WET_SAND: u8 = 4;
/// Тип с реакцией: столкновение с ним ядро обязано отдать в JS.
const LAVA: u8 = 5;

/// Маленький мир для тестов: сетка плюс поля на несколько элементов.
struct Bench {
    w: i32,
    h: i32,
    cells: Vec<u32>,
    kind: Vec<u8>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    minvx: Vec<f32>,
    minvy: Vec<f32>,
    tx: Vec<f32>,
    ty: Vec<f32>,
    density: Vec<f32>,
    falling: Vec<u8>,
    updated: Vec<u8>,
    skip: Vec<u8>,
    duration: Vec<u8>,
    duration_left: Vec<f32>,
    x: Vec<u16>,
    y: Vec<u16>,
    side: Vec<i16>,
    my: Vec<u16>,
    myc: Vec<u16>,
    chunk_now: Vec<u8>,
    chunk_next: Vec<u8>,
    terrain: Vec<u8>,
    changed: Vec<u32>,
    next: u32,
}

impl Bench {
    fn new(w: i32, h: i32) -> Self {
        let cap = 512;
        let chunks = (((w + 39) / 40) * ((h + 39) / 40)) as usize;
        Bench {
            w,
            h,
            cells: vec![0; (w * h) as usize],
            kind: vec![0; cap],
            vx: vec![0.0; cap],
            vy: vec![0.0; cap],
            minvx: vec![0.0; cap],
            minvy: vec![0.0; cap],
            tx: vec![0.0; cap],
            ty: vec![0.0; cap],
            density: vec![0.0; cap],
            falling: vec![0; cap],
            updated: vec![0; cap],
            skip: vec![0; cap],
            duration: vec![0; cap],
            duration_left: vec![0.0; cap],
            x: vec![0; cap],
            y: vec![0; cap],
            side: vec![0; cap],
            my: vec![0; cap],
            myc: vec![0; cap],
            // Все чанки активны: спящий чанк — предмет отдельного теста.
            chunk_now: vec![1; chunks.max(1)],
            chunk_next: vec![0; chunks.max(1)],
            terrain: (0..=1000u32).map(|i| i as u8).collect(),
            changed: Vec::new(),
            next: 0,
        }
    }

    fn put(&mut self, x: i32, y: i32, kind: u8, density: f32) -> usize {
        let slot = self.next as usize;
        self.next += 1;
        self.kind[slot] = kind;
        self.density[slot] = density;
        self.x[slot] = x as u16;
        self.y[slot] = y as u16;
        self.cells[(y * self.w + x) as usize] = ELEMENT_MIN + slot as u32;
        slot
    }

    fn terrain(&mut self, x: i32, y: i32) {
        // 15 — `vZ.Block`: сквозь него не проходит ничто.
        self.cells[(y * self.w + x) as usize] = 15;
    }

    fn at(&self, x: i32, y: i32) -> u32 {
        self.cells[(y * self.w + x) as usize]
    }

    fn find(&self, slot: usize) -> Option<(i32, i32)> {
        let want = ELEMENT_MIN + slot as u32;
        (0..self.h).find_map(|y| {
            (0..self.w).find_map(|x| if self.at(x, y) == want { Some((x, y)) } else { None })
        })
    }

    fn world(&mut self) -> World<'_> {
        World {
            width: self.w,
            height: self.h,
            chunk_size: 40,
            chunk_should_update: &self.chunk_now,
            chunk_dirty_next: &mut self.chunk_next,
            chunk_width: (self.w + 39) / 40,
            chunk_height: (self.h + 39) / 40,
            cells: &mut self.cells,
            terrain_type: &self.terrain,
            changed: &mut self.changed,
            elements: Elements {
                kind: &mut self.kind,
                velocity_x: &mut self.vx,
                velocity_y: &mut self.vy,
                min_velocity_x: &mut self.minvx,
                min_velocity_y: &mut self.minvy,
                threshold_x: &mut self.tx,
                threshold_y: &mut self.ty,
                density: &mut self.density,
                is_free_falling: &mut self.falling,
                has_been_updated: &mut self.updated,
                skip_physics: &mut self.skip,
                has_duration: &mut self.duration,
                duration_left: &mut self.duration_left,
                x: &mut self.x,
                y: &mut self.y,
                last_side_checked: &mut self.side,
                moves_y_axis: &mut self.my,
                moves_y_axis_count: &mut self.myc,
            },
        }
    }

    /// Прогон тиков. Флаги «посчитано» гасятся между тиками — в игре это
    /// делает она сама по своему списку, куда попадает и наш.
    fn run(&mut self, table: &MatterTable, ticks: u64) -> EventQueue {
        let (w, h) = (self.w, self.h);
        let mut queue = EventQueue::with_capacity(1 << 16);
        let mut rng = Rng::new(12345);
        let mut marks = Marks::new();
        for t in 0..ticks {
            marks.clear();
            let mut world = self.world();
            update_region(
                &mut world,
                table,
                &mut queue,
                &mut rng,
                &mut marks,
                0,
                0,
                w,
                h,
                t % 2 == 0,
                1.0 / 60.0,
            );
            self.updated.fill(0);
        }
        queue
    }
}

/// Таблица материй с ролями и плотностями настоящей игры.
fn table() -> MatterTable {
    let mut t = MatterTable::new();
    t.set(SAND, Matter::Solid, 1.0);
    t.set(WATER, Matter::Liquid, 1.0);
    t.set(STEAM, Matter::Gas, 1.0);
    t.set(WET_SAND, Matter::Slushy, 1.0);
    t.set(LAVA, Matter::Liquid, 0.1);
    t.set_reactive(LAVA, true, false);
    t
}

// ---- движение ------------------------------------------------------------

#[test]
fn sand_falls_to_the_floor() {
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    let s = b.put(4, 0, SAND, 150.0);
    b.run(&table(), 60);
    assert_eq!(b.find(s), Some((4, 30)), "песчинка обязана лежать на терраине");
    assert_eq!(b.at(4, 0), 0, "и не остаться наверху");
}

#[test]
fn position_fields_follow_the_cell() {
    // Координаты в полях элемента обязаны ехать вместе с клеткой: на них
    // полагаются машины, конвейеры и сохранение.
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    let s = b.put(4, 0, SAND, 150.0);
    b.run(&table(), 60);
    assert_eq!((b.x[s] as i32, b.y[s] as i32), (4, 30), "позиция в полях разошлась с сеткой");
}

#[test]
fn sand_spreads_after_landing() {
    // Сыпучее в Sandustry расползается не диагональным шагом, а горизонтальной
    // скоростью, полученной при ударе. Столб песка обязан осесть в кучу шире
    // одной клетки — если этого нет, перенос `on_blocked`/`Ti` сломан.
    let mut b = Bench::new(32, 40);
    for x in 0..32 {
        b.terrain(x, 39);
    }
    for y in 20..38 {
        b.put(16, y, SAND, 150.0);
    }
    b.run(&table(), 240);
    let width = (0..32).filter(|&x| b.at(x, 38) >= ELEMENT_MIN).count();
    assert!(width > 1, "куча обязана расползтись, а осталась столбом шириной {width}");
}

#[test]
fn water_spreads_wider_than_sand() {
    let mut b = Bench::new(40, 12);
    for x in 0..40 {
        b.terrain(x, 11);
    }
    for y in 4..11 {
        b.put(20, y, WATER, 100.0);
    }
    b.run(&table(), 120);
    let width = (0..40).filter(|&x| b.at(x, 10) >= ELEMENT_MIN).count();
    assert!(width > 3, "вода обязана растекаться по дну, а заняла всего {width} клеток");
}

#[test]
fn water_stays_above_the_floor() {
    let mut b = Bench::new(16, 16);
    for x in 0..16 {
        b.terrain(x, 15);
    }
    let s = b.put(8, 2, WATER, 100.0);
    b.run(&table(), 120);
    let (_, y) = b.find(s).expect("вода не должна исчезать");
    assert!(y <= 14, "вода провалилась в терраин: y = {y}");
}

#[test]
fn gas_rises() {
    let mut b = Bench::new(16, 24);
    for x in 0..16 {
        b.terrain(x, 23);
    }
    let s = b.put(8, 20, STEAM, 25.0);
    b.run(&table(), 60);
    let (_, y) = b.find(s).expect("пар не должен исчезать");
    assert!(y < 20, "газ обязан всплывать, а остался на y = {y}");
}

#[test]
fn heavier_sinks_through_lighter() {
    let mut b = Bench::new(8, 16);
    for x in 0..8 {
        b.terrain(x, 15);
    }
    for y in 8..15 {
        b.put(4, y, WATER, 100.0);
    }
    let sand = b.put(4, 4, SAND, 150.0);
    b.run(&table(), 240);
    let (_, y) = b.find(sand).expect("песчинка не должна исчезать");
    assert!(y > 12, "тяжёлое обязано утонуть в лёгком, а осталось на y = {y}");
}

#[test]
fn wet_sand_holds_a_steeper_pile_than_dry() {
    // Вязкое получает при ударе вдвое больше горизонтальной скорости, но
    // тратит её медленнее. Проверяем не число, а то, что материалы ведут себя
    // по-разному: одинаковая куча означала бы, что конфиг материи потерян.
    let mut dry = Bench::new(40, 40);
    let mut wet = Bench::new(40, 40);
    for x in 0..40 {
        dry.terrain(x, 39);
        wet.terrain(x, 39);
    }
    for y in 20..38 {
        dry.put(20, y, SAND, 150.0);
        wet.put(20, y, WET_SAND, 150.0);
    }
    dry.run(&table(), 240);
    wet.run(&table(), 240);
    let dry_w = (0..40).filter(|&x| dry.at(x, 38) >= ELEMENT_MIN).count();
    let wet_w = (0..40).filter(|&x| wet.at(x, 38) >= ELEMENT_MIN).count();
    assert!(dry_w > 1 && wet_w > 1, "обе кучи обязаны осесть: сухая {dry_w}, мокрая {wet_w}");
}

// ---- соглашения игры -----------------------------------------------------

#[test]
fn element_marked_by_the_game_is_left_alone() {
    // §1 соглашений: элемент, посчитанный игрой в этом кадре, второй раз за тик
    // трогать нельзя — иначе он пройдёт двойной путь, и струя порвётся.
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    let s = b.put(4, 4, SAND, 150.0);
    b.updated[s] = 1;

    let mut queue = EventQueue::with_capacity(16);
    let mut rng = Rng::new(1);
    let mut marks = Marks::new();
    let mut world = b.world();
    update_element_cell(&mut world, &table(), &mut queue, &mut rng, &mut marks, s, 4, 4, 1.0 / 60.0);

    assert_eq!(b.find(s), Some((4, 4)), "помеченный игрой элемент не имеет права двигаться");
}

#[test]
fn moved_element_is_reported_back() {
    // Обратная сторона того же правила: элемент, который подвинули мы, обязан
    // попасть в список — иначе флаг никто не погасит и он залипнет навсегда.
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    let s = b.put(4, 4, SAND, 150.0);
    b.vy[s] = 300.0;
    b.ty[s] = 0.9;

    let mut queue = EventQueue::with_capacity(16);
    let mut rng = Rng::new(1);
    let mut marks = Marks::new();
    let mut world = b.world();
    let moved = update_element_cell(
        &mut world,
        &table(),
        &mut queue,
        &mut rng,
        &mut marks,
        s,
        4,
        4,
        1.0 / 60.0,
    );

    assert!(moved, "элемент с набранной скоростью обязан сдвинуться");
    assert_eq!(marks.slots(), &[s as u32], "сдвинутый элемент обязан попасть в список");
    assert_eq!(b.updated[s], 1, "и быть помеченным как посчитанный");
}

#[test]
fn sleeping_chunk_is_not_simulated() {
    // §2 и `cellOps.Do`: клетка из margin-полосы считается, только если её
    // собственный чанк активен в этом кадре.
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    let s = b.put(4, 4, SAND, 150.0);
    b.chunk_now.fill(0);

    let mut queue = EventQueue::with_capacity(16);
    let mut rng = Rng::new(1);
    let mut marks = Marks::new();
    let mut world = b.world();
    update_element_cell(&mut world, &table(), &mut queue, &mut rng, &mut marks, s, 4, 4, 1.0 / 60.0);

    assert_eq!(b.find(s), Some((4, 4)), "клетка спящего чанка не имеет права двигаться");
}

#[test]
fn falling_element_wakes_its_chunk() {
    // Пока частица только копит скорость, она никуда не едет — но чанк обязан
    // остаться активным, иначе физика встаёт до случайного пробуждения извне.
    let mut b = Bench::new(8, 32);
    let s = b.put(4, 4, SAND, 150.0);

    let mut queue = EventQueue::with_capacity(16);
    let mut rng = Rng::new(1);
    let mut marks = Marks::new();
    let mut world = b.world();
    update_element_cell(&mut world, &table(), &mut queue, &mut rng, &mut marks, s, 4, 4, 1.0 / 60.0);

    assert_eq!(b.chunk_next[0], 1, "чанк падающей частицы обязан проснуться");
}

#[test]
fn reaction_goes_back_to_js() {
    // Реакции ядро не считает никогда: за ними тянутся вторичные продукты,
    // звук и очки производства.
    let mut b = Bench::new(8, 16);
    for x in 0..8 {
        b.terrain(x, 15);
    }
    b.put(4, 8, LAVA, 300.0);
    let water = b.put(4, 7, WATER, 100.0);
    b.vy[water] = 300.0;
    b.ty[water] = 0.9;

    let mut queue = EventQueue::with_capacity(16);
    let mut rng = Rng::new(1);
    let mut marks = Marks::new();
    let mut world = b.world();
    update_element_cell(
        &mut world,
        &table(),
        &mut queue,
        &mut rng,
        &mut marks,
        water,
        4,
        7,
        1.0 / 60.0,
    );

    assert!(
        queue.events().iter().any(|e| matches!(e, Event::NeedsJs { .. })),
        "столкновение реагирующих типов обязано уйти в JS"
    );
}

#[test]
fn unknown_matter_goes_back_to_js() {
    // Всё, чего ядро не знает, обязано вернуться старому коду, а не тихо
    // остаться необработанным.
    let mut b = Bench::new(8, 8);
    b.put(4, 4, 200, 1000.0); // тип, которого нет в таблице
    let queue = b.run(&table(), 1);
    assert!(
        queue.events().iter().any(|e| matches!(e, Event::NeedsJs { .. })),
        "незнакомый материал обязан уйти в JS"
    );
}

#[test]
fn machine_owned_element_is_not_touched() {
    // `skipPhysics = AGGRESSIVE` означает, что элемент забрала машина.
    let mut b = Bench::new(8, 32);
    let s = b.put(4, 4, SAND, 150.0);
    b.skip[s] = crate::view::skip::AGGRESSIVE;
    b.run(&table(), 60);
    assert_eq!(b.find(s), Some((4, 4)), "элемент под контролем машины двигать нельзя");
}

// ---- отбор чанков --------------------------------------------------------

use crate::chunk::{inspect, Gate, Reason, Verdict};

fn open_gate() -> Gate<'static> {
    Gate { mod_hooks: &[], block_types: &[], block_width: 0, block_scale: 4 }
}

#[test]
fn plain_chunk_goes_native() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, SAND, 150.0);
    let t = table();
    let world = b.world();
    assert_eq!(inspect(&world, &t, &open_gate(), 0, 0, 16, 16), Verdict::Native);
}

#[test]
fn chunk_with_mod_hook_goes_back_to_js() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, SAND, 150.0);
    let t = table();
    // На тип песка подписан перехватчик мода.
    let mut hooks = vec![0u8; 256];
    hooks[SAND as usize] = 1;
    let gate = Gate { mod_hooks: &hooks, block_types: &[], block_width: 0, block_scale: 4 };
    let world = b.world();
    assert_eq!(
        inspect(&world, &t, &gate, 0, 0, 16, 16),
        Verdict::Skip(Reason::ModHook),
        "клетку с мод-хуком ядро трогать не имеет права"
    );
}

#[test]
fn chunk_with_structure_goes_back_to_js() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, SAND, 150.0);
    let t = table();
    // Сетка структур 4x4 тайла, машина в одном из них.
    let mut blocks = vec![0u8; 16];
    blocks[5] = 7;
    let gate = Gate { mod_hooks: &[], block_types: &blocks, block_width: 4, block_scale: 4 };
    let world = b.world();
    assert_eq!(inspect(&world, &t, &gate, 0, 0, 16, 16), Verdict::Skip(Reason::Structure));
}

#[test]
fn running_timer_keeps_physics_and_ticks_down() {
    // §8 в его настоящем виде: пока таймер тикает, элемент живёт обычной
    // физикой. Отдавать в JS весь чанк из-за таймера — значит отдать треть
    // обжитого мира и остаться без выигрыша.
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    let s = b.put(4, 0, SAND, 150.0);
    b.duration[s] = 1;
    b.duration_left[s] = 10.0;

    b.run(&table(), 60);

    assert_eq!(b.find(s), Some((4, 30)), "элемент с живым таймером обязан падать");
    assert!(b.duration_left[s] < 10.0, "и таймер обязан тикать: {}", b.duration_left[s]);
}

#[test]
fn expired_timer_goes_back_to_js() {
    // Истёк — дальше не наше дело: там превращение семени, гаснущий огонь,
    // очки производства и события модов.
    let mut b = Bench::new(8, 16);
    let s = b.put(4, 4, SAND, 150.0);
    b.duration[s] = 1;
    b.duration_left[s] = 0.001;

    let queue = b.run(&table(), 2);
    assert!(
        queue.events().iter().any(|e| matches!(e, Event::NeedsJs { .. })),
        "истёкший таймер обязан уйти в JS"
    );
}

#[test]
fn unknown_matter_keeps_the_chunk_in_js() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, 200, 1000.0); // типа нет в таблице
    let t = table();
    let world = b.world();
    assert_eq!(
        inspect(&world, &t, &open_gate(), 0, 0, 16, 16),
        Verdict::Skip(Reason::UnknownMatter)
    );
}
