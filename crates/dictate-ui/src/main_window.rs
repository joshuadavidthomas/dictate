use std::collections::HashMap;

use futures::channel::oneshot;
use gpui::AnyElement;
use gpui::Context;
use gpui::Entity;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::FontWeight;
use gpui::Hsla;
use gpui::IntoElement;
use gpui::KeyDownEvent;
use gpui::ParentElement;
use gpui::Render;
use gpui::SharedString;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::svg;
use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Icon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::Theme;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Input;
use gpui_kit::component::input::InputEvent;
use gpui_kit::component::input::InputState;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::v_flex;
use gpui_kit::rgb;

use crate::bridge::DaemonBridge;
use crate::bridge::Feed;
use crate::bridge::PageQuery;
use crate::bridge::PluginInfo;
use crate::bridge::PluginPage;
use crate::bridge::SettingsEdit;

/// Display face for page and section titles; bundled so every platform
/// renders the same letterforms.
pub(crate) const SERIF: &str = "Instrument Serif";
const MONO: &str = "Menlo";
// Brand orange from `packaging/icons/platform/README.md`.
const DICTATE_ORANGE: u32 = 0x00ff_8a25;
const LOCAL_GREEN: u32 = 0x0021_a46b;
const SIDEBAR_WIDTH: f32 = 236.0;
/// Height of the title bar row, which carries the wordmark and status.
const TITLE_BAR_HEIGHT: f32 = 52.0;
/// Inset of the macOS traffic lights from the window's top-left corner,
/// centering the 14 px buttons vertically in the title bar.
pub(crate) const TRAFFIC_LIGHT_INSET: f32 = 19.0;
/// Where title bar content starts on macOS, clear of the traffic lights.
const TITLE_BAR_CONTENT_INSET: f32 = 100.0;
const CONTENT_MAX_WIDTH: f32 = 760.0;
/// The plugin page Home shows the latest rows of: the bundled history.
const RECENT_PAGE: &str = "history";
const RECENT_ROWS: u32 = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MainWindowSettings {
    pub model: Choice,
    pub models: Vec<Choice>,
    pub mode: Choice,
    /// Every writing style, in the order the Style page offers them.
    pub modes: Vec<Choice>,
    pub delivery: Choice,
    pub deliveries: Vec<Choice>,
    /// The configured microphone; an empty key is the system default.
    pub microphone: Choice,
    pub microphones: Vec<Choice>,
    /// Saved settings the running daemon has not picked up yet.
    pub restart_needed: Vec<SharedString>,
    pub shortcut: SharedString,
    /// Where user plugins go.
    pub plugins_dir: SharedString,
    pub dictionary: Vec<WordEntry>,
    pub replacements: Vec<WordEntry>,
    pub style_sample: StyleSample,
}

/// A setting's value as `config.toml` spells it, and as the window names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub key: SharedString,
    pub label: SharedString,
}

/// A spoken phrase and the text Dictate writes for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WordEntry {
    pub spoken: SharedString,
    pub written: SharedString,
}

/// One sentence as spoken and as the configured writing style formats it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleSample {
    pub spoken: SharedString,
    pub written: SharedString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum Page {
    #[default]
    Home,
    /// A page a plugin added, by id.
    Plugin(SharedString),
    Style,
    Dictionary,
    Plugins,
    Settings,
}

/// Something the window asked the plugins for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum Loaded<T> {
    #[default]
    Loading,
    Ready(T),
    Failed(SharedString),
}

pub struct MainWindow {
    page: Page,
    settings: MainWindowSettings,
    plugins: Option<DaemonBridge>,
    plugin_pages: Vec<PluginPage>,
    /// Each plugin page's content, by page id.
    feeds: HashMap<SharedString, Loaded<Feed>>,
    /// The latest rows of [`RECENT_PAGE`], for Home.
    recent: Loaded<Feed>,
    plugin_list: Loaded<Vec<PluginInfo>>,
    /// The Dictionary page's add rows.
    word_form: WordForm,
    replacement_form: WordForm,
    /// Why the last change was not saved.
    edit_error: Option<SharedString>,
    /// The next keystroke becomes the push-to-talk shortcut.
    recording_shortcut: bool,
    focus_handle: FocusHandle,
}

/// Fields for a new dictionary or replacement entry.
struct WordForm {
    spoken: Entity<InputState>,
    written: Entity<InputState>,
}

impl WordForm {
    fn new(
        style: WordStyle,
        spoken: &'static str,
        written: &'static str,
        window: &mut Window,
        cx: &mut Context<MainWindow>,
    ) -> Self {
        let field =
            |placeholder: &'static str, window: &mut Window, cx: &mut Context<MainWindow>| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                // Enter in either field adds the entry.
                cx.subscribe_in(&input, window, move |this, _, event, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.add_word(style, window, cx);
                    }
                })
                .detach();
                input
            };
        Self {
            spoken: field(spoken, window, cx),
            written: field(written, window, cx),
        }
    }
}

impl MainWindow {
    pub fn new(
        settings: MainWindowSettings,
        plugins: Option<DaemonBridge>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let word_form = WordForm::new(
            WordStyle::Spelling,
            "Sounds like, e.g. gee pee you eye",
            "Spelled, e.g. GPUI",
            window,
            cx,
        );
        let replacement_form = WordForm::new(
            WordStyle::Expansion,
            "When you say, e.g. my email",
            "Write, e.g. you@example.com",
            window,
            cx,
        );
        let mut window = Self {
            page: Page::default(),
            settings,
            plugins,
            plugin_pages: Vec::new(),
            feeds: HashMap::new(),
            recent: Loaded::Loading,
            plugin_list: Loaded::Loading,
            word_form,
            replacement_form,
            edit_error: None,
            recording_shortcut: false,
            focus_handle: cx.focus_handle().tab_stop(false),
        };
        window.refresh(cx);
        window
    }

