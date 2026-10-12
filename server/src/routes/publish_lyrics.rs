use crate::{
    errors::ApiError,
    lyricsfile::{
        prepare_legacy_lyrics, prepare_stored_lyricsfile, LyricsfileTrackMetadata, PreparedLyrics,
    },
    repositories::{lyrics_repository, track_repository},
    utils::is_valid_publish_token,
    AppState,
};
use anyhow::Result;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use axum_macros::debug_handler;
use rusqlite::Connection;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequest {
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}

#[debug_handler]
pub async fn route(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PublishRequest>,
) -> Result<StatusCode, ApiError> {
    match headers.get("X-Publish-Token") {
        Some(publish_token) => {
            let is_valid =
                is_valid_publish_token(publish_token.to_str()?, &state.challenge_cache).await;

            if is_valid {
                let mut conn = state.pool.get()?;
                publish_lyrics(&payload, &mut conn)?;

                Ok(StatusCode::CREATED)
            } else {
                Err(ApiError::IncorrectPublishTokenError)
            }
        }
        None => Err(ApiError::IncorrectPublishTokenError),
    }
}

fn publish_lyrics(payload: &PublishRequest, conn: &mut Connection) -> Result<i64> {
    let prepared = prepare_lyrics(payload)?;
    let mut tx = conn.transaction()?;
    let duration = payload.duration.round();

    let existing_track = track_repository::get_track_id_by_metadata_tx(
        payload.track_name.trim(),
        payload.artist_name.trim(),
        payload.album_name.trim(),
        duration,
        &mut tx,
    )?;

    let track_id = match existing_track {
        Some(track_id) => track_id,
        None => track_repository::add_one_tx(
            payload.track_name.trim(),
            payload.artist_name.trim(),
            payload.album_name.trim(),
            duration,
            &mut tx,
        )?,
    };

    lyrics_repository::add_one_tx(
        &prepared.plain_lyrics,
        &prepared.synced_lyrics,
        &prepared.lyricsfile,
        track_id,
        prepared.instrumental,
        prepared.has_word_sync,
        &Some("lrclib".to_owned()),
        &mut tx,
    )?;

    tx.commit()?;

    Ok(track_id)
}

fn prepare_lyrics(payload: &PublishRequest) -> Result<PreparedLyrics> {
    if let Some(lyricsfile) = payload
        .lyricsfile
        .as_ref()
        .filter(|content| !content.trim().is_empty())
        .cloned()
    {
        return Ok(
            prepare_stored_lyricsfile(lyricsfile.clone()).unwrap_or_else(|error| {
                tracing::warn!(%error, "failed to derive legacy fields from published Lyricsfile");
                PreparedLyrics {
                    plain_lyrics: None,
                    synced_lyrics: None,
                    lyricsfile,
                    instrumental: false,
                    has_word_sync: false,
                }
            }),
        );
    }

    let metadata = LyricsfileTrackMetadata::new(
        &payload.track_name,
        &payload.album_name,
        &payload.artist_name,
        payload.duration,
    );
    prepare_legacy_lyrics(
        &metadata,
        payload.plain_lyrics.as_deref(),
        payload.synced_lyrics.as_deref(),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::migrate, lyricsfile::parse_lyricsfile};

    fn connection() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
    }

    fn payload() -> PublishRequest {
        PublishRequest {
            track_name: "Test Song".to_owned(),
            artist_name: "Test Artist".to_owned(),
            album_name: "Test Album".to_owned(),
            duration: 95.492,
            plain_lyrics: None,
            synced_lyrics: None,
            lyricsfile: None,
        }
    }

    #[test]
    fn legacy_publish_generates_and_persists_lyricsfile() {
        let mut connection = connection();
        let mut payload = payload();
        payload.synced_lyrics =
            Some("[ar:Test Artist]\n[00:03.250]Second\n[00:01.5]First".to_owned());
        payload.lyricsfile = Some(" \n".to_owned());

        publish_lyrics(&payload, &mut connection).unwrap();

        let row = connection
            .query_row(
                "SELECT plain_lyrics, synced_lyrics, lyricsfile, instrumental, has_lyricsfile, has_word_sync FROM lyrics",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                        row.get::<_, bool>(5)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0.as_deref(), Some("Second\nFirst"));
        assert_eq!(row.1, payload.synced_lyrics);
        assert!(!row.3);
        assert!(row.4);
        assert!(!row.5);

        let parsed = parse_lyricsfile(&row.2).unwrap();
        assert_eq!(parsed.plain_lyrics.as_deref(), Some("Second\nFirst"));
        assert_eq!(
            parsed.synced_lyrics.as_deref(),
            Some("[00:01.50]First\n[00:03.25]Second")
        );

        let duration_ms: i64 = serde_yaml::from_str::<serde_yaml::Value>(&row.2).unwrap()
            ["metadata"]["duration_ms"]
            .as_i64()
            .unwrap();
        assert_eq!(duration_ms, 95_492);
    }

    #[test]
    fn supplied_instrumental_lyricsfile_wins_and_is_stored_unchanged() {
        let mut connection = connection();
        let mut payload = payload();
        let lyricsfile = "version: '1.0'\nmetadata:\n  title: Test Song\n  artist: Test Artist\n  instrumental: true\nlines:\n  - text: ignored\n    start_ms: 1000\nplain: ignored\n";
        payload.plain_lyrics = Some("legacy plain".to_owned());
        payload.synced_lyrics = Some("[00:01.00]legacy synced".to_owned());
        payload.lyricsfile = Some(lyricsfile.to_owned());

        publish_lyrics(&payload, &mut connection).unwrap();

        let row = connection
            .query_row(
                "SELECT plain_lyrics, synced_lyrics, lyricsfile, instrumental, has_lyricsfile, has_word_sync FROM lyrics",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                        row.get::<_, bool>(5)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, None);
        assert_eq!(row.1, None);
        assert_eq!(row.2, lyricsfile);
        assert!(row.3);
        assert!(row.4);
        assert!(!row.5);
    }

    #[test]
    fn word_synced_lyricsfile_sets_has_word_sync() {
        let mut connection = connection();
        let mut payload = payload();
        let lyricsfile = "version: '1.0'\nmetadata:\n  title: Test Song\n  artist: Test Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n    words:\n      - text: 'Hello '\n        start_ms: 1000\n      - text: world\n        start_ms: 1500\nplain: |\n  Hello world\n";
        payload.lyricsfile = Some(lyricsfile.to_owned());

        publish_lyrics(&payload, &mut connection).unwrap();

        let (stored, has_word_sync): (String, bool) = connection
            .query_row("SELECT lyricsfile, has_word_sync FROM lyrics", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(stored, lyricsfile);
        assert!(has_word_sync);

        let parsed = parse_lyricsfile(&stored).unwrap();
        assert!(parsed.has_word_sync);
        assert_eq!(
            parsed.synced_lyrics.as_deref(),
            Some("[00:01.00]Hello world")
        );
    }
}
