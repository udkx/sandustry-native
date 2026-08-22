//! Замер ядра на снимке настоящего мира.
//!
//! Снимок снимается из игры профайлером и лежит в файле, поэтому вход
//! зафиксирован: прогон сегодня и прогон через месяц стартуют с одного и того
//! же состояния. Перед каждой итерацией состояние восстанавливается — тик
//! мутирует мир, и без сброса второй замер шёл бы уже по осевшей сцене.
//!
//!     cargo run --release -p sand-bench -- fixtures/busy-320.json [итераций]

use std::time::Instant;

use sand_sim::events::{EventQueue, NullSink};
use sand_sim::physics::{update_region, Marks, Matter, MatterTable, Rng};
use sand_sim::view::{Elements, World};

use serde::Deserialize;

/// Шаг времени игры: один кадр из шестидесяти.
const DT: f32 = 1.0 / 60.0;

#[derive(Deserialize)]
struct Dump {
    world: WorldMeta,
    region: Region,
    counts: Counts,
    cells: Vec<u32>,
    elements: Vec<Element>,
}

#[derive(Deserialize)]
struct WorldMeta {
    #[serde(rename = "chunkSize")]
    chunk_size: i32,
}

#[derive(Deserialize, Clone, Copy)]
struct Region {
    w: i32,
    h: i32,
}

#[derive(Deserialize)]
struct Counts {
    cells: usize,
    #[serde(rename = "nonEmpty")]
    non_empty: usize,
    elements: usize,
}

#[derive(Deserialize, Clone)]
struct Element {
    i: u32,
    #[serde(rename = "type")]
    kind: u32,
    #[serde(rename = "velocityX")]
    velocity_x: f32,
    #[serde(rename = "velocityY")]
    velocity_y: f32,
    #[serde(rename = "minVelocityY")]
    min_velocity_y: f32,
    #[serde(rename = "thresholdX")]
    threshold_x: f32,
    #[serde(rename = "thresholdY")]
    threshold_y: f32,
    density: f32,
    #[serde(rename = "isFreeFalling")]
    is_free_falling: u32,
    #[serde(rename = "skipPhysics")]
    skip_physics: u32,
    x: u32,
    y: u32,
    #[serde(rename = "lastSideChecked")]
    last_side_checked: i32,
    #[serde(rename = "movesYAxis")]
    moves_y_axis: u32,
    #[serde(rename = "movesYAxisCount")]
    moves_y_axis_count: u32,
}

/// Снимок в виде, пригодном для быстрого восстановления: клонируется целиком
/// перед каждой итерацией.
#[derive(Clone)]
struct State {
    cells: Vec<u32>,
    kind: Vec<u8>,
    velocity_x: Vec<f32>,
    velocity_y: Vec<f32>,
    min_velocity_x: Vec<f32>,
    min_velocity_y: Vec<f32>,
    threshold_x: Vec<f32>,
    threshold_y: Vec<f32>,
    density: Vec<f32>,
    is_free_falling: Vec<u8>,
    has_been_updated: Vec<u8>,
    skip_physics: Vec<u8>,
    has_duration: Vec<u8>,
    duration_left: Vec<f32>,
    x: Vec<u16>,
    y: Vec<u16>,
    last_side_checked: Vec<i16>,
    moves_y_axis: Vec<u16>,
    moves_y_axis_count: Vec<u16>,
    chunk_now: Vec<u8>,
    chunk_dirty: Vec<u8>,
    terrain_type: Vec<u8>,
    changed: Vec<u32>,
}

