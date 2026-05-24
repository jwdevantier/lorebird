use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Button, Label, Orientation};

// ── Guile Scheme FFI ──────────────────────────────────────────────────

mod guile {
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    #[link(name = "guile-3.0")]
    extern "C" {
        /// Bootstrap the Guile interpreter. Must be called once.
        pub fn scm_init_guile();

        /// Evaluate a null-terminated C string as Scheme source, return a SCM value.
        pub fn scm_c_eval_string(expr: *const c_char) -> *const c_char;

        /// Convert a Scheme string SCM value to a heap-allocated C string (caller frees with libc::free).
        pub fn scm_to_locale_string(scm: *const c_char) -> *mut c_char;
    }

    /// Evaluate a Scheme expression and get the result as a Rust String.
    pub fn eval(expr: &str) -> Result<String, String> {
        let c_expr = CString::new(expr).map_err(|e| format!("nul byte in expr: {e}"))?;
        let scm = unsafe { scm_c_eval_string(c_expr.as_ptr()) };

        if scm.is_null() {
            return Err("Guile evaluation returned null".into());
        }

        let c_result = unsafe { scm_to_locale_string(scm) };
        if c_result.is_null() {
            return Err("scm_to_locale_string returned null".into());
        }

        let rust_string = unsafe { CStr::from_ptr(c_result) }
            .to_string_lossy()
            .into_owned();

        // Free the C string that Guile allocated for us.
        unsafe { libc::free(c_result as *mut _) };

        Ok(rust_string)
    }
}

// ── Main ──────────────────────────────────────────────────────────────

fn main() {
    // Bootstrap Guile before GTK touches anything.
    unsafe { guile::scm_init_guile() };

    let app = Application::builder()
        .application_id("org.loreread.app")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    // ── widgets ────────────────────────────────────────────────────
    let vbox = Box::new(Orientation::Vertical, 10);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(30);
    vbox.set_margin_end(30);

    let button = Button::with_label("Click me → run Scheme!");
    let label = Label::new(Some(""));
    label.set_margin_top(10);

    vbox.append(&button);
    vbox.append(&label);

    // ── signal ─────────────────────────────────────────────────────
    let label_clone = label.clone();
    button.connect_clicked(move |_| {
        // The Scheme expression we evaluate.
        let expr = r#"(string-append
  "Hello from Guile Scheme!  Random int: "
  (number->string (random 1000)))"#;

        match guile::eval(expr) {
            Ok(s) => label_clone.set_label(&s),
            Err(e) => label_clone.set_label(&format!("Guile error: {e}")),
        }
    });

    // ── window ─────────────────────────────────────────────────────
    let window = ApplicationWindow::builder()
        .application(app)
        .title("loreread — GTK + Guile + SQLite")
        .default_width(480)
        .default_height(200)
        .child(&vbox)
        .build();

    window.present();
}