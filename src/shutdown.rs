//! Cooperative shutdown: notice that Windows (or the user) wants us gone and
//! unwind the display loop, instead of being hard-terminated mid-HID-write.
//!
//! Two notification paths, because this binary lives in both worlds:
//!   * **Console control events** (Ctrl+C, console close, logoff, shutdown) —
//!     delivered whenever a console is attached, i.e. `cargo run` / debug builds.
//!   * **A hidden top-level window** pumping `WM_QUERYENDSESSION` /
//!     `WM_ENDSESSION` — the only end-session notification a
//!     `windows_subsystem = "windows"` process receives. Without a window of any
//!     kind Windows cannot ask us to close, so it just kills the process while it
//!     is talking to the display / PawnIO driver. A hidden window that answers
//!     "yes, go ahead" immediately is what keeps the shutdown quiet.
//!
//! Both paths block (briefly, bounded) until the display loop reports that it
//! closed its handles, so cleanup actually happens before the OS pulls the rug.

use log::{debug, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// How long a shutdown notification waits for the display loop to clean up.
/// Windows allows ~5s before force-killing, so stay well inside that.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Signalled when shutdown is requested — lets [`sleep`] wake instantly instead
/// of holding the OS up for the rest of a polling period.
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Signalled by [`mark_finished`] once the hardware handles are closed.
static FINISHED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Has someone asked us to stop? Checked by the display loop and by the retry
/// paths, which must not stall a shutdown with reconnect attempts.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}

/// Register the shutdown notification handlers. Best-effort: on failure the app
/// still runs, it just goes back to being terminated abruptly.
pub fn install() {
    raise_shutdown_priority();
    install_console_handler();
    spawn_end_session_window();
}

/// Sleep, waking early if shutdown is requested.
pub fn sleep(duration: Duration) {
    if requested() {
        return;
    }
    let (lock, cvar) = &WAKE;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let _ = cvar.wait_timeout_while(guard, duration, |woken| !*woken);
}

/// Announce that the display loop has closed its handles and it is safe for the
/// OS to proceed.
pub fn mark_finished() {
    set(&FINISHED);
}

/// Ask the display loop to stop, and record why (for the log).
fn request(reason: &str) {
    if !REQUESTED.swap(true, Ordering::SeqCst) {
        info!("Shutdown requested ({reason}); winding down display loop");
    }
    set(&WAKE);
}

fn set(signal: &(Mutex<bool>, Condvar)) {
    let (lock, cvar) = signal;
    let mut flag = lock.lock().unwrap_or_else(|e| e.into_inner());
    *flag = true;
    cvar.notify_all();
}

/// Give the display loop a bounded window to close its handles. Called from the
/// notification handlers, whose return means "we are ready to die".
fn wait_for_cleanup() {
    let (lock, cvar) = &FINISHED;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    match cvar.wait_timeout_while(guard, CLEANUP_TIMEOUT, |done| !*done) {
        Ok((_, timeout)) if timeout.timed_out() => {
            warn!("Display loop did not finish within {CLEANUP_TIMEOUT:?}; exiting anyway");
        }
        _ => debug!("Display loop finished; ready for shutdown"),
    }
}

/// Ask to be notified before ordinary applications (higher level == earlier), so
/// we release the HID device and the PawnIO driver while they are still alive.
fn raise_shutdown_priority() {
    use windows::Win32::System::Threading::SetProcessShutdownParameters;
    // 0x280 is the default for applications; 0x300 puts us ahead of them but
    // still behind services (0x400+). SAFETY: plain scalar arguments.
    unsafe {
        let _ = SetProcessShutdownParameters(0x300, 0);
    }
}

// ---------------------------------------------------------------------------
// Console control events (debug builds / `cargo run`)
// ---------------------------------------------------------------------------

fn install_console_handler() {
    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    // SAFETY: `console_handler` matches PHANDLER_ROUTINE and lives for the
    // program's lifetime.
    unsafe {
        if SetConsoleCtrlHandler(Some(console_handler), true).is_err() {
            debug!("No console control handler installed (no console attached)");
        }
    }
}

unsafe extern "system" fn console_handler(event: u32) -> windows::core::BOOL {
    use windows::Win32::Foundation::{FALSE, TRUE};
    use windows::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };

    let reason = match event {
        CTRL_C_EVENT => "Ctrl+C",
        CTRL_BREAK_EVENT => "Ctrl+Break",
        CTRL_CLOSE_EVENT => "console closed",
        CTRL_LOGOFF_EVENT => "logoff",
        CTRL_SHUTDOWN_EVENT => "system shutdown",
        _ => return FALSE,
    };

    request(reason);
    // For close/logoff/shutdown the system terminates us once we return, so hold
    // it here until the loop has let go of the hardware.
    wait_for_cleanup();
    TRUE
}

// ---------------------------------------------------------------------------
// End-session window (release builds, launched from the scheduled task)
// ---------------------------------------------------------------------------

/// Create a hidden top-level window on its own thread and pump its messages.
/// It is never shown; it exists purely to receive the end-session broadcast,
/// which message-only (`HWND_MESSAGE`) windows do not get.
fn spawn_end_session_window() {
    let spawned = std::thread::Builder::new()
        .name("ch170-endsession".to_string())
        .spawn(run_end_session_window);

    if let Err(err) = spawned {
        warn!("Could not start end-session listener thread: {err}");
    }
}

