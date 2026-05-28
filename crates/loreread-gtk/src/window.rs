//! Main application window — Thunderbird-style tri-pane layout.
//!
//! The sidebar is built from the loaded config: each profile appears
//! as a header with "All Mail" and its views underneath. Clicking a
//! row sets the active profile (and optionally the view query).

use std::cell::RefCell;
use std::rc::Rc;

use gio::ListStore;
use glib::Object;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, ColumnView, ColumnViewColumn, Grid, HeaderBar, IconSize,
    Image, Label, ListBoxRow, ListItem, Orientation, Paned, PolicyType, SearchEntry,
    ScrolledWindow, SignalListItemFactory, SingleSelection,
    TextBuffer, TextView, TreeExpander, TreeListModel, TreeListRow, WrapMode,
};

use crate::app_state::AppState;
use crate::folder_item::FolderItem;
use crate::thread_node::ThreadNode;

// ── Public entry point ─────────────────────────────────────────────

/// Build and present the main loreread window.
pub fn build_window(app: &Application, state: &Rc<RefCell<AppState>>) {
    let state_ref = state.borrow();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("loreread")
        .default_width(1200)
        .default_height(700)
        .build();

    // ── Header bar ────────────────────────────────────────────
    let header = HeaderBar::new();
    let title_label = Label::new(Some("loreread"));
    title_label.add_css_class("title");
    header.set_title_widget(Some(&title_label));

    let fetch_btn = gtk4::Button::with_label("Fetch");
    let index_btn = gtk4::Button::with_label("Index");

    // ── Status bar (created early so callbacks can clone it) ──
    let status_label = Label::new(Some("Ready \u{2014} select a profile, then click Index"));
    status_label.set_margin_start(8);
    status_label.set_margin_top(4);
    status_label.set_margin_bottom(4);
    status_label.add_css_class("dim-label");
    status_label.add_css_class("caption");

    // ── Wire Index button ─────────────────────────────────────
    let state_for_index = state.clone();
    let status_for_index = status_label.clone();
    index_btn.connect_clicked(move |_btn| {
        let s = state_for_index.borrow();
        match s.index_and_rebuild() {
            Ok(n) => {
                status_for_index.set_text(&format!(
                    "Indexed {} new messages, rebuilt thread tree",
                    n
                ));
            }
            Err(e) => {
                status_for_index.set_text(&format!("Error: {}", e));
            }
        }
    });

    // ── Wire Fetch button ─────────────────────────────────────
    let state_for_fetch = state.clone();
    let status_for_fetch = status_label.clone();
    fetch_btn.connect_clicked(move |_btn| {
        let s = state_for_fetch.borrow();
        match s.call_fetch_hook() {
            Ok(true) => {
                status_for_fetch.set_text("Fetch succeeded, indexing\u{2026}");
                match s.index_and_rebuild() {
                    Ok(n) => {
                        status_for_fetch.set_text(&format!(
                            "Fetched & indexed {} new messages",
                            n
                        ));
                    }
                    Err(e) => {
                        status_for_fetch.set_text(&format!("Indexing error: {}", e));
                    }
                }
            }
            Ok(false) => {
                status_for_fetch.set_text("Fetch hook returned false \u{2014} no new mail");
            }
            Err(e) => {
                status_for_fetch.set_text(&format!("Fetch error: {}", e));
            }
        }
    });

    header.pack_end(&index_btn);
    header.pack_end(&fetch_btn);
    window.set_titlebar(Some(&header));

    // ── Main vertical box: paned + status ──────────────────────
    let main_vbox = Box::new(Orientation::Vertical, 0);

    // ── Tri-pane: sidebar | center | preview ──────────────────
    let outer_paned = Paned::new(Orientation::Horizontal);
    let inner_paned = Paned::new(Orientation::Horizontal);

    // Sidebar (built from config)
    let (sidebar_scrolled, sidebar_model, sidebar_lb) = build_sidebar(&state_ref);

    outer_paned.set_start_child(Some(&sidebar_scrolled));
    outer_paned.set_shrink_start_child(false);

    // Center + preview
    let (center, selection, preview_labels) =
        build_center_pane(&state_ref.root_model);
    inner_paned.set_start_child(Some(&center));
    inner_paned.set_shrink_start_child(false);

    let preview = build_preview_pane(&preview_labels);
    inner_paned.set_end_child(Some(&preview));
    inner_paned.set_shrink_end_child(false);
    inner_paned.set_position(550);

    outer_paned.set_end_child(Some(&inner_paned));
    outer_paned.set_position(180);
    outer_paned.set_vexpand(true);
    outer_paned.set_hexpand(true);

    main_vbox.append(&outer_paned);
    main_vbox.append(&status_label);
    window.set_child(Some(&main_vbox));

    // ── Wire sidebar selection → profile selection ────────────
    let state_for_sidebar = state.clone();
    let status_for_sidebar = status_label;
    let model = sidebar_model; // moved into closure
    sidebar_lb.connect_row_selected(move |_lb, row| {
        let Some(row) = row else { return };
        let idx = row.index() as u32;
        // Retrieve the FolderItem from the sidebar model
        let item: Option<FolderItem> = model
            .item(idx)
            .and_downcast::<FolderItem>();
        let Some(item) = item else { return };

        let profile = item.profile_label();
        let query = item.query();
        let kind = item.row_kind();

        match kind.as_str() {
            "profile-header" => {
                let s = state_for_sidebar.borrow();
                s.select_profile(&profile);
                status_for_sidebar.set_text(&format!(
                    "Selected profile: {} \u{2014} click Index to load",
                    profile
                ));
            }
            "all-mail" => {
                let s = state_for_sidebar.borrow();
                s.select_profile(&profile);
                s.clear_view();
                status_for_sidebar.set_text(&format!("All mail for: {}", profile));
            }
            "view" => {
                let s = state_for_sidebar.borrow();
                s.select_profile(&profile);
                s.select_view(query.to_string());
                status_for_sidebar.set_text(&format!(
                    "View \u{2018}{}\u{2019} in: {}",
                    query, profile
                ));
            }
            _ => {}
        }
    });

    // ── Wire selection → preview ──────────────────────────────
    let pl = preview_labels;
    selection.connect_selection_changed(move |sel, _pos, _n| {
        if let Some(obj) = sel.selected_item()
            && let Some(row) = obj.downcast_ref::<TreeListRow>()
            && let Some(node) = row.item().and_downcast::<ThreadNode>()
        {
            pl.from_label.set_text(&node.sender());
            pl.subject_label.set_text(&node.subject());
            pl.date_label.set_text(&node.date());

            let body = node.body_preview();
            if body.is_empty() {
                pl.body_buffer.set_text("(no preview available)");
            } else {
                pl.body_buffer.set_text(&body);
            }
            return;
        }
        pl.from_label.set_text("");
        pl.subject_label.set_text("");
        pl.date_label.set_text("");
        pl.body_buffer.set_text("");
    });

    window.present();
}

