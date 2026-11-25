mod actions;
mod cell;
mod definition;
mod diff_highlight;
mod external;
mod highlight;
mod keymap_editor;
mod panels;
mod session;
mod theme;
mod ui;
mod viewer;
mod workspace;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

fn main() {
    // `gridterm mcp`: MCP server over stdio for agents started in a cell.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        let socket = std::env::var_os(agents::socket::SOCKET_ENV).map(std::path::PathBuf::from);
        agents::mcp_stdio::run(std::io::stdin().lock(), std::io::stdout().lock(), socket);
        return;
    }
    // `gridterm open [DIR] [-- COMMAND…]`: a terminal in the running app.
    if std::env::args().nth(1).as_deref() == Some("open") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(external::cli_open(&args));
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let config = config::Config::load();

    // Folders, scripts and `gridterm open` requests that macOS hands to the
    // app (also the ones that launched it): queued until the window exists.
    let (open_tx, mut open_rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    app.on_open_urls(move |urls| {
        let _ = open_tx.unbounded_send(urls);
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        highlight::prewarm();
        let light = config.ui.theme == config::ThemeMode::Light;
        theme::set_light(light);
        terminal::colors::set_light(light);
        Theme::change(if light { ThemeMode::Light } else { ThemeMode::Dark }, None, cx);
        workspace::square_corners(cx);
        actions::apply_keymap(cx, config::Keymap::load());
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "gridterm".into(),
            items: vec![MenuItem::action("Quit gridterm", actions::Quit)],
            disabled: false,
        }]);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(900.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("gridterm".into()), ..Default::default() }),
            window_min_size: Some(size(px(640.), px(400.))),
            // e2e runs: always on top, so other windows never cover it
            // (macOS stops drawing a covered window and the checks stall).
            #[cfg(feature = "e2e")]
            kind: if std::env::var_os("GRIDTERM_E2E").is_some() { WindowKind::PopUp } else { WindowKind::Normal },
            ..Default::default()
        };
        let (window, ws) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| workspace::Workspace::new(config.clone(), window, cx))
        })
        .expect("failed to open window");
        let ws = ws.downgrade();
        cx.spawn(async move |cx| {
            use futures::StreamExt as _;
            while let Some(urls) = open_rx.next().await {
                let ws = ws.clone();
                let _ = cx.update_window(window, move |_, window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_urls(urls, window, cx));
                });
            }
        })
        .detach();
        // Closing the window quits (and saves the session).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
