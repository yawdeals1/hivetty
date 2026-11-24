//! Startup timing. `HIVETTY_DEBUG_STARTUP=1` prints how long each startup stage took, measured
//! from process creation, to stderr.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

static T0: OnceLock<Instant> = OnceLock::new();
static ON: OnceLock<bool> = OnceLock::new();

fn enabled() -> bool {
    *ON.get_or_init(|| std::env::var_os("HIVETTY_DEBUG_STARTUP").is_some())
}

/// Time since the process was created (falls back to the first call).
pub fn since_start() -> Duration {
    let t0 = *T0.get_or_init(|| Instant::now().checked_sub(process_age()).unwrap_or_else(Instant::now));
    t0.elapsed()
}

pub fn mark(stage: &str) {
    if enabled() {
        eprintln!("[startup] {:>6.1} ms  {stage}", since_start().as_secs_f64() * 1000.0);
    }
}

fn process_age() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exit, mut kernel, mut user, mut now) = (z, z, z, z, z);
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return Duration::ZERO;
    }
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    Duration::from_nanos(ticks(now).saturating_sub(ticks(created)) * 100)
}
