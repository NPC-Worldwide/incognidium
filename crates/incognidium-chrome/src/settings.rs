use crate::persistence::{load_user_data, save_user_data, Settings};

pub fn load_settings() -> Settings {
    load_user_data().map(|d| d.settings).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let mut data = load_user_data().unwrap_or_default();
    data.settings = settings.clone();
    save_user_data(&data)
}
