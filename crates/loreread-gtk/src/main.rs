mod folder_item;
mod thread_node;
mod window;

use gtk4::prelude::*;
use gtk4::Application;

fn main() {
    let app = Application::builder()
        .application_id("org.loreread.app")
        .build();

    app.connect_activate(|app| {
        window::build_window(app);
    });

    app.run();
}