use std::sync::Arc;
use anyhow::Result;
use rusqlite::Connection;
use crate::providers::noop::NoopProvider;
use crate::repositories::{lyrics_repository, track_repository};
use crate::entities::missing_track::MissingTrack;
use crate::AppState;

#[derive(Debug)]
pub struct ScrapedData {
  pub plain_lyrics: Option<String>,
  pub synced_lyrics: Option<String>,
  pub instrumental: bool,
}

pub async fn start_queue(state: Arc<AppState>) {
  tokio::spawn(async move {
    let mut provider = NoopProvider::new();
    loop {
      let maybe_missing_track = {
        let mut queue_lock = state.queue.lock().await;
        queue_lock.pop_front()
      };

      if let Some(missing_track) = maybe_missing_track {
        let maybe_data = provider.retrieve_lyrics(
          &missing_track.name,
          &missing_track.artist_name,
          &missing_track.album_name,
          missing_track.duration,
        ).await;

        match maybe_data {
          Ok(data) => {
            let mut conn = state.pool.get().unwrap();
            process_lyrics_result(&missing_track, data, &mut conn).await;
          },
          Err(err) => tracing::error!(
            message = format!("Error while finding lyrics for {}!", missing_track.name),
            track_name = missing_track.name,
            artist_name = missing_track.artist_name,
            album_name = missing_track.album_name,
            duration = missing_track.duration,
            error = err.to_string(),
            queue = true,
          ),
        }
      } else {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
      }
    }
  });
}

async fn process_lyrics_result(missing_track: &MissingTrack, data: Option<ScrapedData>, conn: &mut Connection) {
  if let Some(data) = data {
    match add_found(missing_track, &data, conn).await {
      Ok(_) => tracing::info!(
        message = format!("Lyrics for {} is added!", missing_track.name),
        track_name = missing_track.name,
        artist_name = missing_track.artist_name,
        album_name = missing_track.album_name,
        duration = missing_track.duration,
        queue = true,
      ),
      Err(err) => tracing::error!(
        message = format!("Failed to save lyrics for {}!", missing_track.name),
        track_name = missing_track.name,
        artist_name = missing_track.artist_name,
        album_name = missing_track.album_name,
        duration = missing_track.duration,
        error = err.to_string(),
        queue = true,
      ),
    }
  } else {
    tracing::info!(
      message = format!("No lyrics found for {}!", missing_track.name),
      track_name = missing_track.name,
      artist_name = missing_track.artist_name,
      album_name = missing_track.album_name,
      duration = missing_track.duration,
      queue = true,
    );
  }
}

async fn add_found(missing_track: &MissingTrack, data: &ScrapedData, conn: &mut Connection) -> Result<()> {
  let mut tx = conn.transaction()?;

  let track_id = track_repository::add_one_tx(
    &missing_track.name.trim(),
    &missing_track.artist_name.trim(),
    &missing_track.album_name.trim(),
    missing_track.duration,
    &mut tx,
  )?;

  lyrics_repository::add_one_tx(
    &data.plain_lyrics,
    &data.synced_lyrics,
    track_id,
    data.instrumental,
    &None,
    &mut tx,
  )?;

  tx.commit()?;

  Ok(())
}
