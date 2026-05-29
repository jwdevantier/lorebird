//! Main application window — Thunderbird-style tri-pane layout.
//!
//! The sidebar is built from the loaded config: each profile appears
//! as a header with "All Mail" and its views underneath. Clicking a
//! row sets the active profile (and optionally the view query).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gio::ListStore;
use glib::Object;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, ColumnView, ColumnViewColumn, CustomSorter, Grid, HeaderBar, IconSize,
    Image, Label, ListBoxRow, ListItem, Ordering, Orientation, Paned, PolicyType, SearchEntry,
    ScrolledWindow, SignalListItemFactory, SingleSelection, SortListModel, Spinner, SortType,
    TreeExpander, TreeListModel, TreeListRow, WrapMode,
};
use sourceview5 as sv;
use sourceview5::prelude::*;

use crate::app_state::AppState;
use crate::folder_item::FolderItem;
use crate::thread_node::ThreadNode;

// ── Public entry point ─────────────────────────────────────────────

/// Build and present the main loreread window.
pub fn build_window(app: &Application, state: &Rc<RefCell<AppState>>) {
    let state_ref = state.borrow();

    // ── Apply theme (dark/light) ──────────────────────────────
    let is_dark = state_ref.theme == "dark";
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(is_dark);
    }

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

    let refresh_btn = gtk4::Button::from_icon_name("view-refresh");
    refresh_btn.set_tooltip_text(Some("Refresh mail"));

    // ── Spinner (shown during async fetch) ─────────────────────
    let spinner = Spinner::new();
    spinner.set_spinning(false);

    // ── Status bar (created early so callbacks can clone it) ──
    let status_label = Label::new(Some("Ready \u{2014} select a profile, then Refresh"));
    status_label.set_margin_start(8);
    status_label.set_margin_top(4);
    status_label.set_margin_bottom(4);
    status_label.add_css_class("dim-label");
    status_label.add_css_class("caption");

    // ── Wire Refresh button (async via Lua thread) ───────────────
    let state_for_refresh = state.clone();
    let status_for_refresh = status_label.clone();
    let spinner_for_refresh = spinner.clone();
    let refresh_btn_ref = refresh_btn.clone();
    refresh_btn.connect_clicked(move |_btn| {
        refresh_btn_ref.set_sensitive(false);
        let s = state_for_refresh.borrow();
        match s.request_fetch() {
            Ok(()) => {
                spinner_for_refresh.set_spinning(true);
                status_for_refresh.set_text("Refreshing\u{2026}");
                let state_poll = state_for_refresh.clone();
                let status_poll = status_for_refresh.clone();
                let spinner_poll = spinner_for_refresh.clone();
                let btn_poll = refresh_btn_ref.clone();
                glib::timeout_add_local(Duration::from_millis(100), move || {
                    let s = state_poll.borrow();
                    match s.poll_fetch_result() {
                        Some(result) => {
                            spinner_poll.set_spinning(false);
                            btn_poll.set_sensitive(true);
                            match s.handle_fetch_result(&result) {
                                Ok(msg) => status_poll.set_text(&msg),
                                Err(e) => status_poll.set_text(&format!("Refresh error: {}", e)),
                            }
                            glib::ControlFlow::Break
                        }
                        None => glib::ControlFlow::Continue,
                    }
                });
            }
            Err(e) => {
                refresh_btn_ref.set_sensitive(true);
                status_for_refresh.set_text(&format!("Refresh error: {}", e));
            }
        }
    });

    header.pack_end(&refresh_btn);
    header.pack_end(&spinner);
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
    let (center, selection, preview_labels, search_entry) =
        build_center_pane(&state_ref.root_model, is_dark);
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

    // ── Wire sidebar selection → profile + view/search ──────
    let state_for_sidebar = state.clone();
    let status_for_sidebar = status_label.clone();
    let search_for_sidebar = search_entry.clone();
    let model = sidebar_model;
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

        // Filter out non-interactive rows
        if kind == "separator" || kind == "placeholder" {
            return;
        }

        let s = state_for_sidebar.borrow();

        match kind.as_str() {
            "profile-header" => {
                s.select_profile(&profile);
                // Clear any active search
                search_for_sidebar.set_text("");
                status_for_sidebar.set_text(&format!(
                    "Selected profile: {} \u{2014} click Refresh to load",
                    profile
                ));
            }
            "all-mail" => {
                s.select_profile(&profile);

                // Open existing DB if available
                if s.db.borrow().is_none() {
                    let maildir = s.active_maildir.borrow().clone();
                    if !maildir.as_os_str().is_empty() {
                        let _ = s.open_db(&maildir);
                    }
                }
                if s.db.borrow().is_some() {
                    match s.show_all() {
                        Ok(()) => status_for_sidebar.set_text(&format!(
                            "All mail for: {}", profile
                        )),
                        Err(e) => status_for_sidebar.set_text(&format!("Error: {}", e)),
                    }
                } else {
                    status_for_sidebar.set_text(&format!(
                        "No index for {} \u{2014} click Refresh",
                        profile
                    ));
                }
                search_for_sidebar.set_text("");
            }
            "view" => {
                s.select_profile(&profile);

                // Open existing DB if available
                if s.db.borrow().is_none() {
                    let maildir = s.active_maildir.borrow().clone();
                    if !maildir.as_os_str().is_empty() {
                        let _ = s.open_db(&maildir);
                    }
                }
                if s.db.borrow().is_none() {
                    status_for_sidebar.set_text(&format!(
                        "No index for {} \u{2014} click Refresh",
                        profile
                    ));
                    return;
                }

                // Run the view's query
                s.select_view(query.to_string());
                search_for_sidebar.set_text(&query);
                match s.search(&query) {
                    Ok(n) => status_for_sidebar.set_text(&format!(
                        "View \u{2018}{}\u{2019} in: {} \u{2014} {} match(es)",
                        item.name(), profile, n
                    )),
                    Err(e) => status_for_sidebar.set_text(&format!("Search error: {}", e)),
                }
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
            pl.date_label.set_text(&node.last_reply());

            let body = node.body_preview();
            if body.is_empty() {
                set_body_with_highlight(&pl.body_buffer, "(no preview available)");
            } else {
                set_body_with_highlight(&pl.body_buffer, &body);
            }
            return;
        }
        pl.from_label.set_text("");
        pl.subject_label.set_text("");
        pl.date_label.set_text("");
        set_body_with_highlight(&pl.body_buffer, "");
    });

    // ── Wire search bar ──────────────────────────────────────────
    // Enter / activate → run search query
    let state_for_search = state.clone();
    let status_for_search = status_label.clone();
    search_entry.connect_activate(move |entry| {
        let query = entry.text().to_string();
        if query.is_empty() {
            // Empty query → show all
            let s = state_for_search.borrow();
            match s.show_all() {
                Ok(()) => {
                    status_for_search.set_text("Showing all threads");
                }
                Err(e) => {
                    status_for_search.set_text(&format!("Error: {}", e));
                }
            }
        } else {
            let s = state_for_search.borrow();
            match s.search(&query) {
                Ok(n) => {
                    status_for_search.set_text(&format!(
                        "Found {} matching message(s) in thread(s)",
                        n
                    ));
                }
                Err(e) => {
                    status_for_search.set_text(&format!("Search error: {}", e));
                }
            }
        }
    });

    // Escape / stop-search → clear search, show all
    let state_for_clear = state.clone();
    let status_for_clear = status_label;
    search_entry.connect_stop_search(move |entry| {
        entry.set_text("");
        let s = state_for_clear.borrow();
        match s.show_all() {
            Ok(()) => {
                status_for_clear.set_text("Showing all threads");
            }
            Err(e) => {
                status_for_clear.set_text(&format!("Error: {}", e));
            }
        }
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
    pub body_buffer: sv::Buffer,
}

/// Build the centre pane, returning the root widget, the selection model
/// (for wiring to the preview), and the preview labels.
fn build_center_pane(root_model: &ListStore, is_dark: bool) -> (Box, SingleSelection, PreviewLabels, SearchEntry) {
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
    let body_buffer = sv::Buffer::new(None::<&gtk4::TextTagTable>);
    body_buffer.set_highlight_syntax(true);
    // Sync SourceView style scheme with the app theme
    let style_mgr = sv::StyleSchemeManager::default();
    let scheme_name = if is_dark { "Adwaita-dark" } else { "kate" };
    let fallback_name = if is_dark { "oblivion" } else { "Adwaita" };
    if let Some(scheme) = style_mgr.scheme(scheme_name)
        .or_else(|| style_mgr.scheme(fallback_name))
    {
        body_buffer.set_style_scheme(Some(&scheme));
    }
    body_buffer.set_text("Select a profile, then click Refresh to load messages.");
    // No highlighting for the initial placeholder
    body_buffer.set_language(None);

    let preview_labels = PreviewLabels {
        from_label,
        subject_label,
        date_label,
        body_buffer,
    };

    (vbox, selection, preview_labels, search)
}

// ── Thread list (ColumnView + TreeListModel) ──────────────────────

fn build_thread_list(root_model: &ListStore) -> (ColumnView, SingleSelection) {
    // ── Column view (created first to get its composite sorter) ──
    //
    // GTK ColumnView sorting works like this:
    // 1. Each sortable column has a CustomSorter that compares items
    // 2. ColumnView.get_sorter() returns a composite sorter reflecting
    //    the column the user last clicked and direction
    // 3. That composite sorter drives a SortListModel, which keeps
    //    root-level items in sorted order
    // 4. TreeListModel wraps SortListModel for expansion
    let column_view = ColumnView::new(None::<SingleSelection>);
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

    // — Column: Started ──────────────────────────────────────
    let started_factory = SignalListItemFactory::new();
    started_factory.connect_setup(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let label = Label::new(None);
        label.set_xalign(1.0);
        label.set_width_chars(6);
        label.add_css_class("dim-label");
        label.add_css_class("numeric");
        list_item.set_child(Some(&label));
    });
    started_factory.connect_bind(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let row = list_item
            .item()
            .and_downcast::<TreeListRow>()
            .unwrap();
        if let Some(node) = row.item().and_downcast::<ThreadNode>()
            && let Some(label) = list_item.child().and_downcast::<Label>()
        {
            label.set_label(&node.started());
        }
    });

    // Sorters compare ThreadNode items — ColumnView unwraps
    // TreeListRow automatically before passing to sorters.
    let started_sorter = CustomSorter::new(|a, b| {
        let a_node = a.downcast_ref::<ThreadNode>().unwrap();
        let b_node = b.downcast_ref::<ThreadNode>().unwrap();
        match a_node.started_ts().cmp(&b_node.started_ts()) {
            std::cmp::Ordering::Less => Ordering::Smaller,
            std::cmp::Ordering::Equal => Ordering::Equal,
            std::cmp::Ordering::Greater => Ordering::Larger,
        }
    });

    let started_col =
        ColumnViewColumn::new(Some("Started"), Some(started_factory));
    started_col.set_sorter(Some(&started_sorter));
    started_col.set_resizable(false);
    column_view.append_column(&started_col);

    // — Column: Last Reply ─────────────────────────────────
    let last_reply_factory = SignalListItemFactory::new();
    last_reply_factory.connect_setup(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let label = Label::new(None);
        label.set_xalign(1.0);
        label.set_width_chars(10);
        label.add_css_class("dim-label");
        label.add_css_class("numeric");
        list_item.set_child(Some(&label));
    });
    last_reply_factory.connect_bind(|_, obj| {
        let list_item = obj.downcast_ref::<ListItem>().unwrap();
        let row = list_item
            .item()
            .and_downcast::<TreeListRow>()
            .unwrap();
        if let Some(node) = row.item().and_downcast::<ThreadNode>()
            && let Some(label) = list_item.child().and_downcast::<Label>()
        {
            label.set_label(&node.last_reply());
        }
    });

    let last_reply_sorter = CustomSorter::new(|a, b| {
        let a_node = a.downcast_ref::<ThreadNode>().unwrap();
        let b_node = b.downcast_ref::<ThreadNode>().unwrap();
        // Natural ascending order; SortType::Descending reverses to newest-first
        match a_node.last_reply_ts().cmp(&b_node.last_reply_ts()) {
            std::cmp::Ordering::Less => Ordering::Smaller,
            std::cmp::Ordering::Equal => Ordering::Equal,
            std::cmp::Ordering::Greater => Ordering::Larger,
        }
    });

    let last_reply_col =
        ColumnViewColumn::new(Some("Last Reply"), Some(last_reply_factory));
    last_reply_col.set_sorter(Some(&last_reply_sorter));
    last_reply_col.set_resizable(false);
    column_view.append_column(&last_reply_col);

    // ── Model pipeline: SortListModel → TreeListModel → Selection ──
    //
    // ColumnView.get_sorter() returns a composite sorter that tracks
    // which column the user clicked and in which direction.  We plug
    // it into a SortListModel so the root-level items stay sorted.
    let view_sorter = column_view.sorter().expect("ColumnView must have a sorter");
    let sorted_model = SortListModel::new(Some(root_model.clone()), Some(view_sorter));

    let tree_model = TreeListModel::new(
        sorted_model.upcast::<gio::ListModel>(),
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

    let selection = SingleSelection::new(Some(tree_model));
    selection.set_can_unselect(true);
    selection.set_autoselect(false);

    column_view.set_model(Some(&selection));

    // Default sort: Last Reply descending (newest first)
    column_view.sort_by_column(Some(&last_reply_col), SortType::Descending);

    (column_view, selection)
}
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

    let body_view = sv::View::with_buffer(&labels.body_buffer);
    body_view.set_editable(false);
    body_view.set_cursor_visible(false);
    body_view.set_wrap_mode(WrapMode::WordChar);
    body_view.set_left_margin(4);
    body_view.set_right_margin(4);
    body_view.set_top_margin(4);
    body_view.set_bottom_margin(4);
    body_view.set_show_line_numbers(true);
    body_view.set_monospace(true);

    scrolled.set_child(Some(&body_view));
    vbox.append(&scrolled);

    vbox
}

