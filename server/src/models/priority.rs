use std::fmt::Display;
use serde::{self, Deserialize, Serialize};

#[derive(Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
  Synced,
  Plain
}

impl Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Priority::Synced => write!(f, "synced"),
            Priority::Plain => write!(f, "plain"),
        }
    }
}
