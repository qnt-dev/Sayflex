//! Канал событий от основного приложения.
//!
//! Питомец — отдельный процесс, и он **не имеет своих хуков**: ни
//! клавиатурного, ни микрофона. Всё, что он знает о происходящем, приходит
//! строками в stdin. Это осознанное решение по приватности: второй глобальный
//! хук на систему вешать нельзя, а SayFlex уже знает всё нужное.
//!
//! Формат намеренно примитивный — по строке на событие, поля через пробел.
//! Разбирается без зависимостей, читается глазами в логе, легко эмулируется
//! для тестов:
//!
//! ```text
//! typing 0.8
//! voice 0.42
//! dictation start
//! dictation stop
//! insert ok
//! insert fail
//! translation ready
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
    /// Пользователь печатает. Значение — интенсивность 0…1, не сами клавиши:
    /// какие именно нажаты, питомцу знать незачем.
    Typing(f32),
    /// Уровень микрофона 0…1 во время диктовки.
    Voice(f32),
    DictationStarted,
    DictationFinished,
    InsertSucceeded,
    InsertFailed,
    TranslationReady,
    /// Родитель завершает работу. EOF является равнозначным резервным
    /// сигналом, поэтому питомец не может пережить закрывшееся приложение.
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
            ("typing", value) => Some(Self::Typing(
                value
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0),
            )),
            ("voice", Some(value)) => Some(Self::Voice(value.parse::<f32>().ok()?.clamp(0.0, 1.0))),
            ("dictation", Some("start")) => Some(Self::DictationStarted),
            ("dictation", Some("stop")) => Some(Self::DictationFinished),
            ("insert", Some("ok")) => Some(Self::InsertSucceeded),
            ("insert", Some("fail")) => Some(Self::InsertFailed),
            ("translation", Some("ready")) => Some(Self::TranslationReady),
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
    /// гибель IPC-потока: продолжать жить без родителя питомец не должен.
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
        assert_eq!(Event::parse("typing 0.8"), Some(Event::Typing(0.8)));
        assert_eq!(Event::parse("typing"), Some(Event::Typing(1.0)));
        assert_eq!(Event::parse("voice 0.42"), Some(Event::Voice(0.42)));
        assert_eq!(
            Event::parse("dictation start"),
            Some(Event::DictationStarted)
        );
        assert_eq!(Event::parse("insert fail"), Some(Event::InsertFailed));
        assert_eq!(
            Event::parse("translation ready"),
            Some(Event::TranslationReady)
        );
        assert_eq!(Event::parse("shutdown"), Some(Event::Shutdown));
    }

    #[test]
    fn clamps_values_and_ignores_junk() {
        assert_eq!(Event::parse("typing 9.0"), Some(Event::Typing(1.0)));
        assert_eq!(Event::parse("voice -3"), Some(Event::Voice(0.0)));
        assert_eq!(Event::parse(""), None);
        assert_eq!(Event::parse("что-то не то"), None);
        assert_eq!(Event::parse("voice"), None);
    }

    #[test]
    fn disconnected_parent_is_an_explicit_stop_condition() {
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let events = Events { receiver };
        let mut drained = Vec::new();

        assert!(!events.drain(&mut drained));
        assert!(drained.is_empty());
    }
}
