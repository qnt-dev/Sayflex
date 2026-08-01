//! Необязательный управляющий канал самостоятельного desktop pet.
//!
//! Обычный запуск не читает stdin и ни от какого другого продукта не зависит.
//! Канал включается только явным аргументом `--stdin-control`; он нужен для
//! автоматизированных проверок и будущих интеграций через нейтральный API.
//! Питомец сам не ставит глобальные хуки и не открывает микрофон.
//!
//! Формат намеренно примитивный — по строке на событие, поля через пробел.
//! Разбирается без зависимостей, читается глазами в логе, легко эмулируется
//! для тестов:
//!
//! ```text
//! activity 0.8
//! sound 0.42
//! focus on
//! celebrate
//! upset
//! shutdown
//! ```

use std::{
    io::{BufRead, BufReader},
    sync::mpsc::{self, Receiver},
    thread,
};

/// Что приложение сообщает питомцу.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// Абстрактная интенсивность внешней активности 0…1.
    Activity(f32),
    /// Абстрактный уровень звука 0…1; источник выбирает интеграция.
    Sound(f32),
    FocusStarted,
    FocusFinished,
    Celebrate,
    Upset,
    /// Завершает процесс в режиме явного stdin-контроля.
    Shutdown,
}

impl Event {
    /// Разбирает строку протокола. `None` — строка не распознана.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        let head = parts.next()?;
        let tail = parts.next();
        match (head, tail) {
            ("activity", value) => Some(Self::Activity(
                value
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0),
            )),
            ("sound", Some(value)) => Some(Self::Sound(value.parse::<f32>().ok()?.clamp(0.0, 1.0))),
            ("focus", Some("on")) => Some(Self::FocusStarted),
            ("focus", Some("off")) => Some(Self::FocusFinished),
            ("celebrate", None) => Some(Self::Celebrate),
            ("upset", None) => Some(Self::Upset),
            ("shutdown", None) => Some(Self::Shutdown),
            _ => None,
        }
    }
}

/// Читает события из stdin в отдельном потоке.
///
/// Отдельный поток нужен, чтобы чтение никогда не задерживало кадр: цикл
/// отрисовки только забирает накопившееся и идёт дальше.
pub struct Events {
    receiver: Receiver<Event>,
}

impl Events {
    #[must_use]
    pub fn from_stdin() -> Self {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("pet-ipc".to_owned())
            .spawn(move || {
                let stdin = std::io::stdin();
                for line in BufReader::new(stdin.lock()).lines().map_while(Result::ok) {
                    if let Some(event) = Event::parse(&line)
                        && sender.send(event).is_err()
                    {
                        break;
                    }
                }
            })
            .ok();
        Self { receiver }
    }

    /// Забирает всё накопившееся, не блокируя кадр. `false` означает EOF или
    /// гибель управляющего потока. Это условие завершает только экземпляр,
    /// который был явно запущен с `--stdin-control`.
    pub fn drain(&self, out: &mut Vec<Event>) -> bool {
        out.clear();
        loop {
            match self.receiver.try_recv() {
                Ok(event) => out.push(event),
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::{Event, Events};

    #[test]
    fn parses_every_documented_line() {
        assert_eq!(Event::parse("activity 0.8"), Some(Event::Activity(0.8)));
        assert_eq!(Event::parse("activity"), Some(Event::Activity(1.0)));
        assert_eq!(Event::parse("sound 0.42"), Some(Event::Sound(0.42)));
        assert_eq!(Event::parse("focus on"), Some(Event::FocusStarted));
        assert_eq!(Event::parse("upset"), Some(Event::Upset));
        assert_eq!(Event::parse("shutdown"), Some(Event::Shutdown));
    }

    #[test]
    fn clamps_values_and_ignores_junk() {
        assert_eq!(Event::parse("activity 9.0"), Some(Event::Activity(1.0)));
        assert_eq!(Event::parse("sound -3"), Some(Event::Sound(0.0)));
        assert_eq!(Event::parse(""), None);
        assert_eq!(Event::parse("что-то не то"), None);
        assert_eq!(Event::parse("sound"), None);
    }

    #[test]
    fn disconnected_control_stream_is_an_explicit_stop_condition() {
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let events = Events { receiver };
        let mut drained = Vec::new();

        assert!(!events.drain(&mut drained));
        assert!(drained.is_empty());
    }
}
