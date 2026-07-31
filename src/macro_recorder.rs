//! Windows-only, privacy-preserving input recording for the RPA editor.
//!
//! The recorder deliberately omits pointer moves and normal keyboard text. It only captures
//! mouse button presses and a small allow-list of navigation/clipboard shortcuts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroRecording {
    pub events: Vec<RecordedMacroEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedMacroEvent {
    /// Milliseconds since the recorder was started. This can be converted to wait steps.
    pub after_ms: u64,
    pub window: RecordedWindow,
    pub action: RecordedAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedWindow {
    /// Native Windows HWND. It must be re-selected/validated before a macro is played back.
    pub hwnd: usize,
    pub title: String,
    pub process_id: u32,
    pub rect: RecordedRect,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RecordedRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecordedAction {
    Click {
        button: RecordedMouseButton,
        /// Position relative to the recorded foreground window, normalized to 0.0–1.0.
        normalized_x: f32,
        normalized_y: f32,
    },
    Shortcut(RecordedShortcut),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum RecordedMouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum RecordedShortcut {
    Copy,
    Paste,
    SelectAll,
    Enter,
    Escape,
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::{
        sync::{mpsc, Arc, Mutex, OnceLock},
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU},
            WindowsAndMessaging::{
                CallNextHookEx, GetAncestor, GetForegroundWindow, GetMessageW, GetWindowRect,
                GetWindowTextW, GetWindowThreadProcessId, PostQuitMessage, PostThreadMessageW,
                SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HC_ACTION,
                KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP,
                WM_KEYDOWN, WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSKEYDOWN,
            },
        },
    };

    const STOP_MESSAGE: u32 = WM_APP + 0x415;
    const LL_MOUSE_INJECTED: u32 = 0x0000_0001;
    const LL_KEYBOARD_INJECTED: u32 = 0x0000_0010;
    const VK_C: u32 = b'C' as u32;
    const VK_V: u32 = b'V' as u32;
    const VK_A: u32 = b'A' as u32;
    const VK_RETURN: u32 = 0x0d;
    const VK_ESCAPE: u32 = 0x1b;

    struct SharedRecording {
        started: Instant,
        events: Mutex<Vec<RecordedMacroEvent>>,
    }

    static ACTIVE: OnceLock<Mutex<Option<Arc<SharedRecording>>>> = OnceLock::new();

    fn active_slot() -> &'static Mutex<Option<Arc<SharedRecording>>> {
        ACTIVE.get_or_init(|| Mutex::new(None))
    }

    pub struct MacroRecorder {
        thread_id: u32,
        shared: Arc<SharedRecording>,
        join: Option<JoinHandle<Result<(), String>>>,
    }

    impl MacroRecorder {
        pub fn start() -> Result<Self, String> {
            let shared = Arc::new(SharedRecording {
                started: Instant::now(),
                events: Mutex::new(Vec::new()),
            });
            {
                let mut active = active_slot()
                    .lock()
                    .map_err(|_| "Makroinspelaren låste sig.")?;
                if active.is_some() {
                    return Err("En makroinspelning pågår redan.".to_string());
                }
                *active = Some(Arc::clone(&shared));
            }

            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let join = thread::spawn(move || hook_thread(ready_tx));
            let thread_id = match ready_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(id)) => id,
                Ok(Err(error)) => {
                    let _ = join.join();
                    clear_active();
                    return Err(error);
                }
                Err(_) => {
                    // The thread is allowed to finish naturally; the global slot is still cleared.
                    clear_active();
                    return Err(
                        "Makroinspelaren kunde inte starta Windows-hakarna inom 3 sekunder."
                            .to_string(),
                    );
                }
            };
            Ok(Self {
                thread_id,
                shared,
                join: Some(join),
            })
        }

        pub fn stop(mut self) -> Result<MacroRecording, String> {
            self.stop_inner()
        }

        pub fn is_finished(&self) -> bool {
            self.join.as_ref().is_none_or(JoinHandle::is_finished)
        }

        fn stop_inner(&mut self) -> Result<MacroRecording, String> {
            if !self.is_finished() {
                unsafe {
                    PostThreadMessageW(self.thread_id, STOP_MESSAGE, WPARAM(0), LPARAM(0))
                        .map_err(|error| format!("Kunde inte stoppa makroinspelaren: {error}"))?;
                }
            }
            if let Some(join) = self.join.take() {
                join.join()
                    .map_err(|_| "Makroinspelarens tråd avslutades oväntat.".to_string())??;
            }
            let events = self
                .shared
                .events
                .lock()
                .map_err(|_| "Makrohändelserna låste sig.")?
                .clone();
            Ok(MacroRecording { events })
        }
    }

    impl Drop for MacroRecorder {
        fn drop(&mut self) {
            if self.join.is_some() {
                let _ = self.stop_inner();
            }
        }
    }

    fn clear_active() {
        if let Ok(mut active) = active_slot().lock() {
            *active = None;
        }
    }

    fn hook_thread(ready: mpsc::SyncSender<Result<u32, String>>) -> Result<(), String> {
        let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) }
            .map_err(|error| format!("Kunde inte installera mus-haken: {error}"));
        let mouse = match mouse {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready.send(Err(error.clone()));
                clear_active();
                return Err(error);
            }
        };
        let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) }
            .map_err(|error| format!("Kunde inte installera tangentbords-haken: {error}"));
        let keyboard = match keyboard {
            Ok(hook) => hook,
            Err(error) => {
                unsafe {
                    let _ = UnhookWindowsHookEx(mouse);
                }
                let _ = ready.send(Err(error.clone()));
                clear_active();
                return Err(error);
            }
        };

        let thread_id = unsafe { GetCurrentThreadId() };
        let _ = ready.send(Ok(thread_id));
        let mut message = MSG::default();
        loop {
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 == -1 {
                unsafe {
                    let _ = UnhookWindowsHookEx(keyboard);
                    let _ = UnhookWindowsHookEx(mouse);
                }
                clear_active();
                return Err("Windows meddelandekö för makroinspelaren misslyckades.".to_string());
            }
            if result.0 == 0 || message.message == STOP_MESSAGE {
                break;
            }
        }
        unsafe {
            let _ = UnhookWindowsHookEx(keyboard);
            let _ = UnhookWindowsHookEx(mouse);
        }
        clear_active();
        Ok(())
    }

    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if info.flags & LL_MOUSE_INJECTED == 0 {
                let button = match wparam.0 as u32 {
                    WM_LBUTTONDOWN => Some(RecordedMouseButton::Left),
                    WM_RBUTTONDOWN => Some(RecordedMouseButton::Right),
                    _ => None,
                };
                if let (Some(button), Some(window)) = (button, window_at_point(info.pt)) {
                    let x = normalize(info.pt.x, window.rect.x, window.rect.width);
                    let y = normalize(info.pt.y, window.rect.y, window.rect.height);
                    push_event(
                        window,
                        RecordedAction::Click {
                            button,
                            normalized_x: x,
                            normalized_y: y,
                        },
                    );
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            if info.flags.0 & LL_KEYBOARD_INJECTED == 0 {
                let control = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
                let alt = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } < 0;
                if control && alt && info.vkCode == VK_ESCAPE {
                    unsafe { PostQuitMessage(0) };
                    return unsafe { CallNextHookEx(None, code, wparam, lparam) };
                }
                let shortcut = match (control, info.vkCode) {
                    (true, VK_C) => Some(RecordedShortcut::Copy),
                    (true, VK_V) => Some(RecordedShortcut::Paste),
                    (true, VK_A) => Some(RecordedShortcut::SelectAll),
                    (_, VK_RETURN) => Some(RecordedShortcut::Enter),
                    (_, VK_ESCAPE) => Some(RecordedShortcut::Escape),
                    _ => None,
                };
                if let (Some(shortcut), Some(window)) = (shortcut, foreground_window()) {
                    push_event(window, RecordedAction::Shortcut(shortcut));
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn push_event(window: RecordedWindow, action: RecordedAction) {
        if window.process_id == std::process::id() {
            return;
        }
        let Some(shared) = active_slot().lock().ok().and_then(|active| active.clone()) else {
            return;
        };
        let event = RecordedMacroEvent {
            after_ms: shared
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            window,
            action,
        };
        if let Ok(mut events) = shared.events.lock() {
            if events.len() < 10_000 {
                events.push(event);
            }
        };
    }

    fn foreground_window() -> Option<RecordedWindow> {
        recorded_window(unsafe { GetForegroundWindow() })
    }

    fn window_at_point(point: POINT) -> Option<RecordedWindow> {
        let window = unsafe { WindowFromPoint(point) };
        let root = unsafe { GetAncestor(window, GA_ROOT) };
        recorded_window(if root.0.is_null() { window } else { root })
    }

    fn recorded_window(hwnd: HWND) -> Option<RecordedWindow> {
        if hwnd.0.is_null() {
            return None;
        }
        unsafe {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return None;
            }
            let width = (rect.right - rect.left).max(1) as u32;
            let height = (rect.bottom - rect.top).max(1) as u32;
            let mut process_id = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            let mut title = [0u16; 512];
            let length = GetWindowTextW(hwnd, &mut title).max(0) as usize;
            Some(RecordedWindow {
                hwnd: hwnd.0 as usize,
                title: String::from_utf16_lossy(&title[..length]),
                process_id,
                rect: RecordedRect {
                    x: rect.left,
                    y: rect.top,
                    width,
                    height,
                },
            })
        }
    }

    fn normalize(position: i32, origin: i32, size: u32) -> f32 {
        ((position - origin) as f32 / size.max(1) as f32).clamp(0.0, 1.0)
    }
}

#[cfg(target_os = "windows")]
pub use platform::MacroRecorder;

#[cfg(not(target_os = "windows"))]
pub struct MacroRecorder;

#[cfg(not(target_os = "windows"))]
impl MacroRecorder {
    pub fn start() -> Result<Self, String> {
        Err(
            "Global makroinspelning är för närvarande endast implementerad för Windows 10/11."
                .to_string(),
        )
    }

    pub fn stop(self) -> Result<MacroRecording, String> {
        Err(
            "Global makroinspelning är för närvarande endast implementerad för Windows 10/11."
                .to_string(),
        )
    }

    pub fn is_finished(&self) -> bool {
        false
    }
}
