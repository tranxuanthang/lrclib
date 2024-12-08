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

pub async fn start_queue(workers_count: u8, state: Arc<AppState>) {
  // Do not start queue if the workers_count is equal or smaller than zero
  if workers_count <= 0 {
    return
  }

  for _ in 0..workers_count {
    let state_clone = Arc::clone(&state);

    tokio::spawn(async move {
      worker(state_clone).await;
    });
  }
}

async fn worker(state: Arc<AppState>) {
  let mut provider = NoopProvider::new();
  loop {
    let maybe_missing_track = get_next_track(&state).await;

    if let Some(missing_track) = maybe_missing_track {
      process_track(&state, &mut provider, missing_track).await;
    } else {
      tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
  }
}

async fn get_next_track(state: &Arc<AppState>) -> Option<MissingTrack> {
  state.queue.pop()
}

async fn process_track(state: &Arc<AppState>, provider: &mut NoopProvider, missing_track: MissingTrack) {
  let maybe_data = provider.retrieve_lyrics(
    &missing_track.name,
    &missing_track.artist_name,
    &missing_track.album_name,
    missing_track.duration,
  ).await;

  match maybe_data {
    Ok(data) => {
      process_lyrics_result(&missing_track, data, state).await;
    },
    Err(err) => {
      tracing::error!(
        message = format!("error while finding lyrics"),
        track_name = missing_track.name,
        artist_name = missing_track.artist_name,
        album_name = missing_track.album_name,
        duration = missing_track.duration,
        error = err.to_string(),
        queue = true,
      );

      // Push the track back to the queue
      let _ = state.queue.push(missing_track);
    },
  }
}

async fn process_lyrics_result(missing_track: &MissingTrack, data: Option<ScrapedData>, state: &Arc<AppState>) {
  let mut conn = state.pool.get().unwrap();
  let remaining_jobs = get_remaining_jobs(&state).await;

  if let Some(data) = data {
    match add_found(missing_track, &data, &mut conn).await {
      Ok(_) => tracing::info!(
        message = format!("added new lyrics"),
        track_name = missing_track.name,
        artist_name = missing_track.artist_name,
        album_name = missing_track.album_name,
        duration = missing_track.duration,
        remaining_jobs = remaining_jobs,
        queue = true,
      ),
      Err(err) => tracing::error!(
        message = format!("failed to save lyrics"),
        track_name = missing_track.name,
        artist_name = missing_track.artist_name,
        album_name = missing_track.album_name,
        duration = missing_track.duration,
        remaining_jobs = remaining_jobs,
        error = err.to_string(),
        queue = true,
      ),
    }
  } else {
    tracing::debug!(
      message = format!("no lyrics found"),
      track_name = missing_track.name,
      artist_name = missing_track.artist_name,
      album_name = missing_track.album_name,
      duration = missing_track.duration,
      remaining_jobs = remaining_jobs,
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

async fn get_remaining_jobs(state: &Arc<AppState>) -> usize {
  state.queue.len()
}