    /// Wait for `reply` off the UI thread, then hand it to `apply` and
    /// redraw. `None` means the daemon dropped the request.
    fn load<T: 'static>(
        reply: oneshot::Receiver<T>,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut Self, Option<T>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let value = reply.await.ok();
            drop(this.update(cx, |this, cx| {
                apply(this, value);
                cx.notify();
            }));
        })
        .detach();
    }

    /// Ask the daemon again for the settings, the plugins' pages, and what
    /// the current page shows.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(plugins) = self.plugins.clone() else {
            return;
        };
        // Plugins reload when their files change, and settings when
        // config.toml does, so both are asked for again rather than kept.
        Self::load(plugins.pages(), cx, |this, pages| {
            this.plugin_pages = pages.unwrap_or_default();
        });
        Self::load(plugins.settings(), cx, |this, settings| {
            if let Some(settings) = settings {
                this.settings = settings;
            }
        });
        match self.page.clone() {
            Page::Home => {
                let query = PageQuery {
                    search: None,
                    limit: Some(RECENT_ROWS),
                };
                Self::load(
                    plugins.render(RECENT_PAGE.into(), query),
                    cx,
                    |this, feed| {
                        this.recent = loaded(feed);
                    },
                );
            }
            Page::Plugin(id) => {
                self.feeds.entry(id.clone()).or_default();
                let reply = plugins.render(id.clone(), PageQuery::default());
                Self::load(reply, cx, move |this, feed| {
                    this.feeds.insert(id, loaded(feed));
                });
            }
            Page::Plugins => {
                Self::load(plugins.plugins(), cx, |this, list| {
                    this.plugin_list = list.map_or_else(
                        || Loaded::Failed("The daemon did not answer.".into()),
                        Loaded::Ready,
                    );
                });
            }
            Page::Style | Page::Dictionary | Page::Settings => {}
        }
    }

    /// Save `edit` to `config.toml` through the daemon, adopt the settings
    /// that come back, then run `saved`. A refused edit shows why instead.
    fn save(
        &mut self,
        edit: SettingsEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
        saved: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(bridge) = self.plugins.clone() else {
            return;
        };
        let reply = bridge.edit(edit);
        cx.spawn_in(window, async move |this, cx| {
            let result = reply
                .await
                .unwrap_or_else(|_| Err("The daemon did not answer.".to_owned()));
            drop(this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(settings) => {
                        this.settings = settings;
                        this.edit_error = None;
                        saved(this, window, cx);
                    }
                    Err(error) => this.edit_error = Some(format!("Not saved: {error}").into()),
                }
                cx.notify();
            }));
        })
        .detach();
    }

    fn add_word(&mut self, style: WordStyle, window: &mut Window, cx: &mut Context<Self>) {
        let form = self.form(style);
        let (spoken_input, written_input) = (form.spoken.clone(), form.written.clone());
        let spoken = spoken_input.read(cx).value().trim().to_owned();
        let written = written_input.read(cx).value().trim().to_owned();
        if spoken.is_empty() || written.is_empty() {
            let empty = if spoken.is_empty() {
                &spoken_input
            } else {
                &written_input
            };
            empty.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        self.save(
            style.add(spoken, written),
            window,
            cx,
            move |_, window, cx| {
                for input in [&written_input, &spoken_input] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                // Back to the row's first field: a word's spelling, a
                // replacement's trigger.
                let first = match style {
                    WordStyle::Spelling => &written_input,
                    WordStyle::Expansion => &spoken_input,
                };
                first.update(cx, |input, cx| input.focus(window, cx));
            },
        );
    }

    fn record_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.recording_shortcut {
            return;
        }
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        self.recording_shortcut = false;
        if keystroke.key == "escape" && !modifiers.modified() {
            cx.notify();
            return;
        }
        let mut shortcut = String::new();
        for (held, name) in [
            (modifiers.platform, "<Command>"),
            (modifiers.control, "<Control>"),
            (modifiers.alt, "<Option>"),
            (modifiers.shift, "<Shift>"),
        ] {
            if held {
                shortcut.push_str(name);
            }
        }
        shortcut.push_str(&keystroke.key);
        // The daemon refuses a shortcut the platform cannot listen for, and
        // the page says why.
        self.save(
            SettingsEdit::SetShortcut(shortcut),
            window,
            cx,
            |_, _, _| {},
        );
        cx.notify();
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        let Some(bridge) = self.plugins.clone() else {
            return;
        };
        // A restart that works replaces this process, so only a refusal
        // ever answers.
        Self::load(bridge.restart(), cx, |this, refused| {
            if let Some(refused) = refused {
                this.edit_error = Some(format!("Not restarted: {refused}").into());
            }
        });
    }

    /// Names the saved settings the daemon has not picked up, with a button
    /// that restarts it.
    fn restart_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let pending = &self.settings.restart_needed;
        if pending.is_empty() {
            return None;
        }
        let names = match pending.as_slice() {
            [only] => only.to_string(),
            [rest @ .., last] => format!(
                "{} and {last}",
                rest.iter()
                    .map(SharedString::as_ref)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            [] => String::new(),
        };
        Some(
            h_flex()
                .id("restart-bar")
                .debug_selector(|| "restart-bar".into())
                .mb_8()
                .px_4()
                .py_3()
                .gap_4()
                .justify_between()
                .rounded_lg()
                .border_1()
                .border_color(rgb(DICTATE_ORANGE).opacity(0.35))
                .bg(rgb(DICTATE_ORANGE).opacity(0.08))
                .child(
                    div()
                        .text_sm()
                        .child(format!("Restart Dictate to apply the new {names}.")),
                )
                .child(
                    Button::new("restart-now")
                        .label("Restart now")
                        .primary()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                )
                .into_any_element(),
        )
    }

    fn form(&self, style: WordStyle) -> &WordForm {
        match style {
            WordStyle::Spelling => &self.word_form,
            WordStyle::Expansion => &self.replacement_form,
        }
    }

    fn select_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.edit_error = None;
        self.recording_shortcut = false;
        self.refresh(cx);
        cx.notify();
    }

    fn nav_item(
        &self,
        page: Page,
        label: SharedString,
        icon: IconName,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.page == page;
        let selector = format!("nav-{label}");
        // Selection changes color and background only, never weight, so the
        // label doesn't shift when it becomes active.
        h_flex()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .h(px(38.0))
            .px_3()
            .gap_3()
            .rounded_lg()
            .text_size(px(15.0))
            .cursor_pointer()
            .text_color(if active {
                theme.sidebar_accent_foreground
            } else {
                theme.muted_foreground
            })
            .when(active, |item| item.bg(theme.sidebar_accent))
            .when(!active, |item| {
                item.hover(|item| {
                    item.bg(theme.sidebar_accent.opacity(0.5))
                        .text_color(theme.sidebar_foreground)
                })
            })
            .child(Icon::new(icon).size_4())
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| this.select_page(page.clone(), cx)))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut nav = vec![
            self.nav_item(Page::Home, "Home".into(), IconName::House, cx)
                .into_any_element(),
        ];
        for page in &self.plugin_pages {
            nav.push(
                self.nav_item(
                    Page::Plugin(page.id.clone()),
                    page.title.clone(),
                    IconName::ScrollText,
                    cx,
                )
                .into_any_element(),
            );
        }
        nav.extend([
            self.nav_item(Page::Style, "Style".into(), IconName::Type, cx)
                .into_any_element(),
            self.nav_item(Page::Dictionary, "Dictionary".into(), IconName::BookA, cx)
                .into_any_element(),
            self.nav_item(Page::Plugins, "Plugins".into(), IconName::Puzzle, cx)
                .into_any_element(),
        ]);
        let settings = self
            .nav_item(Page::Settings, "Settings".into(), IconName::Settings, cx)
            .into_any_element();
        let theme = cx.theme();

        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .px_3()
            .pb_3()
            .text_color(theme.sidebar_foreground)
            .child(
                h_flex()
                    .px_3()
                    .pt_2()
                    .pb_6()
                    .gap_2()
                    .child(spectrum_mark("brand-mark", 22.0))
                    .child(
                        div()
                            .font_family(SERIF)
                            .text_size(px(28.0))
                            .child("Dictate"),
                    ),
            )
            .child(v_flex().gap_0p5().children(nav))
            .child(div().flex_1())
            .child(settings)
            .into_any_element()
    }

    /// The title bar is a toolbar row: native traffic lights on the left, and
    /// on the right a status pill saying where transcription happens.
    fn render_title_bar(&self, theme: &Theme) -> impl IntoElement {
        TitleBar::new()
            .h(px(TITLE_BAR_HEIGHT))
            .when(cfg!(target_os = "macos"), |bar| {
                bar.pl(px(TITLE_BAR_CONTENT_INSET))
            })
            .bg(theme.sidebar)
            .border_b_0()
            .child(
                h_flex().w_full().pr_4().justify_end().child(
                    h_flex()
                        .id("local-status")
                        .debug_selector(|| "local-status".into())
                        .gap_2()
                        .px_2p5()
                        .py_1()
                        .rounded_full()
                        .bg(theme.foreground.opacity(0.05))
                        .text_sm()
                        .child(div().size(px(6.0)).rounded_full().bg(rgb(LOCAL_GREEN)))
                        .child("On this computer")
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(self.settings.model.label.clone()),
                        ),
                ),
            )
    }

    fn render_home(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let settings = &self.settings;
        let keys = shortcut_keys(&settings.shortcut);

        // The instruction reads as a sentence with the keys set inline, so the
        // page leads with what to do rather than with a banner about it.
        let gesture = if keys.is_empty() {
            h_flex()
                .gap_2()
                .child("Run")
                .child(inline_code("dictate record toggle", theme))
                .child("and talk, then run it again.")
        } else {
            h_flex()
                .gap_2()
                .child("Hold")
                .child(
                    h_flex()
                        .gap_1()
                        .children(keys.into_iter().map(|key| key_cap(key, theme))),
                )
                .child("anywhere and talk.")
        };
        let instruction = v_flex().gap_2().text_size(px(17.0)).child(gesture).child(
            div()
                .text_color(theme.muted_foreground)
                .child(delivery_sentence(&settings.delivery.label)),
        );

        v_flex()
            .id("home-page")
            .debug_selector(|| "home-page".into())
            .w_full()
            .gap_12()
            .child(
                v_flex()
                    .gap_5()
                    .child(page_title(
                        "Ready when you are",
                        "Everything happens on this computer. Nothing you say leaves it.",
                        theme,
                    ))
                    .child(instruction),
            )
            .child(self.render_recent(theme))
    }

    /// Home's latest dictations, from the history plugin; nothing when it is
    /// turned off.
    fn render_recent(&self, theme: &Theme) -> AnyElement {
        if !self.plugin_pages.iter().any(|page| page.id == RECENT_PAGE) {
            return div().into_any_element();
        }
        v_flex()
            .id("home-recent")
            .debug_selector(|| "home-recent".into())
            .gap_4()
            .child(section_title("Recent"))
            .child(loaded_view(&self.recent, theme, |feed| {
                feed_view(feed, theme)
            }))
            .into_any_element()
    }

    fn render_plugin_page(&self, id: &SharedString, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let page = self.plugin_pages.iter().find(|page| &page.id == id);
        let title = page.map_or_else(|| id.clone(), |page| page.title.clone());
        let plugin = page.map_or_else(|| id.clone(), |page| page.plugin.clone());
        let content = self.feeds.get(id).cloned().unwrap_or_default();
        v_flex()
            .id("plugin-page")
            .debug_selector(|| "plugin-page".into())
            .w_full()
            .gap_12()
            .child(page_title(
                title,
                format!("From the {plugin} plugin."),
                theme,
            ))
            .child(loaded_view(&content, theme, |feed| feed_view(feed, theme)))
    }

    fn render_plugins(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("plugins-page")
            .debug_selector(|| "plugins-page".into())
            .w_full()
            .gap_12()
            .child(page_title(
                "Plugins",
                "Every part of Dictate is a Lua file you can read, replace, or turn off.",
                theme,
            ))
            .child(loaded_view(&self.plugin_list, theme, |plugins| {
                v_flex()
                    .border_b_1()
                    .border_color(hairline(theme))
                    .children(plugins.iter().map(|plugin| plugin_row(plugin, theme)))
                    .into_any_element()
            }))
            .child(config_hint(
                "Add your own, or replace one by name, as .lua files in",
                &self.settings.plugins_dir,
                theme,
            ))
    }

    fn render_style(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let settings = &self.settings;
        v_flex()
            .id("style-page")
            .debug_selector(|| "style-page".into())
            .w_full()
            .gap_12()
            .child(page_title(
                "Style",
                "Fillers dropped, spoken punctuation applied, sentences capitalized.",
                theme,
            ))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(section_title("Your style")),
                    )
                    .child(Self::choices(
                        "style-modes",
                        &settings.modes,
                        &settings.mode,
                        SettingsEdit::SetMode,
                        cx,
                    ))
                    .child(
                        h_flex()
                            .items_stretch()
                            .border_t_1()
                            .border_b_1()
                            .border_color(hairline(theme))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_3()
                                    .py_6()
                                    .pr_8()
                                    .child(label("You say", theme))
                                    .child(
                                        div()
                                            .font_family(SERIF)
                                            .italic()
                                            .text_size(px(22.0))
                                            .line_height(px(30.0))
                                            .text_color(theme.muted_foreground)
                                            .child(format!(
                                                "\u{201c}{}\u{201d}",
                                                settings.style_sample.spoken
                                            )),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_3()
                                    .py_6()
                                    .pl_8()
                                    .border_l_1()
                                    .border_color(hairline(theme))
                                    .child(label("Dictate writes", theme))
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .line_height(px(28.0))
                                            .child(settings.style_sample.written.clone()),
                                    ),
                            ),
                    ),
            )
    }

    fn render_dictionary(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let settings = &self.settings;
        v_flex()
            .id("dictionary-page")
            .debug_selector(|| "dictionary-page".into())
            .w_full()
            .gap_12()
            .child(page_title(
                "Dictionary",
                "Names, jargon, and shorthand Dictate should always get right.",
                theme,
            ))
            .child(self.word_section(
                "Words",
                "dictionary-words",
                &settings.dictionary,
                WordStyle::Spelling,
                cx,
            ))
            .child(self.word_section(
                "Replacements",
                "dictionary-replacements",
                &settings.replacements,
                WordStyle::Expansion,
                cx,
            ))
    }

    fn word_section(
        &self,
        title: &'static str,
        id: &'static str,
        entries: &[WordEntry],
        style: WordStyle,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let rows = entries.iter().enumerate().map(|(index, entry)| {
            let spoken = entry.spoken.to_string();
            let remove = Button::new((id, index))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .tooltip("Remove")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.save(style.remove(spoken.clone()), window, cx, |_, _, _| {});
                }));
            let row = h_flex()
                .py_3()
                .gap_4()
                .border_t_1()
                .border_color(hairline(theme));
            let row = match style {
                WordStyle::Spelling => row
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_base()
                            .child(entry.written.clone()),
                    )
                    .child(
                        div()
                            .font_family(SERIF)
                            .italic()
                            .text_size(px(18.0))
                            .text_color(theme.muted_foreground)
                            .child(format!("\u{201c}{}\u{201d}", entry.spoken)),
                    ),
                WordStyle::Expansion => row
                    .child(div().flex_none().text_base().child(entry.spoken.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(DICTATE_ORANGE))
                            .child(Icon::new(IconName::ArrowRight).size_4()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_base()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(entry.written.clone()),
                    ),
            };
            row.child(remove)
        });

        // The fields follow the rows' order: a word leads with its spelling,
        // a replacement with what you say.
        let form = self.form(style);
        let (first, second) = match style {
            WordStyle::Spelling => (&form.written, &form.spoken),
            WordStyle::Expansion => (&form.spoken, &form.written),
        };
        let add_row = h_flex()
            .id(SharedString::from(format!("{id}-add")))
            .debug_selector(move || format!("{id}-add"))
            .pt_3()
            .gap_2()
            .border_t_1()
            .border_color(hairline(theme))
            .child(div().flex_1().min_w_0().child(Input::new(first)))
            .child(div().flex_1().min_w_0().child(Input::new(second)))
            .child(
                Button::new(SharedString::from(format!("{id}-add-button")))
                    .label("Add")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_word(style, window, cx);
                    })),
            );

        v_flex()
            .id(id)
            .debug_selector(|| id.to_owned())
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(section_title(title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(entries.len().to_string()),
                    ),
            )
            .child(v_flex().children(rows).child(add_row))
    }

    /// One button per choice; the current one is filled, and clicking
    /// another saves it.
    fn choices(
        id: &'static str,
        choices: &[Choice],
        current: &Choice,
        edit: fn(SharedString) -> SettingsEdit,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .debug_selector(|| id.to_owned())
            .flex_wrap()
            .gap_2()
            .children(choices.iter().enumerate().map(|(index, choice)| {
                let key = choice.key.clone();
                let button = Button::new((id, index)).label(choice.label.clone()).small();
                if choice.key == current.key {
                    button.primary()
                } else {
                    button
                        .outline()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.save(edit(key.clone()), window, cx, |_, _, _| {});
                        }))
                }
            }))
    }

    /// A button naming the current choice that opens a menu of the others;
    /// picking one saves it.
    fn menu_choice(
        id: &'static str,
        choices: &[Choice],
        current: &Choice,
        edit: fn(SharedString) -> SettingsEdit,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let choices = choices.to_vec();
        let current_key = current.key.clone();
        Button::new(id)
            .label(current.label.clone())
            .outline()
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                choices.iter().fold(menu, |menu, choice| {
                    let this = this.clone();
                    let key = choice.key.clone();
                    menu.item(
                        PopupMenuItem::new(choice.label.clone())
                            .checked(choice.key == current_key)
                            .on_click(move |_, window, cx| {
                                drop(this.update(cx, |this, cx| {
                                    this.save(edit(key.clone()), window, cx, |_, _, _| {});
                                }));
                            }),
                    )
                })
            })
    }

    /// The push-to-talk keys and a button that records new ones.
    fn shortcut_control(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        if self.recording_shortcut {
            return value_text("Press the shortcut, or Esc to cancel".into(), theme);
        }
        let keys = shortcut_keys(&self.settings.shortcut);
        let keys = if keys.is_empty() {
            value_text("Not configured".into(), theme)
        } else {
            h_flex()
                .gap_1()
                .children(keys.into_iter().map(|key| key_cap(key, theme)))
                .into_any_element()
        };
        h_flex()
            .gap_3()
            .child(keys)
            .child(
                Button::new("settings-shortcut-record")
                    .label("Change")
                    .outline()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.recording_shortcut = true;
                        this.edit_error = None;
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_settings(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let settings = &self.settings;
        let shortcut = self.shortcut_control(cx);

        v_flex()
            .id("settings-page")
            .debug_selector(|| "settings-page".into())
            .w_full()
            .gap_12()
            .child(page_title(
                "Settings",
                "How Dictate listens, and where your words go.",
                theme,
            ))
            .child(settings_section(
                "Dictation",
                vec![
                    setting_row(
                        "Push to talk",
                        "Records while held, anywhere on your computer. Applies after a restart.",
                        shortcut,
                        theme,
                    ),
                    setting_row(
                        "Microphone",
                        "Audio input for new recordings. Applies after a restart.",
                        Self::menu_choice(
                            "settings-microphone",
                            &settings.microphones,
                            &settings.microphone,
                            SettingsEdit::SetMicrophone,
                            cx,
                        )
                        .into_any_element(),
                        theme,
                    ),
                    setting_row(
                        "Delivery",
                        "Where finished dictation goes. Applies after a restart.",
                        Self::choices(
                            "settings-delivery",
                            &settings.deliveries,
                            &settings.delivery,
                            SettingsEdit::SetDelivery,
                            cx,
                        )
                        .into_any_element(),
                        theme,
                    ),
                ],
                theme,
            ))
            .child(settings_section(
                "Transcription",
                vec![
                    setting_row(
                        "Model",
                        "Runs locally and produces the final transcript. Applies after a restart, which downloads it if needed.",
                        Self::menu_choice(
                            "settings-model",
                            &settings.models,
                            &settings.model,
                            SettingsEdit::SetModel,
                            cx,
                        )
                        .into_any_element(),
                        theme,
                    ),
                    setting_row(
                        "Writing style",
                        "Punctuation, capitalization, and cleanup.",
                        Self::menu_choice(
                            "settings-mode",
                            &settings.modes,
                            &settings.mode,
                            SettingsEdit::SetMode,
                            cx,
                        )
                        .into_any_element(),
                        theme,
                    ),
                ],
                theme,
            ))
    }
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.render_sidebar(cx);
        let restart_bar = self.restart_bar(cx);
        let edit_error = self.edit_error.clone().map(|error| {
            div()
                .id("edit-error")
                .debug_selector(|| "edit-error".into())
                .mb_6()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error)
        });
        let content = match &self.page {
            Page::Home => self.render_home(cx).into_any_element(),
            Page::Plugin(id) => self.render_plugin_page(&id.clone(), cx).into_any_element(),
            Page::Style => self.render_style(cx).into_any_element(),
            Page::Dictionary => self.render_dictionary(cx).into_any_element(),
            Page::Plugins => self.render_plugins(cx).into_any_element(),
            Page::Settings => self.render_settings(cx).into_any_element(),
        };
        let theme = cx.theme();

        // A warm canvas holds the borderless sidebar; each page sits on a
        // raised sheet beside it.
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::record_shortcut))
            .bg(theme.sidebar)
            .text_color(theme.foreground)
            .child(self.render_title_bar(theme))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(sidebar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .mr_2()
                            .mb_2()
                            .rounded_2xl()
                            .border_1()
                            .border_color(hairline(theme))
                            .bg(theme.background)
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("main-content")
                                    .debug_selector(|| "main-content".into())
                                    .size_full()
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(CONTENT_MAX_WIDTH))
                                            .mx_auto()
                                            .px_12()
                                            .pt_12()
                                            .pb_16()
                                            .children(restart_bar)
                                            .children(edit_error)
                                            .child(content),
                                    ),
                            ),
                    ),
            )
    }
}

