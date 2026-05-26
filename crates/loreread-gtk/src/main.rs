use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Label, Orientation};

fn main() {
    let app = Application::builder()
        .application_id("org.loreread.app")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    let vbox = Box::new(Orientation::Vertical, 10);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(30);
    vbox.set_margin_end(30);

    let label = Label::new(Some("loreread — Lua + GTK + SQLite"));
    label.set_margin_top(10);
    vbox.append(&label);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("loreread — Lua + GTK + SQLite")
        .default_width(480)
        .default_height(200)
        .child(&vbox)
        .build();

    window.present();
}
