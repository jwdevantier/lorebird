//! GObject model for a thread node in the mail list.
//!
//! Each `ThreadNode` represents a single message visible in the
//! `ColumnView` tree. Child messages (replies) are stored in a
//! lazily-created `gio::ListStore` so that `TreeListModel` can
//! discover them.

mod imp {
    use gio::ListStore;
    use glib::prelude::*;
    use glib::subclass::prelude::*;
    use glib::Properties;
    use std::cell::{Cell, OnceCell, RefCell};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::ThreadNode)]
    pub struct ThreadNodeInner {
        #[property(get, set)]
        subject: RefCell<String>,
        #[property(get, set)]
        sender: RefCell<String>,
        #[property(get, set)]
        date: RefCell<String>,
        #[property(get, set)]
        has_children: Cell<bool>,
        #[property(get, set)]
        body_preview: RefCell<String>,
        /// Children list — not a GObject property, only used by
        /// `TreeListModel` to discover child rows.
        pub children: OnceCell<ListStore>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThreadNodeInner {
        const NAME: &'static str = "LorereadThreadNode";
        type Type = super::ThreadNode;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ThreadNodeInner {}
}

glib::wrapper! {
    pub struct ThreadNode(ObjectSubclass<imp::ThreadNodeInner>);
}

impl ThreadNode {
    /// Create a new thread node with the given display fields.
    pub fn new(subject: &str, from_addr: &str, date: &str) -> Self {
        glib::Object::builder()
            .property("subject", subject)
            .property("sender", from_addr)
            .property("date", date)
            .property("has-children", false)
            .property("body-preview", String::new())
            .build()
    }

    /// Return the `ListStore` of child nodes, creating it on first access.
    pub fn children_store(&self) -> &gio::ListStore {
        use glib::subclass::types::ObjectSubclassIsExt;
        self.imp().children.get_or_init(gio::ListStore::new::<ThreadNode>)
    }

    /// Append a child node and mark this node as having children.
    pub fn add_child(&self, child: &ThreadNode) {
        self.children_store().append(child);
        self.set_has_children(true);
    }
}