fn spectrum_mark(id: &'static str, size: f32) -> impl IntoElement {
    // `svg()` rasterizes at the drawn size; `img()` would rasterize at the SVG's intrinsic
    // 579 px and let the GPU shrink it, which aliases the bars at sidebar sizes.
    svg()
        .path(crate::assets::DICTATE_MARK)
        .id(id)
        .debug_selector(|| id.to_owned())
        .size(px(size))
        .flex_none()
        .text_color(rgb(DICTATE_ORANGE))
}

/// Translucent foreground for dividers, so separators read the same on the
/// light and dark sheets.
fn hairline(theme: &Theme) -> Hsla {
    theme.foreground.opacity(0.08)
}

fn page_title(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .font_family(SERIF)
                .text_size(px(48.0))
                .line_height(px(54.0))
                .child(title.into()),
        )
        .child(
            div()
                .text_base()
                .text_color(theme.muted_foreground)
                .child(description.into()),
        )
}

fn loaded<T>(reply: Option<Result<T, String>>) -> Loaded<T> {
    match reply {
        Some(Ok(value)) => Loaded::Ready(value),
        Some(Err(error)) => Loaded::Failed(error.into()),
        None => Loaded::Failed("The daemon did not answer.".into()),
    }
}

/// `ready` once loaded; otherwise a quiet line saying why not.
fn loaded_view<T>(
    loaded: &Loaded<T>,
    theme: &Theme,
    ready: impl FnOnce(&T) -> AnyElement,
) -> AnyElement {
    match loaded {
        Loaded::Ready(value) => ready(value),
        Loaded::Loading => div()
            .text_color(theme.muted_foreground)
            .child("Loading\u{2026}")
            .into_any_element(),
        Loaded::Failed(error) => v_flex()
            .gap_2()
            .child(div().text_color(theme.danger).child("This could not load."))
            .child(
                div()
                    .font_family(MONO)
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(error.clone()),
            )
            .into_any_element(),
    }
}

