//! Desktop notifications on track change (optional).

use std::sync::Arc;

use muzeec_cli_core::ipc::Event;

use crate::daemon::Daemon;

/// Spawn a thread that subscribes to daemon events and shows notifications.
pub fn spawn(daemon: Arc<Daemon>) {
    if !daemon.config.integrations.notifications {
        return;
    }
    let rx = daemon.bus.subscribe();
    let want_art = daemon.config.integrations.notify_art;
    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || {
            let mut last: Option<std::time::Instant> = None;
            while let Ok(event) = rx.recv() {
                if event.event != "track_changed" {
                    continue;
                }
                // Coalesce rapid skips.
                if let Some(t) = last
                    && t.elapsed() < std::time::Duration::from_millis(750)
                {
                    continue;
                }
                last = Some(std::time::Instant::now());

                let title = event
                    .data
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let artist = event
                    .data
                    .get("artist")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let album = event
                    .data
                    .get("album")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut notification = notify_rust::Notification::new();
                notification
                    .summary(&format!("{artist} — {title}"))
                    .body(album)
                    .appname("muzeec-cli")
                    .icon("muzeec-cli");
                if want_art && let Some(art) = event.data.get("art").and_then(|v| v.as_str()) {
                    notification.hint(notify_rust::Hint::ImagePath(art.to_string()));
                }
                if let Err(e) = notification.show() {
                    tracing::debug!(error = %e, "notification failed");
                }
            }
        })
        .ok();
    let _ = Event::new("noop", serde_json::Value::Null);
}
