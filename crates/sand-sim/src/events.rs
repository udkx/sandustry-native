//! Очередь событий — единственный способ, которым ядро говорит с внешним миром.
//!
//! Игра на каждое перемещение клетки пересчитывает цвет в растре, метит чанк
//! грязным, проверяет коллектор и батарею, шлёт событие модам. Если бы ядро
//! дёргало JS на каждую такую мутацию, переходы между мирами съели бы весь
//! выигрыш от нативного кода. Поэтому мы копим события здесь и отдаём их одним
//! куском в конце тика.

/// Что произошло за тик и должно быть применено снаружи.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// Клетка переехала. Растр, dirty-флаги, коллекторы и события модов —
    /// работа принимающей стороны.
    Moved { from: (i32, i32), to: (i32, i32) },
    /// Клетку ядро обработать не берётся: у её типа есть перехватчик мода,
    /// рядом структура или это состояние материи, которого мы пока не знаем.
    /// Досчитать обязан старый код.
    NeedsJs { x: i32, y: i32, why: Why },
}

/// Почему клетка ушла в JS.
///
/// Досчёт стоит полутора микросекунд на клетку — дороже, чем ядро считает саму
/// физику. Пока непонятно, какая ветка их порождает, чинить нечего: без этого
/// счётчика легко ускорить редкую причину и не заметить массовую.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    /// Жидкость или газ, застоявшиеся дольше `STAGNANT_LIMIT`: испарение —
    /// это экономика (резервуар, удаление, событие).
    Stagnant,
    /// Пара элементов, которая во что-то превращается.
    Reaction,
    /// Истёк таймер: семя проросло, огонь погас, ферма начислила очки.
    Duration,
    /// Состояние материи, которого ядро не знает.
    UnknownMatter,
}

pub trait EventSink {
    fn push(&mut self, event: Event);
}

/// Кольцевой буфер фиксированного размера.
///
/// Размер задаётся заранее и не растёт: аллокация посреди тика — ровно та
/// непредсказуемая пауза, ради избавления от которой всё и затевалось. Если
/// событий больше, чем влезло, лишние считаются потерянными, а счётчик
/// переполнения говорит принимающей стороне, что тик надо доработать целиком
/// старым путём.
pub struct EventQueue {
    events: Vec<Event>,
    capacity: usize,
    overflow: usize,
}

impl EventQueue {
    pub fn with_capacity(capacity: usize) -> Self {
        EventQueue {
            events: Vec::with_capacity(capacity),
            capacity,
            overflow: 0,
        }
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.overflow = 0;
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn overflowed(&self) -> usize {
        self.overflow
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl EventSink for EventQueue {
    #[inline(always)]
    fn push(&mut self, event: Event) {
        if self.events.len() < self.capacity {
            self.events.push(event);
        } else {
            self.overflow += 1;
        }
    }
}

/// Приёмник, который всё выбрасывает — для замеров чистой физики.
pub struct NullSink;

impl EventSink for NullSink {
    #[inline(always)]
    fn push(&mut self, _event: Event) {}
}