/// Rows under small day headings, divided by hairlines: the content leads,
/// with when and where set quietly to the right.
fn feed_view(feed: &Feed, theme: &Theme) -> AnyElement {
    if feed.groups.is_empty() {
        let empty = feed
            .empty
            .clone()
            .unwrap_or_else(|| "Nothing here yet.".into());
        return div()
            .text_base()
            .text_color(theme.muted_foreground)
            .child(empty)
            .into_any_element();
    }
    v_flex()
        .gap_10()
        .children(feed.groups.iter().map(|group| {
            v_flex()
                .child(
                    div()
                        .pb_2()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(group.title.clone()),
                )
                .children(group.rows.iter().map(|row| {
                    h_flex()
                        .items_start()
                        .gap_6()
                        .py_3()
                        .border_t_1()
                        .border_color(hairline(theme))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .text_base()
                                        .line_height(px(24.0))
                                        .child(row.text.clone()),
                                )
                                .when_some(row.detail.clone(), |column, detail| {
                                    column.child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child(detail),
                                    )
                                }),
                        )
                        .when_some(row.meta.clone(), |line, meta| {
                            line.child(
                                div()
                                    .flex_none()
                                    .pt_0p5()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(meta),
                            )
                        })
                }))
        }))
        .into_any_element()
}

