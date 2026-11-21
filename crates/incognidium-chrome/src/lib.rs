//! UI services crate for the Incognidium browser shell.

pub mod about;
pub mod bookmarks;
pub mod history;
pub mod persistence;
pub mod settings;

// Stubs for modules to be filled later as code migrates from the shell.
pub mod menu {
    //! Application/window menu construction (stub).
    pub struct MenuBuilder;
    impl MenuBuilder {
        pub fn new() -> Self {
            Self
        }
        pub fn build(&self) {}
    }
}

pub mod toolbar {
    //! Navigation toolbar widgets (stub).
    pub struct Toolbar;
    impl Toolbar {
        pub fn new() -> Self {
            Self
        }
        pub fn render(&self) {}
    }
}

pub mod find {
    //! Find-in-page / search bar (stub).
    pub struct FindBar {
        query: String,
    }
    impl FindBar {
        pub fn new() -> Self {
            Self {
                query: String::new(),
            }
        }
        pub fn query(&self) -> &str {
            &self.query
        }
    }
}

pub mod keyboard {
    //! Keyboard shortcut definitions (stub).
    pub struct Shortcuts;
    impl Shortcuts {
        pub fn new() -> Self {
            Self
        }
    }
}

pub use about::{
    build_about_page, build_bookmarks_page, build_history_page, build_home_page, build_local_page,
    build_settings_page, build_source_page,
};
pub use bookmarks::{load_bookmarks, save_bookmarks};
pub use history::{load_history, save_history};
pub use persistence::{load_user_data, save_user_data, Bookmark, HistoryEntry, Settings, UserData};
pub use settings::{load_settings, save_settings};
