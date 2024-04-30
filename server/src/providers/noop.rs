use anyhow::Result;
use crate::queue::ScrapedData;

pub struct NoopProvider {}

impl NoopProvider {
  pub fn new() -> Self {
    Self {}
  }

  pub async fn retrieve_lyrics(&mut self, _track_name: &str, _artist_name: &str, _album_name: &str, _duration: f64) -> Result<Option<ScrapedData>> {
    Ok(None)
  }
}