fn plugin_row(plugin: &PluginInfo, theme: &Theme) -> impl IntoElement {
    let (status, color) = match (plugin.active, plugin.errors.is_empty()) {
        (true, true) => ("Running", Hsla::from(rgb(LOCAL_GREEN))),
        (true, false) => ("Running its last good version", theme.warning),
        (false, true) => ("Off", theme.muted_foreground),
        (false, false) => ("Not running", theme.danger),
    };
    let source = if plugin.source == "bundled" {
        SharedString::from("Bundled with Dictate")
    } else {
        plugin.source.clone()
    };
    v_flex()
        .gap_2()
        .py_4()
        .border_t_1()
        .border_color(hairline(theme))
        .child(
            h_flex()
                .gap_6()
                .items_start()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::MEDIUM)
                                .child(plugin.id.clone()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(source),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .pt_1()
                        .text_sm()
                        .child(div().size(px(6.0)).rounded_full().bg(color))
                        .child(status),
                ),
        )
        .children(plugin.errors.iter().map(|error| {
            div()
                .font_family(MONO)
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone())
        }))
}

fn section_title(title: &'static str) -> impl IntoElement {
    div()
        .font_family(SERIF)
        .text_size(px(26.0))
        .line_height(px(32.0))
        .child(title)
}

fn label(text: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .child(text)
}