// ── Sidebar ───────────────────────────────────────────────────────

/// Build the sidebar from the loaded config.
fn build_sidebar(state: &AppState) -> (ScrolledWindow, ListStore, gtk4::ListBox) {
    let scrolled = ScrolledWindow::new();
    scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
    scrolled.set_min_content_width(180);

    // Build the model of FolderItems
    let sidebar_model = ListStore::new::<FolderItem>();

    // Sort profiles alphabetically
    let mut profile_labels: Vec<String> = state.profiles.keys().cloned().collect();
    profile_labels.sort();

    for label in &profile_labels {
        let profile = &state.profiles[label];

        // Profile header
        sidebar_model.append(&FolderItem::profile_header(label));
        // All Mail
        sidebar_model.append(&FolderItem::all_mail(label));
        // Views
        for view in &profile.views {
            sidebar_model.append(&FolderItem::view(label, &view.label, &view.query));
        }
        // Separator (modelled as a disabled item)
        sidebar_model.append(&FolderItem::separator());
    }

    // If no profiles, show a helpful placeholder
    if profile_labels.is_empty() {
        sidebar_model.append(&FolderItem::placeholder(
            "No profiles configured.\n\n\
             Create ~/.config/loreread/config.lua\n\
             or start with --config <path>",
        ));
    }

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    list_box.add_css_class("navigation-sidebar");

    // Bind model → ListBox rows
    list_box.bind_model(
        Some(&sidebar_model),
        |item: &Object| -> gtk4::Widget {
            let folder_item = item.downcast_ref::<FolderItem>().unwrap();
            let row = make_sidebar_row(folder_item);
            row.upcast::<gtk4::Widget>()
        },
    );

    scrolled.set_child(Some(&list_box));
    (scrolled, sidebar_model, list_box)
}

