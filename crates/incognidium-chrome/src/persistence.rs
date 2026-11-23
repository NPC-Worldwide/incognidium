use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

/// User-configurable browser settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub search_engine_url: String,
    pub homepage: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            search_engine_url: "https://duckduckgo.com/?q=%s".to_string(),
            homepage: "about:home".to_string(),
        }
    }
}

/// One history visit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visited_at: u64,
}

/// One bookmark.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub created_at: u64,
}

/// All persisted user data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserData {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
}

fn config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/incognidium")
}

pub fn load_user_data() -> io::Result<UserData> {
    let path = config_dir().join("userdata.json");
    if !path.exists() {
        return Ok(UserData::default());
    }
    let text = fs::read_to_string(&path)?;
    serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn save_user_data(data: &UserData) -> io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join("userdata.json");
    let text = serde_json::to_string_pretty(data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, text)
}
