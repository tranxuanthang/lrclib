CREATE INDEX idx_tracks_metadata_full ON tracks (name_lower, artist_name_lower, album_name_lower, duration);
CREATE INDEX idx_tracks_metadata_no_album ON tracks (name_lower, artist_name_lower, duration);
