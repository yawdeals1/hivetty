//! `hivetty setup-path --add|--remove <dir>`: the installer's way of putting Hivetty's `bin`
//! folder on the user's (or, with `--machine`, everyone's) PATH. Hidden from `--help`.

use anyhow::{Context, Result};
use hivetty_core::pathenv;
use winreg::enums::{RegType, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
use winreg::{RegKey, RegValue};

const USER_KEY: &str = "Environment";
const MACHINE_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

pub fn run(dir: &str, add: bool, machine: bool) -> Result<()> {
    let (root, sub) = if machine { (RegKey::predef(HKEY_LOCAL_MACHINE), MACHINE_KEY) } else { (RegKey::predef(HKEY_CURRENT_USER), USER_KEY) };
    let key = root.open_subkey_with_flags(sub, KEY_READ | KEY_WRITE).with_context(|| format!("opening {sub}"))?;
    // Keep the raw (unexpanded) value so %VARS% in other entries survive.
    let (current, kind) = match key.get_raw_value("Path") {
        Ok(v) => (decode(&v.bytes), v.vtype),
        Err(_) => (String::new(), RegType::REG_EXPAND_SZ),
    };
    let next = if add { pathenv::add(&current, dir) } else { pathenv::remove(&current, dir) };
    let Some(next) = next else {
        println!("PATH already {}", if add { "contains it" } else { "doesn't contain it" });
        return Ok(());
    };
    let kind = if matches!(kind, RegType::REG_SZ | RegType::REG_EXPAND_SZ) { kind } else { RegType::REG_EXPAND_SZ };
    key.set_raw_value("Path", &RegValue { bytes: encode(&next).into(), vtype: kind }).context("writing PATH")?;
    broadcast();
    println!("{} {dir} {} PATH", if add { "Added" } else { "Removed" }, if add { "to" } else { "from" });
    Ok(())
}

fn decode(bytes: &[u8]) -> String {
    let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

fn encode(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
}

/// Tell Explorer (and so newly started programs) that the environment changed.
fn broadcast() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
    let name: Vec<u16> = "Environment".encode_utf16().chain([0]).collect();
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, name.as_ptr() as isize, SMTO_ABORTIFHUNG, 2000, &mut result);
    }
}
