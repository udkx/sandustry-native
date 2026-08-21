//! Нативное ядро симуляции для Sandustry.
//!
//! Считает то же, что её `updateElementCell`, но в нативном коде и над той же
//! разделяемой памятью. Всё, что выходит за пределы движения клеток — звук,
//! растр, экономика, моды — возвращается наружу очередью событий (§ events).
//!
//! Ядро не знает ни про Node, ни про WASM: это чистая логика над срезами.
//! Обвязка живёт в отдельных крейтах, поэтому физику можно гонять в тестах и
//! бенчмарках без запуска игры.

#[cfg(test)]
mod tests;

pub mod chunk;
pub mod events;
pub mod physics;
pub mod view;

pub use chunk::{inspect, Gate, Reason, Stats, Verdict};
pub use events::{Event, EventQueue, EventSink, NullSink};
pub use physics::{reset_updated, update_region, Matter, MatterTable};
pub use view::{Elements, World};
