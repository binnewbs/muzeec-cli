//! MPRIS2 desktop integration via `mpris-server`.

use std::future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mpris_server::{LoopStatus, Metadata, PlaybackStatus, Player, Time, TrackId};
use serde_json::Value;

use crate::daemon::Daemon;

/// Spawn the MPRIS server on a dedicated local-runtime thread.
pub fn spawn(daemon: Arc<Daemon>) {
    std::thread::Builder::new()
        .name("mpris".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!(error = %e, "cannot start MPRIS runtime");
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                if let Err(e) = run(daemon).await {
                    tracing::warn!(error = %e, "MPRIS unavailable");
                }
            });
        })
        .ok();
}

async fn run(daemon: Arc<Daemon>) -> Result<(), Box<dyn std::error::Error>> {
    let player = Rc::new(
        Player::builder("muzeec-cli")
            .identity("muzeec-cli")
            .desktop_entry("muzeec-cli")
            .can_play(true)
            .can_pause(true)
            .can_seek(true)
            .can_go_next(true)
            .can_go_previous(true)
            .can_control(true)
            .build()
            .await?,
    );

    {
        let d = daemon.clone();
        player.connect_play_pause(move |_| {
            let _ = d.dispatch("player.toggle", Value::Null);
        });
    }
    {
        let d = daemon.clone();
        player.connect_play(move |_| {
            let _ = d.dispatch("player.resume", Value::Null);
        });
    }
    {
        let d = daemon.clone();
        player.connect_pause(move |_| {
            let _ = d.dispatch("player.pause", Value::Null);
        });
    }
    {
        let d = daemon.clone();
        player.connect_stop(move |_| {
            let _ = d.dispatch("player.stop", Value::Null);
        });
    }
    {
        let d = daemon.clone();
        player.connect_next(move |_| {
            let _ = d.dispatch("player.next", Value::Null);
        });
    }
    {
        let d = daemon.clone();
        player.connect_previous(move |_| {
            let _ = d.dispatch("player.prev", Value::Null);
        });
    }
    {
        let d = daemon.clone();
        player.connect_seek(move |_, delta| {
            let secs = delta.as_micros() as f64 / 1_000_000.0;
            let _ = d.dispatch("player.seek", serde_json::json!({ "delta_secs": secs }));
        });
    }
    {
        let d = daemon.clone();
        player.connect_set_position(move |_, _track, position| {
            let ms = position.as_millis().max(0) as u64;
            let frame = ms * d.engine.sample_rate() as u64 / 1000;
            let _ = d.dispatch("player.seek", serde_json::json!({ "frame": frame }));
        });
    }
    {
        let d = daemon.clone();
        player.connect_set_loop_status(move |_, status| {
            let mode = match status {
                LoopStatus::None => "off",
                LoopStatus::Track => "one",
                LoopStatus::Playlist => "all",
            };
            let _ = d.dispatch("player.loop", serde_json::json!({ "mode": mode }));
        });
    }
    {
        let d = daemon.clone();
        player.connect_set_shuffle(move |_, shuffle| {
            let _ = d.dispatch("player.shuffle", serde_json::json!({ "value": shuffle }));
        });
    }
    {
        let d = daemon.clone();
        player.connect_set_volume(move |_, volume| {
            let value = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
            let _ = d.dispatch("player.volume", serde_json::json!({ "value": value }));
        });
    }

    // Periodic state push.
    {
        let p = player.clone();
        let d = daemon.clone();
        tokio::task::spawn_local(async move {
            let mut last_track: Option<i64> = None;
            loop {
                let snap = d.engine.snapshot();
                let status = match snap.state {
                    muzeec_cli_core::engine::PlaybackState::Playing => PlaybackStatus::Playing,
                    muzeec_cli_core::engine::PlaybackState::Paused => PlaybackStatus::Paused,
                    muzeec_cli_core::engine::PlaybackState::Stopped => PlaybackStatus::Stopped,
                };
                let _ = p.set_playback_status(status).await;
                p.set_position(Time::from_micros(snap.position_ms as i64 * 1000));
                if let Some(track) = &snap.track
                    && last_track != Some(track.track_id)
                {
                    last_track = Some(track.track_id);
                    let trackid =
                        TrackId::try_from(format!("/muzeec-cli/track/{}", track.track_id).as_str())
                            .unwrap_or(TrackId::NO_TRACK);
                    let mut builder = Metadata::builder()
                        .trackid(trackid)
                        .title(track.title.clone())
                        .album(track.album.clone())
                        .artist([track.artist.clone()])
                        .length(Time::from_millis(track.duration_ms as i64));
                    if let Some(hash) = &track.art_hash {
                        let path = d.art.thumb_path(hash, 480);
                        if path.exists() {
                            builder = builder.art_url(format!("file://{}", path.display()));
                        }
                    }
                    let _ = p.set_metadata(builder.build()).await;
                }
                let (loop_status, shuffle) = {
                    let q = d.queue.lock().unwrap();
                    let loop_status = match q.loop_mode() {
                        muzeec_cli_core::queue::LoopMode::Off => LoopStatus::None,
                        muzeec_cli_core::queue::LoopMode::One => LoopStatus::Track,
                        muzeec_cli_core::queue::LoopMode::All => LoopStatus::Playlist,
                    };
                    (loop_status, q.shuffle())
                };
                let _ = p.set_loop_status(loop_status).await;
                let _ = p.set_shuffle(shuffle).await;
                let _ = p.set_volume(snap.volume as f64 / 100.0).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }

    let runner = player.clone();
    tokio::task::spawn_local(async move {
        let _ = runner.run().await;
    });
    future::pending::<()>().await;
    Ok(())
}
