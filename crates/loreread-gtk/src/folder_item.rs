//! GObject model for a sidebar folder entry.
//!
//! Each `FolderItem` represents a row in the folder sidebar:
//! an account header, a mail folder (Inbox, Sent, …), or a
//! saved view (stored query).

mod imp {
    use glib::prelude::*;
    use glib::subclass::prelude::*;
    use glib::Properties;
    use std::cell::{Cell, RefCell};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::FolderItem)]
    pub struct FolderItemInner {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        icon_name: RefCell<String>,
        #[property(get, set)]
        count: Cell<u32>,
        /// Account headers are non-selectable visual separators.
        #[property(get, set)]
        is_header: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FolderItemInner {
        const NAME: &'static str = "LorereadFolderItem";
        type Type = super::FolderItem;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FolderItemInner {}
}

glib::wrapper! {
    pub struct FolderItem(ObjectSubclass<imp::FolderItemInner>);
}

impl FolderItem {
    /// Create a mail folder row.
    pub fn new(name: &str, icon_name: &str, count: u32) -> Self {
        glib::Object::builder()
            .property("name", name)
            .property("icon-name", icon_name)
            .property("count", count)
            .property("is-header", false)
            .build()
    }

    /// Create an account header row (non-selectable).
    pub fn header(name: &str) -> Self {
        glib::Object::builder()
            .property("name", name)
            .property("icon-name", "network-workgroup")
            .property("count", 0)
            .property("is-header", true)
            .build()
    }
}