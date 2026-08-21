//! Базовые проверки поведения. Не «код написан», а «песок падает, вода течёт».

use crate::events::{Event, EventQueue};
use crate::physics::{reset_updated, update_region, Matter, MatterTable};
use crate::view::{Elements, World, ELEMENT_MIN};

/// Маленький мир для тестов: сетка плюс поля на несколько элементов.
struct Bench {
    w: i32,
    h: i32,
    cells: Vec<u32>,
    kind: Vec<u8>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    minvy: Vec<f32>,
    tx: Vec<f32>,
    ty: Vec<f32>,
    density: Vec<f32>,
    falling: Vec<u8>,
    updated: Vec<u8>,
    skip: Vec<u8>,
    x: Vec<u16>,
    y: Vec<u16>,
    side: Vec<i16>,
    my: Vec<u16>,
    myc: Vec<u16>,
    next: u32,
}

impl Bench {
    fn new(w: i32, h: i32) -> Self {
        let cap = 256;
        Bench {
            w,
            h,
            cells: vec![0; (w * h) as usize],
            kind: vec![0; cap],
            vx: vec![0.0; cap],
            vy: vec![0.0; cap],
            minvy: vec![0.0; cap],
            tx: vec![0.0; cap],
            ty: vec![0.0; cap],
            density: vec![0.0; cap],
            falling: vec![0; cap],
            updated: vec![0; cap],
            skip: vec![0; cap],
            x: vec![0; cap],
            y: vec![0; cap],
            side: vec![0; cap],
            my: vec![0; cap],
            myc: vec![0; cap],
            next: 0,
        }
    }

    fn put(&mut self, x: i32, y: i32, kind: u8, density: f32) {
        let slot = self.next as usize;
        self.next += 1;
        self.kind[slot] = kind;
        self.density[slot] = density;
        self.x[slot] = x as u16;
        self.y[slot] = y as u16;
        self.cells[(y * self.w + x) as usize] = ELEMENT_MIN + slot as u32;
    }

    fn terrain(&mut self, x: i32, y: i32) {
        self.cells[(y * self.w + x) as usize] = 15;
    }

    fn at(&self, x: i32, y: i32) -> u32 {
        self.cells[(y * self.w + x) as usize]
    }

    fn world(&mut self) -> World<'_> {
        World {
            width: self.w,
            height: self.h,
            chunk_size: 40,
            cells: &mut self.cells,
            elements: Elements {
                kind: &mut self.kind,
                velocity_x: &mut self.vx,
                velocity_y: &mut self.vy,
                min_velocity_y: &mut self.minvy,
                threshold_x: &mut self.tx,
                threshold_y: &mut self.ty,
                density: &mut self.density,
                is_free_falling: &mut self.falling,
                has_been_updated: &mut self.updated,
                skip_physics: &mut self.skip,
                x: &mut self.x,
                y: &mut self.y,
                last_side_checked: &mut self.side,
                moves_y_axis: &mut self.my,
                moves_y_axis_count: &mut self.myc,
            },
        }
    }

    fn run(&mut self, table: &MatterTable, ticks: u64) -> EventQueue {
        let (w, h) = (self.w, self.h);
        let mut queue = EventQueue::with_capacity(1 << 16);
        for t in 0..ticks {
            let mut world = self.world();
            reset_updated(&mut world, t);
            update_region(&mut world, table, &mut queue, 0, 0, w, h, t);
        }
        queue
    }
}

fn sand_table() -> MatterTable {
    let mut t = MatterTable::new();
    t.set(1, Matter::Powder, 0);
    t.set(2, Matter::Liquid, 5);
    t
}

#[test]
fn powder_falls_to_the_floor() {
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    b.put(4, 0, 1, 1600.0);
    b.run(&sand_table(), 60);
    assert_eq!(b.at(4, 30), ELEMENT_MIN, "песчинка обязана лежать на терраине");
    assert_eq!(b.at(4, 0), 0, "и не остаться наверху");
}

