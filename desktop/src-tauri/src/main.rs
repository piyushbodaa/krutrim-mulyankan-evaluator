#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// WebKitGTK's DMA-BUF renderer draws a blank window on Hyprland (Omarchy) under Wayland.
/// Set only when the user has not chosen a value, so a later workaround can still override it.
#[cfg(target_os = "linux")]
fn linux_webview_env() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        || ["XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "DESKTOP_SESSION"]
            .iter()
            .any(|key| {
                std::env::var(key)
                    .map(|value| value.to_ascii_lowercase().contains("hyprland"))
                    .unwrap_or(false)
            });
    if hyprland && std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    linux_webview_env();
    krutrim_mulyankan_desktop::run();
}