/// Build a `ListBoxRow` widget for a `FolderItem`.
fn make_sidebar_row(item: &FolderItem) -> ListBoxRow {
    let hbox = Box::new(Orientation::Horizontal, 6);
    hbox.set_margin_top(4);
    hbox.set_margin_bottom(4);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);

    // Separators and placeholders are non-selectable
    let kind = item.row_kind();
    if kind == "separator" {
        let sep = gtk4::Separator::new(Orientation::Horizontal);
        hbox.append(&sep);
        let row = ListBoxRow::new();
        row.set_child(Some(&hbox));
        row.set_selectable(false);
        row.set_activatable(false);
        row.add_css_class("separator");
        return row;
    }

    if kind == "placeholder" {
        let label = Label::new(Some(&item.name()));
        label.set_justify(gtk4::Justification::Center);
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.set_wrap(true);
        hbox.append(&label);
        let row = ListBoxRow::new();
        row.set_child(Some(&hbox));
        row.set_selectable(false);
        row.set_activatable(false);
        return row;
    }

    // Normal row: icon + name
    let icon_name = item.icon_name();
    if !icon_name.is_empty() {
        let img = Image::from_icon_name(&icon_name);
        img.set_icon_size(IconSize::Normal);
        hbox.append(&img);
    }

    let label = Label::new(Some(&item.name()));
    label.set_hexpand(true);
    label.set_xalign(0.0);
    if kind == "profile-header" {
        label.add_css_class("heading");
        label.add_css_class("caption");
    }
    hbox.append(&label);

    let count = item.count();
    if count > 0 {
        let count_lbl = Label::new(Some(&count.to_string()));
        count_lbl.add_css_class("dim-label");
        count_lbl.add_css_class("caption");
        hbox.append(&count_lbl);
    }

    let row = ListBoxRow::new();
    row.set_child(Some(&hbox));

    if kind == "profile-header" {
        // Headers are selectable (they set the active profile)
    }

    row
}

// ── Center pane ───────────────────────────────────────────────────

/// Labels in the preview pane that need to be updated on selection change.
pub(crate) struct PreviewLabels {
    pub from_label: Label,
    pub subject_label: Label,
    pub date_label: Label,
    pub body_buffer: TextBuffer,
}

/// Build the centre pane, returning the root widget, the selection model
/// (for wiring to the preview), and the preview labels.
fn build_center_pane(root_model: &ListStore) -> (Box, SingleSelection, PreviewLabels) {
    let vbox = Box::new(Orientation::Vertical, 0);

    // ── Search bar ────────────────────────────────────────────
    let search = SearchEntry::new();
    search.set_hexpand(true);
    search.set_placeholder_text(Some("Search mail\u{2026}"));
    search.set_margin_top(6);
    search.set_margin_bottom(6);
    search.set_margin_start(8);
    search.set_margin_end(8);
    vbox.append(&search);

    // ── Thread list ──────────────────────────────────────────
    let (column_view, selection) = build_thread_list(root_model);

    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hexpand(true);
    scrolled.set_child(Some(&column_view));
    vbox.append(&scrolled);

    // ── Preview labels (updated on selection) ────────────────
    let from_label = Label::new(Some("no message selected"));
    from_label.set_xalign(0.0);
    let subject_label = Label::new(Some(""));
    subject_label.set_xalign(0.0);
    let date_label = Label::new(Some(""));
    date_label.set_xalign(0.0);
    let body_buffer = TextBuffer::new(None);
    body_buffer.set_text("Select a profile, then click Index to load messages.");

    let preview_labels = PreviewLabels {
        from_label,
        subject_label,
        date_label,
        body_buffer,
    };

    (vbox, selection, preview_labels)
}

// ── Thread list (ColumnView + TreeListModel) ──────────────────────