fn key_cap(key: String, theme: &Theme) -> impl IntoElement {
    div()
        .min_w(px(28.0))
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_b_2()
        .border_color(theme.foreground.opacity(0.14))
        .bg(theme.background)
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_center()
        .child(key)
}

fn inline_code(text: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .px_1p5()
        .rounded_md()
        .bg(theme.foreground.opacity(0.06))
        .font_family(MONO)
        .text_sm()
        .child(text.into())
}

#[derive(Clone, Copy)]
enum WordStyle {
    /// A dictionary term: the written form leads, the pronunciation follows.
    Spelling,
    /// A replacement: the trigger phrase expands into longer text.
    Expansion,
}

impl WordStyle {
    fn add(self, spoken: String, written: String) -> SettingsEdit {
        match self {
            Self::Spelling => SettingsEdit::AddDictionary { spoken, written },
            Self::Expansion => SettingsEdit::AddReplacement { spoken, written },
        }
    }

    fn remove(self, spoken: String) -> SettingsEdit {
        match self {
            Self::Spelling => SettingsEdit::RemoveDictionary { spoken },
            Self::Expansion => SettingsEdit::RemoveReplacement { spoken },
        }
    }
}

/// A quiet footnote pointing at a path.
fn config_hint(
    lead: impl Into<SharedString>,
    path: &SharedString,
    theme: &Theme,
) -> impl IntoElement {
    h_flex()
        .flex_wrap()
        .gap_x_2()
        .gap_y_1()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(lead.into())
        .child(inline_code(path.clone(), theme))
}

