use std::{
    collections::VecDeque,
    io,
    ops::ControlFlow,
    ptr,
    sync::{Arc, Mutex},
};

use windows_sys::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
        PostThreadMessageW, TranslateMessage, WM_APP, WM_QUIT,
    },
};

const COMMAND_WAKE_MESSAGE: u32 = WM_APP + 0x53;

struct DispatcherState<T> {
    thread_id: u32,
    commands: VecDeque<T>,
    closed: bool,
}

impl<T> Default for DispatcherState<T> {
    fn default() -> Self {
        Self {
            thread_id: 0,
            commands: VecDeque::new(),
            closed: false,
        }
    }
}

struct Shared<T> {
    state: Mutex<DispatcherState<T>>,
}

pub(crate) struct StaCommandSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for StaCommandSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> StaCommandSender<T> {
    pub(crate) fn send(&self, command: T) -> Result<(), T> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed || state.thread_id == 0 {
            return Err(command);
        }

        state.commands.push_back(command);
        if unsafe { PostThreadMessageW(state.thread_id, COMMAND_WAKE_MESSAGE, 0, 0) } == 0 {
            return Err(state
                .commands
                .pop_back()
                .expect("the command was appended while the queue lock was held"));
        }
        Ok(())
    }
}

pub(crate) struct StaDispatcher<T> {
    shared: Arc<Shared<T>>,
}

impl<T> StaDispatcher<T> {
    pub(crate) fn channel() -> (StaCommandSender<T>, Self) {
        let shared = Arc::new(Shared {
            state: Mutex::new(DispatcherState::default()),
        });
        (
            StaCommandSender {
                shared: Arc::clone(&shared),
            },
            Self { shared },
        )
    }

    pub(crate) fn bind_current_thread(&self) -> io::Result<()> {
        let mut queue_probe = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut queue_probe, ptr::null_mut(), 0, 0, PM_NOREMOVE);
        }

        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "STA dispatcher is closed",
            ));
        }
        if state.thread_id != 0 {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "STA dispatcher is already bound",
            ));
        }
        state.thread_id = unsafe { GetCurrentThreadId() };
        Ok(())
    }

    pub(crate) fn run(self, handler: impl FnMut(T) -> ControlFlow<()>) -> io::Result<()> {
        self.run_inner(handler, |_| {})
    }

    #[cfg(test)]
    fn run_with_message_observer(
        self,
        handler: impl FnMut(T) -> ControlFlow<()>,
        observer: impl FnMut(u32),
    ) -> io::Result<()> {
        self.run_inner(handler, observer)
    }

    fn run_inner(
        self,
        mut handler: impl FnMut(T) -> ControlFlow<()>,
        mut observer: impl FnMut(u32),
    ) -> io::Result<()> {
        loop {
            if !self.pump_pending_messages(&mut observer)? {
                return Ok(());
            }
            if let Some(command) = self.pop_command() {
                if handler(command).is_break() {
                    return Ok(());
                }
                continue;
            }

            let mut message = MSG::default();
            let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
            if result == 0 {
                return Ok(());
            }
            if result == -1 {
                return Err(io::Error::last_os_error());
            }
            self.dispatch_message(&message, &mut observer);
        }
    }

    fn pump_pending_messages(&self, observer: &mut impl FnMut(u32)) -> io::Result<bool> {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if message.message == WM_QUIT {
                return Ok(false);
            }
            self.dispatch_message(&message, observer);
        }
        Ok(true)
    }

    fn dispatch_message(&self, message: &MSG, observer: &mut impl FnMut(u32)) {
        if message.message == COMMAND_WAKE_MESSAGE {
            return;
        }
        observer(message.message);
        unsafe {
            let _ = TranslateMessage(message);
            DispatchMessageW(message);
        }
    }

    fn pop_command(&self) -> Option<T> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .commands
            .pop_front()
    }
}

impl<T> Drop for StaDispatcher<T> {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        state.thread_id = 0;
        state.commands.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{StaCommandSender, StaDispatcher};
    use std::{
        ops::ControlFlow,
        sync::{Arc, Mutex, mpsc},
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP},
    };

    const TEST_MESSAGE: u32 = WM_APP + 0x54;

    #[derive(Debug, PartialEq, Eq)]
    enum Command {
        Number(u8),
        Shutdown,
    }

    fn start_test_dispatcher(
        delivered: Arc<Mutex<Vec<u8>>>,
        observed: Arc<Mutex<Vec<u32>>>,
    ) -> (StaCommandSender<Command>, u32, thread::JoinHandle<()>) {
        let (sender, dispatcher) = StaDispatcher::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            dispatcher.bind_current_thread().unwrap();
            ready_sender.send(unsafe { GetCurrentThreadId() }).unwrap();
            dispatcher
                .run_with_message_observer(
                    |command| match command {
                        Command::Number(value) => {
                            delivered.lock().unwrap().push(value);
                            ControlFlow::Continue(())
                        }
                        Command::Shutdown => ControlFlow::Break(()),
                    },
                    |message| observed.lock().unwrap().push(message),
                )
                .unwrap();
        });
        let thread_id = ready_receiver.recv().unwrap();
        (sender, thread_id, handle)
    }

    #[test]
    fn idle_dispatcher_services_native_messages_before_the_next_command() {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let (sender, thread_id, handle) =
            start_test_dispatcher(Arc::clone(&delivered), Arc::clone(&observed));

        assert_ne!(
            unsafe { PostThreadMessageW(thread_id, TEST_MESSAGE, 0, 0) },
            0
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while !observed.lock().unwrap().contains(&TEST_MESSAGE) && Instant::now() < deadline {
            thread::yield_now();
        }

        assert_eq!(observed.lock().unwrap().as_slice(), &[TEST_MESSAGE]);
        assert!(delivered.lock().unwrap().is_empty());
        sender.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn commands_remain_fifo_when_multiple_wakes_are_pending() {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let (sender, _thread_id, handle) = start_test_dispatcher(Arc::clone(&delivered), observed);

        sender.send(Command::Number(1)).unwrap();
        sender.send(Command::Number(2)).unwrap();
        sender.send(Command::Number(3)).unwrap();
        sender.send(Command::Shutdown).unwrap();
        handle.join().unwrap();

        assert_eq!(delivered.lock().unwrap().as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn a_closed_dispatcher_returns_the_unsent_command() {
        let (sender, dispatcher) = StaDispatcher::channel();
        dispatcher.bind_current_thread().unwrap();
        drop(dispatcher);

        assert_eq!(sender.send(Command::Number(7)), Err(Command::Number(7)));
    }
}