fn build_thread_list(root_model: &ListStore) -> (ColumnView, SingleSelection) {
    // ── TreeListModel ────────────────────────────────────────
    let tree_model = TreeListModel::new(
        root_model.clone(),
        false, // passthrough=false → items are TreeListRow
        false, // autoexpand — user must click to expand
        |item: &Object| -> Option<gio::ListModel> {
            let node = item.downcast_ref::<ThreadNode>()?;
            let children = node.children_store();
            if children.n_items() > 0 {
                Some(children.clone().upcast())
            } else {
                None
            }
        },
    );

    // ── Selection ───────────────────────────────────────────
    let selection = SingleSelection::new(Some(tree_model));
    selection.set_can_unselect(true);

    // ── Column view ─────────────────────────────────────────
    let column_view = ColumnView::new(Some(selection.clone()));
    column_view.set_vexpand(true);
    column_view.set_hexpand(true);

    // — Column: Subject (with tree expander) ─────────────────
    let subject_factory = SignalListItemFactory::new();
    subject_factory.connect_setup(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let expander = TreeExpander::new();
        let label = Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        expander.set_child(Some(&label));
        list_item.set_child(Some(&expander));
    });
    subject_factory.connect_bind(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let row = list_item
            .item()
            .and_downcast::<TreeListRow>()
            .unwrap();
        let expander = list_item
            .child()
            .and_downcast::<TreeExpander>()
            .unwrap();
        expander.set_list_row(Some(&row));
        if let Some(node) = row.item().and_downcast::<ThreadNode>()
            && let Some(label) = expander.child().and_downcast::<Label>()
        {
            label.set_label(&node.subject());
        }
    });

    let subject_col =
        ColumnViewColumn::new(Some("Subject"), Some(subject_factory));
    subject_col.set_expand(true);
    column_view.append_column(&subject_col);

    // — Column: From ──────────────────────────────────────────
    let from_factory = SignalListItemFactory::new();
    from_factory.connect_setup(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let label = Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_width_chars(18);
        label.add_css_class("dim-label");
        list_item.set_child(Some(&label));
    });
    from_factory.connect_bind(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let row = list_item
            .item()
            .and_downcast::<TreeListRow>()
            .unwrap();
        if let Some(node) = row.item().and_downcast::<ThreadNode>()
            && let Some(label) = list_item.child().and_downcast::<Label>()
        {
            label.set_label(&node.sender());
        }
    });

    let from_col = ColumnViewColumn::new(Some("From"), Some(from_factory));
    column_view.append_column(&from_col);

    // — Column: Date ──────────────────────────────────────────
    let date_factory = SignalListItemFactory::new();
    date_factory.connect_setup(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let label = Label::new(None);
        label.set_xalign(1.0);
        label.set_width_chars(8);
        label.add_css_class("dim-label");
        label.add_css_class("numeric");
        list_item.set_child(Some(&label));
    });
    date_factory.connect_bind(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let row = list_item
            .item()
            .and_downcast::<TreeListRow>()
            .unwrap();
        if let Some(node) = row.item().and_downcast::<ThreadNode>()
            && let Some(label) = list_item.child().and_downcast::<Label>()
        {
            label.set_label(&node.date());
        }
    });

    let date_col = ColumnViewColumn::new(Some("Date"), Some(date_factory));
    date_col.set_resizable(false);
    column_view.append_column(&date_col);

    (column_view, selection)
}

// ── Preview pane ──────────────────────────────────────────────────

fn build_preview_pane(labels: &PreviewLabels) -> Box {
    let vbox = Box::new(Orientation::Vertical, 0);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    // ── Headers ──────────────────────────────────────────────
    let headers = Grid::new();
    headers.set_column_spacing(12);
    headers.set_row_spacing(4);
    headers.set_margin_bottom(8);

    let add_row = |grid: &Grid, row: i32, key: &str, value: &Label| {
        let k = make_header_key(key);
        grid.attach(&k, 0, row, 1, 1);
        let v = value.clone();
        v.set_hexpand(true);
        v.set_xalign(0.0);
        v.set_selectable(true);
        grid.attach(&v, 1, row, 1, 1);
    };

    add_row(&headers, 0, "From", &labels.from_label);
    let to_label = Label::new(Some(""));
    to_label.set_xalign(0.0);
    add_row(&headers, 1, "To", &to_label);
    add_row(&headers, 2, "Subject", &labels.subject_label);
    add_row(&headers, 3, "Date", &labels.date_label);

    vbox.append(&headers);

    // ── Separator ────────────────────────────────────────────
    let sep = gtk4::Separator::new(Orientation::Horizontal);
    vbox.append(&sep);

    // ── Body ─────────────────────────────────────────────────
    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hexpand(true);
    scrolled.set_policy(PolicyType::Automatic, PolicyType::Automatic);

    let body_view = TextView::with_buffer(&labels.body_buffer);
    body_view.set_editable(false);
    body_view.set_cursor_visible(false);
    body_view.set_wrap_mode(WrapMode::WordChar);
    body_view.set_left_margin(4);
    body_view.set_right_margin(4);
    body_view.set_top_margin(4);
    body_view.set_bottom_margin(4);

    scrolled.set_child(Some(&body_view));
    vbox.append(&scrolled);

    vbox
}

/// Create a dim, right-aligned header key label (e.g. "From:").
fn make_header_key(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(1.0);
    label.add_css_class("dim-label");
    label
}