fn settings_section(title: &'static str, rows: Vec<AnyElement>, theme: &Theme) -> impl IntoElement {
    let rows = rows
        .into_iter()
        .map(|row| div().border_t_1().border_color(hairline(theme)).child(row));
    v_flex().gap_3().child(section_title(title)).child(
        v_flex()
            .border_b_1()
            .border_color(hairline(theme))
            .children(rows),
    )
}

fn setting_row(
    label: &'static str,
    description: &'static str,
    value: AnyElement,
    theme: &Theme,
) -> AnyElement {
    h_flex()
        .justify_between()
        .gap_6()
        .py_4()
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(description),
                ),
        )
        .child(div().flex_none().max_w(px(380.0)).child(value))
        .into_any_element()
}

fn value_text(value: SharedString, theme: &Theme) -> AnyElement {
    div()
        .text_base()
        .text_color(theme.muted_foreground)
        .truncate()
        .child(value)
        .into_any_element()
}

fn delivery_sentence(delivery: &str) -> String {
    match delivery {
        "Insert at cursor" => "Dictate types it wherever your cursor is.".to_owned(),
        "Clipboard" => "Dictate copies it to your clipboard.".to_owned(),
        "Standard output" => "Dictate prints it to standard output.".to_owned(),
        other => format!("Dictate delivers it to {}.", other.to_lowercase()),
    }
}