/// Create a dim, right-aligned header key label (e.g. "From:").
/// Detect the source language from message content.
///
/// Email bodies often have prose before the code/diff, so we scan
/// the full content rather than just the first few lines.
fn detect_language(content: &str) -> Option<sv::Language> {
    let lm = sv::LanguageManager::default();

    // — Diff: scan anywhere in the message —
    // Patches in emails are preceded by prose, so we search
    // the whole content for diff markers.
    let is_diff = content.lines().any(|line| {
        line.starts_with("diff --git")
            || line.starts_with("diff -r")
            || line.starts_with("--- a/")
            || line.starts_with("+++ b/")
            || line.starts_with("@@ ")
    });
    if is_diff {
        return lm.language("diff");
    }

    // — Rust: look for strong Rust indicators (avoid false positives
    //   from email prose like "use ..." or "struct ...") —
    //   Require at least two distinct Rust markers.
    let rust_markers = [
        "fn ", "pub fn ", "pub(crate) fn ", "pub(super) fn ",
        "impl ",
        "let mut ",
        "match ",
        "-> ",              // return arrow
        "::",
    ];
    let rust_hits: usize = content.lines()
        .take(50)
        .map(|line| {
            let trimmed = line.trim();
            // Skip obvious email lines (quotes, signatures)
            if trimmed.starts_with('>') || trimmed.starts_with("-- ") || trimmed.is_empty() {
                return 0;
            }
            let mut count = 0;
            for marker in &rust_markers {
                if trimmed.contains(marker) && !trimmed.starts_with("//") {
                    count += 1;
                    break;
                }
            }
            count
        })
        .sum();
    if rust_hits >= 2 {
        return lm.language("rust");
    }

    // — C: similar logic —
    let c_markers = ["#include", "void ", "typedef ", "#define ", "enum {"];
    let c_hits: usize = content.lines()
        .take(50)
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('>') || trimmed.starts_with("-- ") || trimmed.is_empty() {
                return 0;
            }
            for marker in &c_markers {
                if trimmed.starts_with(marker) || trimmed.contains(marker) {
                    return 1;
                }
            }
            0
        })
        .sum();
    if c_hits >= 2 {
        return lm.language("c");
    }

    // — Fallback: let GtkSourceView guess —
    lm.guess_language(None::<&std::path::Path>, None)
}

/// Set the buffer's language based on content and display the text.
fn set_body_with_highlight(buffer: &sv::Buffer, text: &str) {
    let lang = detect_language(text);
    buffer.set_language(lang.as_ref());
    buffer.set_text(text);
}

fn make_header_key(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(1.0);
    label.add_css_class("dim-label");
    label
}