/// Own the hidden window and pump its messages until the process exits. Must be
/// the only thing this thread does: a window belongs to the thread that created
/// it, and its messages are only delivered while that thread is in `GetMessage`.
fn run_end_session_window() {
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, WINDOW_EX_STYLE,
        WINDOW_STYLE, WNDCLASSW,
    };

    let class_name = WINDOW_CLASS;

    // SAFETY: `None` asks for this executable's own module handle.
    let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module.into(),
        Err(err) => {
            warn!("End-session listener disabled (GetModuleHandleW failed: {err})");
            return;
        }
    };

    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialized and outlives the call; the class name
    // is a static wide literal.
    if unsafe { RegisterClassW(&class) } == 0 {
        warn!("End-session listener disabled (RegisterClassW failed)");
        return;
    }

    // SAFETY: the class was just registered against this instance. No WS_VISIBLE
    // and zero size: the window is never drawn, it only receives messages — but
    // being top-level (not HWND_MESSAGE) is what gets it the end-session
    // broadcast, which message-only windows do not receive.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            class_name,
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    };
    if let Err(err) = window {
        warn!("End-session listener disabled (CreateWindowExW failed: {err})");
        return;
    }
    debug!("End-session listener ready");

    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, owned message buffer for the duration of the loop.
    // GetMessageW returns 0 on WM_QUIT and -1 on error; stop on either.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: dispatching a message this thread just retrieved.
        unsafe { DispatchMessageW(&msg) };
    }
}

unsafe extern "system" fn window_proc(
    window: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, PostQuitMessage, WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION,
    };

    match message {
        // "May Windows end the session?" — always yes, immediately. Returning 0
        // here (or not answering) is what makes Windows put up the
        // "this app is preventing you from shutting down" screen.
        WM_QUERYENDSESSION => {
            request(end_session_reason(lparam.0 as u32));
            LRESULT(1)
        }
        // The session really is ending. Hold Windows here (bounded) while the
        // display loop closes the HID device and the PawnIO handles.
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                request(end_session_reason(lparam.0 as u32));
                wait_for_cleanup();
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            request("window closed");
            wait_for_cleanup();
            // SAFETY: posts WM_QUIT to this thread's own message queue.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forwarding the arguments Windows handed us, unmodified.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// The hidden window's class name — also how the test finds it.
const WINDOW_CLASS: windows::core::PCWSTR = windows::core::w!("DeepCoolCH170EndSession");

/// Describe the `lParam` flags of an end-session message for the log.
fn end_session_reason(flags: u32) -> &'static str {
    use windows::Win32::UI::WindowsAndMessaging::{
        ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF,
    };

    if flags & ENDSESSION_CRITICAL != 0 {
        "critical shutdown"
    } else if flags & ENDSESSION_LOGOFF != 0 {
        "logoff"
    } else if flags & ENDSESSION_CLOSEAPP != 0 {
        "application restart"
    } else {
        "system shutdown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use windows::Win32::Foundation::{HWND, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SendMessageW, WM_ENDSESSION, WM_QUERYENDSESSION,
    };

    /// Spin until the listener thread has created its window.
    fn find_window() -> HWND {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // SAFETY: static class name, no window-name filter.
            if let Ok(hwnd) = unsafe { FindWindowW(WINDOW_CLASS, windows::core::PCWSTR::null()) } {
                return hwnd;
            }
            assert!(Instant::now() < deadline, "end-session window never appeared");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// The whole point of the hidden window: Windows asks whether it may end the
    /// session, we answer "yes" without stalling, and the loop learns to stop.
    #[test]
    fn query_end_session_is_answered_and_stops_the_loop() {
        install();
        let hwnd = find_window();

        assert!(!requested(), "shutdown must not be requested before the message");

        let started = Instant::now();
        // SAFETY: sending to a window this process owns.
        let answer = unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, None, None) };
        let elapsed = started.elapsed();

        assert_eq!(answer.0, 1, "must not veto the shutdown");
        assert!(elapsed < Duration::from_secs(1), "answered slowly: {elapsed:?}");
        assert!(requested(), "the display loop should have been asked to stop");

        // A pending shutdown must cut the polling sleep short, not hold the OS up.
        let started = Instant::now();
        sleep(Duration::from_secs(30));
        assert!(started.elapsed() < Duration::from_secs(1), "sleep did not wake early");

        // WM_ENDSESSION must hold Windows while the loop closes its handles, then
        // return as soon as it reports it is done. Sent from another thread so the
        // listener thread is the one that blocks.
        let hwnd_bits = hwnd.0 as usize;
        let sender = std::thread::spawn(move || {
            let hwnd = HWND(hwnd_bits as *mut _);
            let started = Instant::now();
            // SAFETY: sending to a window this process owns; wParam=TRUE means the
            // session really is ending.
            unsafe { SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(1)), None) };
            started.elapsed()
        });

        std::thread::sleep(Duration::from_millis(300));
        assert!(!sender.is_finished(), "WM_ENDSESSION returned before cleanup finished");

        mark_finished();
        let blocked_for = sender.join().expect("sender thread panicked");
        assert!(
            blocked_for < CLEANUP_TIMEOUT,
            "WM_ENDSESSION waited out the full timeout ({blocked_for:?}) instead of \
             returning when cleanup finished"
        );
    }
}
