//! Связь с настольным питомцем.
//!
//! Питомец — отдельный процесс, и **своих хуков у него нет**: ни
//! клавиатурного, ни микрофона. Обо всём происходящем он узнаёт только из
//! строк, которые SayFlex пишет ему в stdin. Это осознанное решение: второй
//! глобальный хук на систему вешать нельзя, а всё нужное SayFlex и так знает.
//!
//! Наружу уходит минимум: интенсивность печати, а не клавиши; уровень
//! микрофона, а не звук; факт вставки, а не текст. По этому каналу нельзя
//! восстановить ничего из того, что пользователь диктовал или печатал.
//!
//! Связь необязательная. Если исполняемого файла рядом нет или процесс упал,
//! всё молча выключается — на работу SayFlex это не влияет никак.

use std::{
    env,
    io::Write,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const PET_EXE: &str = "SayFlex.Pet.exe";
const DEV_PET_EXE: &str = "pet_host.exe";

/// Уровень микрофона приходит десятками раз в секунду, а питомцу хватает
/// нескольких: он рисует не осциллограмму, а настроение.
const VOICE_INTERVAL: Duration = Duration::from_millis(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(20);

struct Link {
    stdin: ChildStdin,
    /// Держим процесс, чтобы он не стал зомби после завершения.
    child: Child,
    last_voice: Instant,
}

static LINK: OnceLock<Mutex<Option<Link>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Link>> {
    LINK.get_or_init(|| Mutex::new(None))
}

/// Ищет исполняемый файл рядом с собой: сначала имя из установщика, затем имя
/// из сборки рабочего дерева.
fn executable_path() -> Option<PathBuf> {
    let parent = env::current_exe().ok()?.parent()?.to_path_buf();
    [PET_EXE, DEV_PET_EXE]
        .into_iter()
        .map(|name| parent.join(name))
        .find(|path| path.exists())
}

/// Запускает питомца, если он установлен. Ошибку не возвращает: отсутствие
/// питомца — это не сбой приложения.
pub fn start(worker_job: &rust_platform_windows::WorkerJob) {
    let Some(path) = executable_path() else {
        rust_app_core::diagnostics::log_line("pet lifecycle=unavailable reason=executable_missing");
        return;
    };
    let mut command = Command::new(&path);
    command
        .stdin(Stdio::piped())
        // Питомец ничего не отвечает; его вывод — только диагностика в консоль
        // разработчика, и наследовать её в установленном приложении незачем.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            rust_app_core::diagnostics::log_line(&format!(
                "pet lifecycle=start_failed path={} error={error}",
                path.display()
            ));
            return;
        }
    };
    // В том же job-объекте, что и рабочие процессы: если SayFlex умрёт, на
    // экране не останется висеть осиротевший персонаж.
    if let Err(error) = worker_job.assign(&child) {
        rust_app_core::diagnostics::log_line(&format!(
            "pet lifecycle=start_failed stage=job_assignment error={error}"
        ));
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    let Some(stdin) = child.stdin.take() else {
        rust_app_core::diagnostics::log_line("pet lifecycle=start_failed stage=stdin_pipe");
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    rust_app_core::diagnostics::log_line(&format!(
        "pet lifecycle=started pid={} path={}",
        child.id(),
        path.display()
    ));
    let replacement = Link {
        stdin,
        child,
        last_voice: Instant::now() - VOICE_INTERVAL,
    };
    let previous = {
        let mut guard = slot().lock().unwrap_or_else(|error| error.into_inner());
        guard.replace(replacement)
    };
    if let Some(previous) = previous {
        terminate(previous, "replaced");
    }
}

/// Останавливает питомца явной командой и закрытием канала. Ожидание всегда
/// ограничено: сбой необязательного процесса не имеет права удерживать выход
/// основного приложения или завершение Windows.
pub fn stop() {
    let link = slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(link) = link {
        terminate(link, "shutdown");
    }
}

fn terminate(mut link: Link, reason: &str) {
    let _ = writeln!(link.stdin, "shutdown");
    let _ = link.stdin.flush();
    drop(link.stdin);

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        match link.child.try_wait() {
            Ok(Some(status)) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "pet lifecycle=exited reason={reason} status={status}"
                ));
                return;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(REAP_POLL_INTERVAL);
            }
            Ok(None) | Err(_) => break,
        }
    }

    rust_app_core::diagnostics::log_line(&format!(
        "pet lifecycle=forced_termination reason={reason}"
    ));
    let _ = link.child.kill();
    let _ = link.child.wait();
}

/// Пишет строку протокола. Упавший процесс отцепляется навсегда: пытаться
/// перезапускать его на каждом событии дороже, чем обойтись без питомца.
fn send(line: &str) {
    let mut guard = slot().lock().unwrap_or_else(|error| error.into_inner());
    let Some(link) = guard.as_mut() else {
        return;
    };
    if writeln!(link.stdin, "{line}").is_err() || link.stdin.flush().is_err() {
        let failed = guard.take();
        drop(guard);
        if let Some(failed) = failed {
            terminate(failed, "broken_pipe");
        }
    }
}

/// Пользователь печатает. Уходит только интенсивность 0…1 — какие именно
/// клавиши нажаты, питомец не узнаёт и знать не должен.
pub fn typing(intensity: f32) {
    send(&format!("typing {:.2}", intensity.clamp(0.0, 1.0)));
}

/// Уровень микрофона во время диктовки, прорежённый по времени.
pub fn voice(peak: f32) {
    let mut guard = slot().lock().unwrap_or_else(|error| error.into_inner());
    let Some(link) = guard.as_mut() else {
        return;
    };
    if link.last_voice.elapsed() < VOICE_INTERVAL {
        return;
    }
    link.last_voice = Instant::now();
    let line = format!("voice {:.2}", peak.clamp(0.0, 1.0));
    if writeln!(link.stdin, "{line}").is_err() || link.stdin.flush().is_err() {
        let failed = guard.take();
        drop(guard);
        if let Some(failed) = failed {
            terminate(failed, "voice_broken_pipe");
        }
    }
}

pub fn dictation_started() {
    send("dictation start");
}

pub fn dictation_finished() {
    send("dictation stop");
}

pub fn insert_succeeded() {
    send("insert ok");
}

pub fn insert_failed() {
    send("insert fail");
}

pub fn translation_ready() {
    send("translation ready");
}
