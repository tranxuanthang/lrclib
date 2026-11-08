CREATE TABLE missing_tracks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT,
  name_lower TEXT,
  artist_name TEXT,
  artist_name_lower TEXT,
  album_name TEXT,
  album_name_lower TEXT,
  duration FLOAT,
  created_at DATETIME,
  updated_at DATETIME,
  UNIQUE(name_lower, artist_name_lower, album_name_lower, duration)
);

CREATE INDEX idx_missing_tracks_name_lower ON missing_tracks (name_lower);
CREATE INDEX idx_missing_tracks_artist_name_lower ON missing_tracks (artist_name_lower);
CREATE INDEX idx_missing_tracks_album_name_lower ON missing_tracks (album_name_lower);
CREATE INDEX idx_missing_tracks_duration ON missing_tracks (duration);
CREATE INDEX idx_missing_tracks_created_at ON missing_tracks (created_at);