#[test]
fn powder_position_follows_the_cell() {
    // Координаты в полях элемента обязаны ехать вместе с клеткой: на них
    // полагаются машины, конвейеры и сохранение.
    let mut b = Bench::new(8, 32);
    for x in 0..8 {
        b.terrain(x, 31);
    }
    b.put(4, 0, 1, 1600.0);
    b.run(&sand_table(), 60);
    assert_eq!((b.x[0], b.y[0]), (4, 30), "позиция в полях разошлась с сеткой");
}

#[test]
fn heavier_sinks_through_lighter() {
    let mut b = Bench::new(8, 16);
    for x in 0..8 {
        b.terrain(x, 15);
    }
    for y in 8..15 {
        b.put(4, y, 2, 100.0); // столб жидкости
    }
    b.put(4, 4, 1, 1600.0); // песчинка над ним
    b.run(&sand_table(), 120);
    let sand_y = (0..16).find(|&y| b.at(4, y) == ELEMENT_MIN + 7);
    assert!(
        sand_y.is_some_and(|y| y > 12),
        "тяжёлое обязано утонуть в лёгком, а не лежать сверху: y = {sand_y:?}"
    );
}

#[test]
fn liquid_spreads_sideways() {
    let mut b = Bench::new(24, 8);
    for x in 0..24 {
        b.terrain(x, 7);
    }
    for _ in 0..6 {
        b.put(12, 6, 2, 100.0);
        b.run(&sand_table(), 4);
    }
    let width = (0..24).filter(|&x| b.at(x, 6) >= ELEMENT_MIN).count();
    assert!(width > 1, "жидкость обязана растекаться, а не стоять столбом");
}

#[test]
fn unknown_matter_goes_back_to_js() {
    // Всё, чего ядро не знает, обязано вернуться старому коду, а не тихо
    // остаться необработанным.
    let mut b = Bench::new(8, 8);
    b.put(4, 4, 200, 1000.0); // тип, которого нет в таблице
    let queue = b.run(&sand_table(), 1);
    assert!(
        queue.events().iter().any(|e| matches!(e, Event::NeedsJs { .. })),
        "незнакомый материал обязан уйти в JS"
    );
}

// ---- отбор чанков --------------------------------------------------------

use crate::chunk::{inspect, Gate, Reason, Verdict};

fn open_gate() -> Gate<'static> {
    Gate { mod_hooks: &[], block_types: &[], block_width: 0, block_scale: 4 }
}

#[test]
fn plain_chunk_goes_native() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, 1, 1600.0);
    let table = sand_table();
    let world = b.world();
    assert_eq!(inspect(&world, &table, &open_gate(), 0, 0, 16, 16), Verdict::Native);
}

#[test]
fn chunk_with_mod_hook_goes_back_to_js() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, 1, 1600.0);
    let table = sand_table();
    // На тип 1 подписан перехватчик мода.
    let mut hooks = vec![0u8; 256];
    hooks[1] = 1;
    let gate = Gate { mod_hooks: &hooks, block_types: &[], block_width: 0, block_scale: 4 };
    let world = b.world();
    assert_eq!(
        inspect(&world, &table, &gate, 0, 0, 16, 16),
        Verdict::Skip(Reason::ModHook),
        "клетку с мод-хуком ядро трогать не имеет права"
    );
}

#[test]
fn chunk_with_structure_goes_back_to_js() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, 1, 1600.0);
    let table = sand_table();
    // Сетка структур 4x4 тайла, машина в одном из них.
    let mut blocks = vec![0u8; 16];
    blocks[5] = 7;
    let gate = Gate { mod_hooks: &[], block_types: &blocks, block_width: 4, block_scale: 4 };
    let world = b.world();
    assert_eq!(
        inspect(&world, &table, &gate, 0, 0, 16, 16),
        Verdict::Skip(Reason::Structure)
    );
}

#[test]
fn unknown_matter_keeps_the_chunk_in_js() {
    let mut b = Bench::new(16, 16);
    b.put(4, 4, 200, 1000.0); // типа нет в таблице
    let table = sand_table();
    let world = b.world();
    assert_eq!(
        inspect(&world, &table, &open_gate(), 0, 0, 16, 16),
        Verdict::Skip(Reason::UnknownMatter)
    );
}
