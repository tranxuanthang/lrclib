use crate::{
    entities::track::SimpleTrack,
    repositories::track_repository::{
        get_track_by_id, get_track_by_metadata, get_tracks_by_keyword,
    },
    utils::process_param,
    AppState,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, Json, ServerHandler,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackResponse {
    id: i64,
    name: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

impl From<SimpleTrack> for TrackResponse {
    fn from(track: SimpleTrack) -> Self {
        let (instrumental, plain_lyrics, synced_lyrics) = match track.last_lyrics {
            Some(lyrics) => (
                lyrics.instrumental,
                lyrics.plain_lyrics,
                lyrics.synced_lyrics,
            ),
            None => (false, None, None),
        };

        TrackResponse {
            id: track.id,
            name: track.name.clone(),
            track_name: track.name,
            artist_name: track.artist_name,
            album_name: track.album_name,
            duration: track.duration,
            instrumental,
            plain_lyrics,
            synced_lyrics,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct GetLyricsRequest {
    /// Song title.
    pub track_name: String,
    /// Performing artist.
    pub artist_name: String,
    /// Album title. Improves match accuracy when known.
    pub album_name: Option<String>,
    /// Track duration in seconds. Improves match accuracy when known.
    pub duration: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct GetLyricsByIdRequest {
    /// LRCLIB numeric track id, as returned by other lyrics tools.
    pub track_id: i64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SearchLyricsResponse {
    tracks: Vec<TrackResponse>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchLyricsRequest {
    /// Free-text search query, matched across track/artist/album fields.
    pub q: Option<String>,
    /// Restrict/boost results to this track name.
    pub track_name: Option<String>,
    /// Restrict/boost results to this artist name.
    pub artist_name: Option<String>,
    /// Restrict/boost results to this album name.
    pub album_name: Option<String>,
}

#[derive(Clone)]
pub struct LrclibMcpServer {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

impl LrclibMcpServer {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl LrclibMcpServer {
    #[tool(
        title = "Get lyrics by metadata",
        description = "Look up synced/plain lyrics for a track by track name, artist name, and optionally album name/duration. Returns a not-found error if there is no close match.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_lyrics(
        &self,
        Parameters(params): Parameters<GetLyricsRequest>,
    ) -> Result<Json<TrackResponse>, String> {
        let track_name_lower = process_param(Some(params.track_name.as_str()));
        let artist_name_lower = process_param(Some(params.artist_name.as_str()));
        let album_name_lower = process_param(params.album_name.as_deref());

        let (track_name_lower, artist_name_lower) = match (track_name_lower, artist_name_lower) {
            (Some(track_name_lower), Some(artist_name_lower)) => {
                (track_name_lower, artist_name_lower)
            }
            _ => return Err("track_name and artist_name must not be empty".to_owned()),
        };

        let mut conn = self.state.db_connection().map_err(|e| e.to_string())?;
        let track = get_track_by_metadata(
            &track_name_lower,
            &artist_name_lower,
            album_name_lower.as_deref(),
            params.duration,
            &mut conn,
        )
        .map_err(|e| e.to_string())?;

        match track {
            Some(track) => Ok(Json(track.into())),
            None => Err("No matching track found".to_owned()),
        }
    }

    #[tool(
        title = "Get lyrics by track id",
        description = "Look up lyrics for a track by its LRCLIB numeric track id.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_lyrics_by_id(
        &self,
        Parameters(params): Parameters<GetLyricsByIdRequest>,
    ) -> Result<Json<TrackResponse>, String> {
        let mut conn = self.state.db_connection().map_err(|e| e.to_string())?;
        let track = get_track_by_id(params.track_id, &mut conn).map_err(|e| e.to_string())?;

        match track {
            Some(track) => Ok(Json(track.into())),
            None => Err("No track found with that id".to_owned()),
        }
    }

    #[tool(
        title = "Search lyrics",
        description = "Search LRCLIB for tracks/lyrics by free-text query and/or track/artist/album name. Returns an empty list rather than an error when nothing matches.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_lyrics(
        &self,
        Parameters(params): Parameters<SearchLyricsRequest>,
    ) -> Result<Json<SearchLyricsResponse>, String> {
        let q = process_param(params.q.as_deref());
        let track_name = process_param(params.track_name.as_deref());
        let artist_name = process_param(params.artist_name.as_deref());
        let album_name = process_param(params.album_name.as_deref());

        let mut conn = self.state.db_connection().map_err(|e| e.to_string())?;
        let tracks = get_tracks_by_keyword(
            q.as_deref(),
            track_name.as_deref(),
            artist_name.as_deref(),
            album_name.as_deref(),
            &mut conn,
        )
        .map_err(|e| e.to_string())?;

        Ok(Json(SearchLyricsResponse {
            tracks: tracks.into_iter().map(TrackResponse::from).collect(),
        }))
    }
}

#[tool_handler]
impl ServerHandler for LrclibMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("lrclib", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Look up synchronized and plain lyrics from LRCLIB (lrclib.net). Use get_lyrics when \
                 you know the track and artist (add album/duration for a more precise match), \
                 get_lyrics_by_id when you already have a numeric LRCLIB track id, and search_lyrics \
                 for free-text discovery across multiple tracks.",
            )
    }
}
