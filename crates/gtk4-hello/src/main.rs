// Test: reproduces lorebird's tri-pane layout with SourceView
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Label, Paned, ListBox, Orientation,
    PolicyType, ScrolledWindow, SearchEntry, HeaderBar, Grid,
};
use sourceview5 as sv;
use sourceview5::prelude::*;

fn main() {
    let app = Application::builder()
        .application_id("com.example.Gtk4TestLorebirdLayout")
        .build();

    app.connect_activate(|app| {
        // Apply dark theme like lorebird
        if let Some(settings) = gtk4::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(true);
        }

        let window = ApplicationWindow::builder()
            .application(app)
            .title("GTK4 Test - Lorebird Layout")
            .default_width(1200)
            .default_height(700)
            .build();

        // Header bar like lorebird
        let header = HeaderBar::new();
        let title_label = Label::new(Some("lorebird-test"));
        title_label.add_css_class("title");
        header.set_title_widget(Some(&title_label));
        window.set_titlebar(Some(&header));

        // Main vertical box
        let main_vbox = Box::new(Orientation::Vertical, 0);

        // Tri-pane: sidebar | center | preview
        let outer_paned = Paned::new(Orientation::Horizontal);
        let inner_paned = Paned::new(Orientation::Horizontal);

        // Sidebar
        let sidebar_lb = ListBox::new();
        sidebar_lb.add_css_class("navigation-sidebar");
        for i in 0..10 {
            sidebar_lb.append(&Label::new(Some(&format!("Sidebar item {}", i))));
        }
        let sidebar_sw = ScrolledWindow::new();
        sidebar_sw.set_policy(PolicyType::Never, PolicyType::Automatic);
        sidebar_sw.set_min_content_width(180);
        sidebar_sw.set_child(Some(&sidebar_lb));

        outer_paned.set_start_child(Some(&sidebar_sw));
        outer_paned.set_shrink_start_child(false);

        // Center with search bar
        let center = Box::new(Orientation::Vertical, 0);
        let search = SearchEntry::new();
        search.set_hexpand(true);
        search.set_placeholder_text(Some("Search mail…"));
        search.set_margin_top(6);
        search.set_margin_bottom(6);
        search.set_margin_start(8);
        search.set_margin_end(8);
        center.append(&search);

        let center_content = Label::new(Some("Center pane - thread list would go here"));
        center_content.set_vexpand(true);
        center.append(&center_content);
        inner_paned.set_start_child(Some(&center));
        inner_paned.set_shrink_start_child(false);

        // Preview with SourceView
        let preview = Box::new(Orientation::Vertical, 0);
        preview.set_margin_top(8);
        preview.set_margin_bottom(8);
        preview.set_margin_start(12);
        preview.set_margin_end(12);

        let headers = Grid::new();
        headers.set_column_spacing(12);
        headers.set_row_spacing(4);
        headers.set_margin_bottom(8);
        let from_label = Label::new(Some("From: test@example.com"));
        from_label.set_xalign(0.0);
        headers.attach(&from_label, 0, 0, 2, 1);
        preview.append(&headers);

        let sep = gtk4::Separator::new(Orientation::Horizontal);
        preview.append(&sep);

        // SourceView for body preview
        let body_buffer = sv::Buffer::new(None::<&gtk4::TextTagTable>);
        body_buffer.set_highlight_syntax(true);
        let style_mgr = sv::StyleSchemeManager::default();
        if let Some(scheme) = style_mgr.scheme("Adwaita-dark") {
            body_buffer.set_style_scheme(Some(&scheme));
        }
        body_buffer.set_text("Email body preview would appear here.\n\nThis is a test of SourceView rendering.");

        let body_view = sv::View::with_buffer(&body_buffer);
        body_view.set_editable(false);
        body_view.set_cursor_visible(false);
        body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        body_view.set_left_margin(4);
        body_view.set_right_margin(4);
        body_view.set_top_margin(4);
        body_view.set_bottom_margin(4);
        body_view.set_show_line_numbers(true);
        body_view.set_monospace(true);

        let preview_sw = ScrolledWindow::new();
        preview_sw.set_vexpand(true);
        preview_sw.set_hexpand(true);
        preview_sw.set_policy(PolicyType::Automatic, PolicyType::Automatic);
        preview_sw.set_child(Some(&body_view));
        preview.append(&preview_sw);

        inner_paned.set_end_child(Some(&preview));
        inner_paned.set_shrink_end_child(false);
        inner_paned.set_position(550);

        outer_paned.set_end_child(Some(&inner_paned));
        outer_paned.set_position(180);
        outer_paned.set_vexpand(true);
        outer_paned.set_hexpand(true);

        let status_label = Label::new(Some("Ready — select a profile, then Refresh"));
        status_label.set_margin_start(8);
        status_label.set_margin_top(4);
        status_label.set_margin_bottom(4);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");

        main_vbox.append(&outer_paned);
        main_vbox.append(&status_label);
        window.set_child(Some(&main_vbox));

        window.present();
    });

    app.run();
}
