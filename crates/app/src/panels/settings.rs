//! Settings: grid size, terminal appearance, git behaviour. Changes apply live.

use config::Config;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::PanelEvent;
use crate::theme;
use crate::ui::{section_header, text_button};

pub struct SettingsPanel {
    config: Config,
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(config: Config, _: &mut Context<Self>) -> Self {
        Self { config }
    }

    /// Configuration the panel shows.
    #[cfg(feature = "e2e")]
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Resets the panel's copy (e.g. after the workspace rejected a change).
    pub fn set_config(&mut self, config: Config, cx: &mut Context<Self>) {
        self.config = config;
        cx.notify();
    }

    fn update(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Config)) {
        let mut next = self.config.clone();
        f(&mut next);
        next.grid.max_cols = next.grid.max_cols.clamp(1, 8);
        next.grid.max_rows = next.grid.max_rows.clamp(1, 8);
        next.terminal.font_size = next.terminal.font_size.clamp(8.0, 32.0);
        if next != self.config {
            self.config = next.clone();
            cx.emit(PanelEvent::ConfigChanged(next));
            cx.notify();
        }
    }

    fn stepper(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
        dec: fn(&mut Config),
        inc: fn(&mut Config),
    ) -> impl IntoElement {
        let button = |suffix: &'static str, text: &'static str, f: fn(&mut Config), cx: &mut Context<Self>| {
            div()
                .id((id, if suffix == "dec" { 0usize } else { 1 }))
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(theme::surface0()))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::surface1())))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.update(cx, f)))
                .child(text)
        };
        row(label).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(button("dec", "−", dec, cx))
                .child(div().w(px(36.)).text_center().child(value))
                .child(button("inc", "+", inc, cx)),
        )
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        f: fn(&mut Config),
    ) -> impl IntoElement {
        row(label).child(
            div()
                .id(id)
                .w(px(34.))
                .h(px(18.))
                .p(px(2.))
                .cursor_pointer()
                .bg(rgb(if on { theme::accent() } else { theme::surface1() }))
                .flex()
                .when(on, gpui_kit::Styled::justify_end)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.update(cx, f)))
                .child(div().size(px(14.)).bg(rgb(theme::crust()))),
        )
    }
}

fn row(label: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(32.))
        .px_3()
        .text_xs()
        .child(div().text_color(rgb(theme::text())).child(label))
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.config;
        let path = Config::path().map_or_default(|p| terminal::display_path(&p));
        div()
            .id("settings")
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .pt_2()
            .child(section_header("Terminal grid", None))
            .child(self.stepper("cols", "Max columns", c.grid.max_cols.to_string(), cx, |c| c.grid.max_cols -= 1, |c| c.grid.max_cols += 1))
            .child(self.stepper("rows", "Max rows", c.grid.max_rows.to_string(), cx, |c| c.grid.max_rows -= 1, |c| c.grid.max_rows += 1))
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(theme::overlay0()))
                    .child("New terminals fill a row left to right, then start a new row. The last row stretches to the full width."),
            )
            .child(section_header("Terminal", None))
            .child(self.stepper("font", "Font size", format!("{}", c.terminal.font_size), cx, |c| c.terminal.font_size -= 1.0, |c| c.terminal.font_size += 1.0))
            .child(self.toggle("dim", "Dim inactive terminals", c.terminal.dim_inactive, cx, |c| c.terminal.dim_inactive = !c.terminal.dim_inactive))
            .child(self.toggle("meta", "Option key as Meta", c.terminal.option_as_meta, cx, |c| c.terminal.option_as_meta = !c.terminal.option_as_meta))
            .child(self.toggle("blink", "Blinking cursor", c.terminal.cursor_blink, cx, |c| c.terminal.cursor_blink = !c.terminal.cursor_blink))
            .child(section_header("Appearance", None))
            .child(self.toggle("light", "Light theme", c.ui.theme == config::ThemeMode::Light, cx, |c| {
                c.ui.theme = if c.ui.theme == config::ThemeMode::Light { config::ThemeMode::Dark } else { config::ThemeMode::Light }
            }))
            .child(self.toggle("sidebar-right", "Sidebar on the right", c.ui.sidebar_side == config::SidebarSide::Right, cx, |c| {
                c.ui.sidebar_side = match c.ui.sidebar_side {
                    config::SidebarSide::Left => config::SidebarSide::Right,
                    config::SidebarSide::Right => config::SidebarSide::Left,
                }
            }))
            .child(section_header("Keyboard", None))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .pb_2()
                    .child(text_button("open-keymap", "Keyboard shortcuts…", false, true, cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::OpenKeymap))))
                    .child(div().text_xs().text_color(rgb(theme::overlay0())).child("Change the shortcut of any action: panels, focus, cells, terminal and file viewer.")),
            )
            .child(section_header("Agents", None))
            .child(self.toggle("claude-ide", "Claude Code /ide integration", c.agents.claude_ide, cx, |c| c.agents.claude_ide = !c.agents.claude_ide))
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(theme::overlay0()))
                    .child("Claude Code and opencode running in a cell see the file shown in that cell and its selection (⌥⌘K sends it to the prompt)."),
            )
            .child(self.toggle("agents-mcp", "MCP tools for agents in cells", c.agents.mcp, cx, |c| c.agents.mcp = !c.agents.mcp))
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_xs()
                    .text_color(rgb(theme::overlay0()))
                    .child("Agents opened in a cell (Claude Code, omp, Copilot CLI) load gridterm's MCP tools automatically: get_selection and get_cell_context."),
            )
            .child(section_header("Git", None))
            .child(self.toggle("rebase", "Pull with rebase", c.git.pull_rebase, cx, |c| c.git.pull_rebase = !c.git.pull_rebase))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .child(text_button("save-config", "Save as defaults", true, true, cx.listener(|this, _: &ClickEvent, _, cx| {
                        match this.config.save() {
                            Ok(()) => cx.emit(PanelEvent::info("Settings saved")),
                            Err(e) => cx.emit(PanelEvent::error(format!("Could not save settings: {e:#}"))),
                        }
                    })))
                    .child(div().text_xs().text_color(rgb(theme::overlay0())).child(path)),
            )
    }
}
