use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Button, glib};

const APP_ID: &str = "org.gtk_rs.HelloWorld1";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(build_ui);

    app.run()
}

fn set_ui_scale(w: &ApplicationWindow, scale: i32) {
    let settings = w.settings();
    let dpi = settings.gtk_xft_dpi();
    settings.set_gtk_xft_dpi(scale * dpi);
}

fn build_ui(app: &Application) {
    let button = Button::builder()
        .label("press me!")
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    button.connect_clicked(|btn| {
        btn.set_label("hello, world");
    });

    let window = ApplicationWindow::builder()
        .application(app)
        .title("my GTK app")
        .child(&button)
        .build();

    set_ui_scale(&window, 2);

    window.present()
}
