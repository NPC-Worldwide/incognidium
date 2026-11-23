use crate::persistence::{load_user_data, save_user_data, HistoryEntry};

pub fn load_history() -> Vec<HistoryEntry> {
    load_user_data().map(|d| d.history).unwrap_or_default()
}

pub fn save_history(history: &[HistoryEntry]) -> std::io::Result<()> {
    let mut data = load_user_data().unwrap_or_default();
    data.history = history.to_vec();
    save_user_data(&data)
}
