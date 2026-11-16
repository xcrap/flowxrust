//! FlowX.
//!
//! A `main` that opens the window, or, given `--mcp-server`, serves FlowX's
//! tools to the agent that launched it. Everything else lives in `fx-app` and
//! `fx-bridge`; keeping this file empty of policy is what lets the whole app
//! be driven from a test.

use std::process::ExitCode;

use fx_app::{AppState, open_window};
use fx_store::AppPaths;
use gpui::{App, AppContext, Application};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    // Before anything else: stdout is the MCP channel in that mode, and the
    // app must not start a window, a log subscriber or anything that writes.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == fx_bridge::SERVER_FLAG)
    {
        return fx_bridge::server::run_from_env();
    }

    // Logs go to stderr so a launch from a terminal is still diagnosable, and
    // the filter is overridable because provider failures are the thing people
    // need to see when something goes wrong.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    Application::new()
        .with_assets(fx_app::IconAssets)
        .run(|cx: &mut App| {
            let paths = std::env::var_os("FLOWX_DATA_DIR")
                .map(AppPaths::at)
                .map(Ok)
                .unwrap_or_else(|| AppPaths::discover(cfg!(debug_assertions)))
                .unwrap_or_else(|error| {
                    panic!("could not locate the FlowX support directory: {error}")
                });
            paths.ensure().expect("the FlowX support directory");

            let state = cx.new(|cx| AppState::restore(paths, cx));
            fx_app::AppHandle::install(state.clone(), cx);

            if let Err(error) = open_window(state, cx) {
                tracing::error!(%error, "could not open the FlowX window");
            }
        });

    ExitCode::SUCCESS
}
