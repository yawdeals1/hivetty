//! Windows-level attention: toast notifications, taskbar flash, and a taskbar overlay badge.

use hivetty_core::model::Notification;
use tauri::image::Image;
use tauri::{AppHandle, Manager, UserAttentionType};
use tauri_plugin_notification::NotificationExt;

pub fn toast(app: &AppHandle, n: &Notification, ws_name: &str, app_focused: bool) {
    let Some(win) = app.get_window("main") else { return };
    if app_focused {
        // The in-app ring/badge is enough when the user is looking at Hivetty.
        return;
    }
    let title = match &n.title {
        Some(t) => format!("{t} · {ws_name}"),
        None => ws_name.to_string(),
    };
    let body = n.body.clone();
    let app = app.clone();
    // Window APIs round-trip through the main thread; never make PTY reader threads wait on that.
    std::thread::spawn(move || {
        let _ = app.notification().builder().title(title).body(body).show();
        let _ = win.request_user_attention(Some(UserAttentionType::Informational));
    });
}

/// Blue dot overlay on the taskbar button while anything is unread.
pub fn set_badge(app: &AppHandle, on: bool) {
    let Some(win) = app.get_window("main") else { return };
    std::thread::spawn(move || {
        let _ = win.set_overlay_icon(on.then(badge_image));
    });
}

fn badge_image() -> Image<'static> {
    const N: u32 = 32;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let i = ((y * N + x) * 4) as usize;
            // white ring around a blue disc, anti-aliased edge
            let outer = (c + 0.5 - d).clamp(0.0, 1.0);
            if outer <= 0.0 {
                continue;
            }
            let inner = (c - 2.5 - d).clamp(0.0, 1.0);
            let (r, g, b) = (
                255.0 * (1.0 - inner) + 0x4a as f32 * inner,
                255.0 * (1.0 - inner) + 0xa8 as f32 * inner,
                255.0 * (1.0 - inner) + 0xff as f32 * inner,
            );
            rgba[i] = r as u8;
            rgba[i + 1] = g as u8;
            rgba[i + 2] = b as u8;
            rgba[i + 3] = (outer * 255.0) as u8;
        }
    }
    Image::new_owned(rgba, N, N)
}
