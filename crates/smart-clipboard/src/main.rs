//! smart-clipboard — the desktop window onto the clipboard contract.
//!
//! ```text
//!   smart-clipboard                       # the library window (connects on boot)
//!   smart-clipboard --palette             # …and the quick palette
//!   smart-clipboard --socket /run/x.sock  # point it at a specific service
//!   SMART_SERVICE_SOCKET=… smart-clipboard
//! ```
//!
//! ## What this binary is not
//!
//! It is not a daemon, not a backend, not a database, and not a clipboard
//! manager. It does not read the selection, it does not keep history, and it
//! writes nothing to disk. If it is not running, the clipboard still works;
//! the *history* is served by whatever backend the service hosts, and this
//! window only reflects it.
//!
//! ## The startup order, and why it is this order
//!
//! 1. **Parse the two flags that decide where the service is.** Everything
//!    else is the window's business.
//! 2. **Create the app entity**, which starts the protocol worker and the
//!    decoder immediately — before any window exists.
//! 3. **Open the library window.** Its view is what triggers the first
//!    `Connect`; the worker is already listening, so the connection and the
//!    first paint happen in parallel rather than in sequence.
//! 4. **Open the palette** if asked, which shares the same entity and
//!    therefore the same session.

mod app;
mod format;
mod preview;
mod query;
mod store;
mod theme;
mod ui;
mod views;

use gpui::{App, Application, WindowOptions};
use smart_clipboard_proto::{contract, ServiceLocation};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|argument| argument == "--help" || argument == "-h") {
        print_help();
        return;
    }
    if args
        .iter()
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("smart-clipboard {}", env!("CARGO_PKG_VERSION"));
        println!("contract schema tree: {}", contract::SCHEMA_DIR);
        println!(
            "clipboard schema id:  0x{:016x}",
            contract::EXPECTED_CLIPBOARD_SCHEMA_ID
        );
        return;
    }

    let want_palette = args.iter().any(|argument| argument == "--palette");
    let location = ServiceLocation::from_environment(&args);
    if location.candidates.is_empty() {
        // Should not happen (the transport default is always appended), but a
        // silent empty plan would look like a hang, so say it instead.
        eprintln!(
            "smart-clipboard: no endpoint to try. Pass --socket <path> or set \
             SMART_SERVICE_SOCKET."
        );
        std::process::exit(2);
    }

    Application::new().run(move |cx: &mut App| {
        let app = cx.new(|cx| app::ClipboardApp::new(location.clone(), cx));
        open_library(app.clone(), cx);
        if want_palette {
            app::open_palette(&app, cx);
        }
    });
}

/// Open the library window with the app entity as its root's shared state.
fn open_library(app: gpui::Entity<app::ClipboardApp>, cx: &mut App) {
    let options: WindowOptions = app::library_window_options("smart-clipboard");
    let result = cx.open_window(options, move |window, cx| {
        cx.new(|cx| views::library::LibraryView::new(app.clone(), window, cx))
    });
    if let Err(error) = result {
        eprintln!("smart-clipboard: cannot open the library window: {error}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "\
smart-clipboard — a window onto the smart-protocol clipboard contract

USAGE
  smart-clipboard [--palette] [--socket <path>]

OPTIONS
  --palette            also open the quick-palette window
  --socket <path>      the smart service's unix socket (highest priority)
  --addr <path>        the same, spelled the way smart-service spells it
  -h, --help           this text
  -V, --version        the build's contract identity

ENDPOINT RESOLUTION (in order)
  1. --socket / --addr
  2. $SMART_SERVICE_SOCKET
  3. $XDG_RUNTIME_DIR/smart/service.sock
  4. /tmp/smart-service.sock        (the unix transport's default)

  Every candidate is shown in the app with its outcome; nothing is guessed
  silently.

KEYS (library)
  type            search the service's history
  ↑ ↓             move the selection
  ⏎ / ⇧⏎         copy the item / offer all its representations
  P  G  S  T  R   pin · gallery · sort · theme · refresh
  ⌫               delete (with a confirmation)
  ⌘K / Ctrl+K     the quick palette
  esc             clear the search

WHAT THIS APP DOES NOT DO
  It does not read the clipboard, does not keep history, does not touch a
  database, and does not send paste keystrokes. It asks a service that serves
  the contract; that service owns the backend, the history and the store.

CONTRACT
  schema tree: {schema}
  clipboard:   0x{id:016x}
",
        schema = contract::SCHEMA_DIR,
        id = contract::EXPECTED_CLIPBOARD_SCHEMA_ID
    );
}
