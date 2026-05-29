mod app_state;
mod folder_item;
mod lua_thread;
mod thread_node;
mod window;

use gtk4::prelude::*;
use gtk4::Application;

use std::cell::RefCell;
use std::rc::Rc;

use app_state::AppState;

fn main() {
    let app = Application::builder()
        .application_id("org.loreread.app")
        .build();

    app.connect_activate(|app| {
        // Look for --config <path> on the command line
        let config_path = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--config")
            .map(|w| std::path::PathBuf::from(&w[1]));

        let state = Rc::new(RefCell::new(AppState::new(
            config_path.as_deref(),
        )));
        window::build_window(app, &state);
    });

    app.run();
}