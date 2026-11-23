use crate::persistence::{load_user_data, save_user_data, Bookmark};

pub fn load_bookmarks() -> Vec<Bookmark> {
    load_user_data().map(|d| d.bookmarks).unwrap_or_default()
}

pub fn save_bookmarks(bookmarks: &[Bookmark]) -> std::io::Result<()> {
    let mut data = load_user_data().unwrap_or_default();
    data.bookmarks = bookmarks.to_vec();
    save_user_data(&data)
}
