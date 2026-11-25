mod actions;
mod cell;
mod definition;
mod diff_highlight;
mod highlight;
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let config = config::Config::load();

    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        highlight::prewarm();
        let light = config.ui.theme == config::ThemeMode::Light;
        theme::set_light(light);
        terminal::colors::set_light(light);
        Theme::change(if light { ThemeMode::Light } else { ThemeMode::Dark }, None, cx);
        workspace::square_corners(cx);
        actions::bind_keys(cx);
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
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| workspace::Workspace::new(config.clone(), window, cx))
        })
        .expect("failed to open window");
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
