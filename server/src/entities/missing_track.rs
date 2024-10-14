use serde::{Deserialize,Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MissingTrack {
  pub name: String,
  pub artist_name: String,
  pub album_name: String,
  pub duration: f64,
}

impl fmt::Display for MissingTrack {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "[{}|{}|{}|{}]", self.name, self.artist_name, self.album_name, self.duration)
  }
}

impl Hash for MissingTrack {
  fn hash<H: Hasher>(&self, state: &mut H) {
    self.name.hash(state);
    self.artist_name.hash(state);
    self.album_name.hash(state);
    let rounded_duration = self.duration.round() as i64;
    rounded_duration.hash(state);
  }
}

impl PartialEq for MissingTrack {
  fn eq(&self, other: &Self) -> bool {
    self.name == other.name
      && self.artist_name == other.artist_name
      && self.album_name == other.album_name
      && self.duration.round() as i64 == other.duration.round() as i64
  }
}

impl Eq for MissingTrack {}