/// Splits a shortcut such as `<Super>d` or `Ctrl+Alt+Space` into key-cap labels.
/// Anything that isn't a shortcut, like "Not configured", yields no keys.
fn shortcut_keys(shortcut: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = shortcut.trim();
    while let Some(modifier) = rest.strip_prefix('<') {
        let Some((key, tail)) = modifier.split_once('>') else {
            break;
        };
        keys.push(key.to_owned());
        rest = tail;
    }
    if keys.is_empty() && !rest.contains('+') && rest.chars().count() > 1 && rest.contains(' ') {
        return Vec::new();
    }
    keys.extend(
        rest.split('+')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| {
                let mut chars = key.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(chars).collect()
                })
            }),
    );
    keys
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use gpui::point;
    use gpui::size;

    use super::*;

    fn choice(key: &str, label: &str) -> Choice {
        Choice {
            key: key.to_owned().into(),
            label: label.to_owned().into(),
        }
    }

    fn settings() -> MainWindowSettings {
        MainWindowSettings {
            model: choice("final-model", "Final model"),
            models: vec![choice("final-model", "Final model")],
            mode: choice("technical", "Technical"),
            modes: vec![choice("technical", "Technical"), choice("email", "Email")],
            delivery: choice("insert", "Insert at cursor"),
            deliveries: vec![choice("insert", "Insert at cursor")],
            microphone: choice("", "System default"),
            microphones: vec![choice("", "System default")],
            restart_needed: Vec::new(),
            shortcut: "Not configured".into(),
            plugins_dir: "~/.config/dictate/plugins".into(),
            dictionary: vec![WordEntry {
                spoken: "gee pee you eye".into(),
                written: "GPUI".into(),
            }],
            replacements: Vec::new(),
            style_sample: StyleSample {
                spoken: "um ship it".into(),
                written: "Ship it".into(),
            },
        }
    }

    fn history_feed() -> Feed {
        Feed {
            groups: vec![crate::bridge::FeedGroup {
                title: "Today".into(),
                rows: vec![crate::bridge::FeedRow {
                    text: "Ship it.".into(),
                    detail: Some("Heard \u{201c}ship it period\u{201d}".into()),
                    meta: Some("9:41 AM".into()),
                }],
            }],
            empty: None,
        }
    }

    /// Plugins that answer at once: one `history` page, and one plugin.
    fn fake_plugins() -> DaemonBridge {
        DaemonBridge::new(|request| match request {
            crate::bridge::WindowRequest::Pages(reply) => drop(reply.send(vec![PluginPage {
                id: "history".into(),
                title: "History".into(),
                plugin: "history".into(),
            }])),
            crate::bridge::WindowRequest::Render { page, query, reply } => {
                assert_eq!(page.as_ref(), "history");
                let mut feed = history_feed();
                feed.groups[0]
                    .rows
                    .truncate(query.limit.map_or(usize::MAX, |n| n as usize));
                drop(reply.send(Ok(feed)));
            }
            crate::bridge::WindowRequest::Plugins(reply) => drop(reply.send(vec![PluginInfo {
                id: "history".into(),
                source: "bundled".into(),
                active: true,
                errors: Vec::new(),
            }])),
            // config.toml changed since the window opened.
            crate::bridge::WindowRequest::Settings(reply) => drop(reply.send(MainWindowSettings {
                mode: choice("email", "Email"),
                ..settings()
            })),
            crate::bridge::WindowRequest::Restart(reply) => {
                drop(reply.send("finish the dictation in progress first".to_owned()));
            }
            // The daemon saves the word and answers with the settings after it.
            crate::bridge::WindowRequest::Edit { edit, reply } => {
                let mut settings = settings();
                if let SettingsEdit::AddDictionary { spoken, written } = edit {
                    settings.dictionary.push(WordEntry {
                        spoken: spoken.into(),
                        written: written.into(),
                    });
                } else if let SettingsEdit::SetShortcut(shortcut) = edit {
                    settings.shortcut = shortcut.into();
                    settings.restart_needed = vec!["push-to-talk shortcut".into()];
                } else {
                    drop(reply.send(Err("not saved here".to_owned())));
                    return;
                }
                drop(reply.send(Ok(settings)));
            }
        })
    }

    #[test]
    fn shortcut_keys_split_accelerators_and_reject_prose() {
        assert_eq!(shortcut_keys("<Super>d"), ["Super", "D"]);
        assert_eq!(shortcut_keys("<Ctrl><Alt>space"), ["Ctrl", "Alt", "Space"]);
        assert_eq!(shortcut_keys("Ctrl+Shift+k"), ["Ctrl", "Shift", "K"]);
        assert!(shortcut_keys("Not configured").is_empty());
    }

    #[gpui::test]
    fn home_is_default_and_each_page_can_be_selected(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let cx = cx.add_empty_window();
        let view = cx.update(|window, cx| {
            cx.new(|cx| MainWindow::new(settings(), Some(fake_plugins()), window, cx))
        });
        cx.run_until_parked();
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                point(px(0.0), px(0.0)),
                size(px(1120.0), px(780.0)),
                |_, _| view.clone().into_any_element(),
            );
        };

        draw(cx);
        assert_eq!(view.update(cx, |view, _| view.page.clone()), Page::Home);
        assert!(cx.debug_bounds("brand-mark").is_some());
        assert!(cx.debug_bounds("home-page").is_some());

        assert!(
            cx.debug_bounds("nav-History").is_some(),
            "the plugin's page is listed"
        );
        assert_eq!(
            view.update(cx, |view, _| view.settings.mode.label.clone()),
            "Email",
            "the window shows the settings the daemon has now, not the ones it opened with"
        );
        assert_eq!(
            view.update(cx, |view, _| view.recent.clone()),
            Loaded::Ready(history_feed()),
            "Home asks the history page for its latest rows"
        );

        for (page, selector) in [
            (Page::Plugin("history".into()), "plugin-page"),
            (Page::Plugins, "plugins-page"),
            (Page::Style, "style-page"),
            (Page::Dictionary, "dictionary-words"),
            (Page::Settings, "settings-page"),
        ] {
            view.update(cx, |view, cx| view.select_page(page, cx));
            cx.run_until_parked();
            draw(cx);
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} should render"
            );
            assert!(cx.debug_bounds("home-page").is_none());
        }

        view.update_in(cx, |view, window, cx| {
            view.select_page(Page::Dictionary, cx);
            for (input, text) in [
                (view.word_form.spoken.clone(), "  oh pen code "),
                (view.word_form.written.clone(), "OpenCode"),
            ] {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
            view.add_word(WordStyle::Spelling, window, cx);
        });
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("dictionary-words-add").is_some());
        assert_eq!(
            view.update(cx, |view, _| view.settings.dictionary.last().cloned()),
            Some(WordEntry {
                spoken: "oh pen code".into(),
                written: "OpenCode".into(),
            }),
            "the window saves the trimmed word and shows the settings that come back"
        );
        assert_eq!(
            view.update(cx, |view, cx| view.word_form.written.read(cx).value()),
            "",
            "a saved word clears the add row"
        );
    }

    #[gpui::test]
    fn settings_saves_a_recorded_shortcut_and_offers_a_restart(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let cx = cx.add_empty_window();
        let view = cx.update(|window, cx| {
            cx.new(|cx| MainWindow::new(settings(), Some(fake_plugins()), window, cx))
        });
        cx.run_until_parked();
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                point(px(0.0), px(0.0)),
                size(px(1120.0), px(780.0)),
                |_, _| view.clone().into_any_element(),
            );
        };
        draw(cx);
        assert!(
            cx.debug_bounds("restart-bar").is_none(),
            "nothing waits on a restart yet"
        );

        view.update_in(cx, |view, window, cx| {
            view.select_page(Page::Settings, cx);
            view.recording_shortcut = true;
            let press = KeyDownEvent {
                keystroke: gpui::Keystroke::parse("cmd-shift-d").expect("keystroke"),
                is_held: false,
                prefer_character_input: false,
            };
            view.record_shortcut(&press, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            view.update(cx, |view, _| (
                view.recording_shortcut,
                view.settings.shortcut.clone()
            )),
            (false, "<Command><Shift>d".into()),
            "the next keystroke is saved as the shortcut, in the form config.toml uses"
        );

        draw(cx);
        assert!(
            cx.debug_bounds("restart-bar").is_some(),
            "a saved setting the daemon has not picked up offers a restart"
        );
        view.update(cx, MainWindow::restart);
        cx.run_until_parked();
        assert_eq!(
            view.update(cx, |view, _| view.edit_error.clone()),
            Some("Not restarted: finish the dictation in progress first".into()),
            "a refused restart says why"
        );
    }
}
