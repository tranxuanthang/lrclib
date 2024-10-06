CREATE TABLE flags (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  lyrics_id INTEGER,
  content TEXT,
  created_at DATETIME,
  FOREIGN KEY (lyrics_id) REFERENCES lyrics (id)
);