impl State {
    fn from_dump(d: &Dump) -> Self {
        let cap = d.elements.iter().map(|e| e.i).max().unwrap_or(0) as usize + 1;
        let mut s = State {
            cells: d.cells.clone(),
            kind: vec![0; cap],
            velocity_x: vec![0.0; cap],
            velocity_y: vec![0.0; cap],
            min_velocity_x: vec![0.0; cap],
            min_velocity_y: vec![0.0; cap],
            threshold_x: vec![0.0; cap],
            threshold_y: vec![0.0; cap],
            density: vec![0.0; cap],
            is_free_falling: vec![0; cap],
            has_been_updated: vec![0; cap],
            skip_physics: vec![0; cap],
            has_duration: vec![0; cap],
            duration_left: vec![0.0; cap],
            x: vec![0; cap],
            y: vec![0; cap],
            last_side_checked: vec![0; cap],
            moves_y_axis: vec![0; cap],
            moves_y_axis_count: vec![0; cap],
            chunk_now: Vec::new(),
            chunk_dirty: Vec::new(),
            // В снимке таблицы терраина нет: там идентификатор клетки и есть
            // её тип.
            terrain_type: (0..=1000u32).map(|i| i as u8).collect(),
            changed: Vec::new(),
        };
        for e in &d.elements {
            let i = e.i as usize;
            s.kind[i] = e.kind as u8;
            s.velocity_x[i] = e.velocity_x;
            s.velocity_y[i] = e.velocity_y;
            s.min_velocity_y[i] = e.min_velocity_y;
            s.threshold_x[i] = e.threshold_x;
            s.threshold_y[i] = e.threshold_y;
            s.density[i] = e.density;
            s.is_free_falling[i] = e.is_free_falling as u8;
            s.skip_physics[i] = e.skip_physics as u8;
            s.x[i] = e.x as u16;
            s.y[i] = e.y as u16;
            s.last_side_checked[i] = e.last_side_checked as i16;
            s.moves_y_axis[i] = e.moves_y_axis as u16;
            s.moves_y_axis_count[i] = e.moves_y_axis_count as u16;
        }
        s
    }

    fn world(&mut self, region: Region, chunk_size: i32) -> World<'_> {
        let cw = (region.w + chunk_size - 1) / chunk_size;
        let ch = (region.h + chunk_size - 1) / chunk_size;
        self.chunk_dirty.resize((cw * ch) as usize, 0);
        // Снимок берётся из живого мира целиком: все его чанки считаем
        // активными, иначе замер мерил бы пропуск, а не работу.
        self.chunk_now.resize((cw * ch) as usize, 1);
        World {
            width: region.w,
            height: region.h,
            chunk_size,
            chunk_should_update: &self.chunk_now,
            chunk_dirty_next: &mut self.chunk_dirty,
            terrain_type: &self.terrain_type,
            chunk_width: cw,
            chunk_height: ch,
            cells: &mut self.cells,
            changed: &mut self.changed,
            elements: Elements {
                kind: &mut self.kind,
                velocity_x: &mut self.velocity_x,
                velocity_y: &mut self.velocity_y,
                min_velocity_x: &mut self.min_velocity_x,
                min_velocity_y: &mut self.min_velocity_y,
                threshold_x: &mut self.threshold_x,
                threshold_y: &mut self.threshold_y,
                density: &mut self.density,
                is_free_falling: &mut self.is_free_falling,
                has_been_updated: &mut self.has_been_updated,
                skip_physics: &mut self.skip_physics,
                has_duration: &mut self.has_duration,
                duration_left: &mut self.duration_left,
                x: &mut self.x,
                y: &mut self.y,
                last_side_checked: &mut self.last_side_checked,
                moves_y_axis: &mut self.moves_y_axis,
                moves_y_axis_count: &mut self.moves_y_axis_count,
            },
        }
    }

    fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &c in &self.cells {
            h ^= c as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }
}

/// Таблица состояний материи.
///
/// В игре она строится из `elementDefinitions`, включая элементы модов, и
/// приезжает со стороны JS. Здесь — набор по умолчанию, покрывающий базовые
/// материалы: всё, чего в нём нет, ядро вернёт в JS как необработанное.
fn default_table(state: &State) -> MatterTable {
    let mut table = MatterTable::new();
    // Плотность — надёжный признак: жидкости в игре легче сыпучего и текут
    // вбок, сыпучее тяжелее и только осыпается. Настоящую таблицу пришлёт JS,
    // здесь достаточно разумного приближения, чтобы замер был честным.
    let mut seen = [false; 256];
    for i in 0..state.kind.len() {
        let k = state.kind[i];
        if k == 0 || seen[k as usize] {
            continue;
        }
        seen[k as usize] = true;
        let d = state.density[i];
        // Песок в игре — Solid, а не Powder: у Powder обработчика движения
        // нет вовсе, и приняв одно за другое, замер мерил бы стоящий мир.
        if d <= 120.0 {
            table.set(k, Matter::Liquid, 1.0);
        } else {
            table.set(k, Matter::Solid, 1.0);
        }
    }
    table
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("укажите снимок: cargo run --release -p sand-bench -- fixtures/busy-320.json");
            std::process::exit(2);
        }
    };
    let iterations: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(500);

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("не читается {path}: {e}");
        std::process::exit(2);
    });
    let dump: Dump = serde_json::from_str(&raw).unwrap_or_else(|e| {
        eprintln!("не разбирается {path}: {e}");
        std::process::exit(2);
    });

    let region = dump.region;
    let mut pristine = State::from_dump(&dump);

    // Снимок плотно забитой сцены меряет в основном холостой скан: массе
    // некуда двигаться. Ключ --carve вырезает в середине полость, масса
    // обрушивается, и замер показывает цену настоящей работы.
    if std::env::args().any(|a| a == "--carve") {
        let (w, h) = (region.w, region.h);
        for y in h / 2..h {
            for x in w / 4..(3 * w) / 4 {
                pristine.cells[(y * w + x) as usize] = 0;
            }
        }
        println!("вырезана полость: нижняя половина центральной части очищена");
    }
    let pristine = pristine;
    let table = default_table(&pristine);

    println!(
        "снимок {}x{}: клеток {}, занято {} ({:.1}%), элементов {}",
        region.w,
        region.h,
        dump.counts.cells,
        dump.counts.non_empty,
        100.0 * dump.counts.non_empty as f64 / dump.counts.cells as f64,
        dump.counts.elements,
    );

    for _ in 0..20 {
        let mut s = pristine.clone();
        let mut rng = Rng::new(1);
        let mut marks = Marks::new();
        let mut w = s.world(region, dump.world.chunk_size);
        update_region(
            &mut w,
            &table,
            &mut NullSink,
            &mut rng,
            &mut marks,
            0,
            0,
            region.w,
            region.h,
            true,
            DT,
        );
    }

    let mut times = Vec::with_capacity(iterations);
    let mut touched = 0usize;
    let mut moved = 0usize;
    let mut needs_js = 0usize;
    let mut hash = 0u64;

    for it in 0..iterations {
        let mut s = pristine.clone();
        let mut queue = EventQueue::with_capacity(1 << 18);
        let t0 = Instant::now();
        let mut rng = Rng::new(it as u64 + 1);
        let mut marks = Marks::new();
        let n = {
            let mut w = s.world(region, dump.world.chunk_size);
            update_region(
                &mut w,
                &table,
                &mut queue,
                &mut rng,
                &mut marks,
                0,
                0,
                region.w,
                region.h,
                it % 2 == 0,
                DT,
            )
        };
        times.push(t0.elapsed().as_nanos() as u64);
        touched += n;
        moved += sand_sim::last_moved();
        needs_js += queue
            .events()
            .iter()
            .filter(|e| matches!(e, sand_sim::Event::NeedsJs { .. }))
            .count();
        hash = s.hash();
    }

    times.sort_unstable();
    let median = times[times.len() / 2] as f64;
    let p95 = times[(times.len() as f64 * 0.95) as usize] as f64;
    let cells = dump.counts.cells as f64;
    let touched_avg = touched as f64 / iterations as f64;

    println!();
    println!("обработано клеток за тик: {touched_avg:.0}");
    println!(
        "  из них переместилось {:.0}, отдано в JS {:.0}",
        moved as f64 / iterations as f64,
        needs_js as f64 / iterations as f64
    );
    println!("тик: медиана {:.3} мс, p95 {:.3} мс", median / 1e6, p95 / 1e6);
    println!("на просканированную клетку: {:.2} нс", median / cells);
    if touched_avg > 0.0 {
        println!("на активную клетку:         {:.2} нс", median / touched_avg);
    }
    println!("хеш после тика: {hash:016x}");
}
