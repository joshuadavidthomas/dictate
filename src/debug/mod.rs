mod feeders;
mod registry;
mod screens;

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use gpui::App as GpuiApp;
use gpui::Bounds;
use gpui::Context;
use gpui::FocusHandle;
use gpui::IntoElement;
use gpui::KeyBinding;
use gpui::ParentElement;
use gpui::QuitMode;
use gpui::Render;
use gpui::Window;
use gpui::WindowBounds;
use gpui::WindowHandle;
use gpui::WindowOptions;
use gpui::actions;
use gpui::div;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;
use gpui::size;
use gpui_platform::application;
use registry::DebugComponent;
use registry::PreviewClock;
use serde::Serialize;

const WINDOW_WIDTH: f32 = 920.0;
const WINDOW_HEIGHT: f32 = 620.0;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Debug)]
pub struct Args {
    pub list: bool,
    pub screen: Option<String>,
    pub scenario: Option<String>,
}

#[derive(Clone, Debug)]
struct Selection {
    screen: String,
    scenario: String,
}

#[derive(Debug, Serialize)]
struct ScreenListing {
    name: &'static str,
    description: &'static str,
    scenarios: &'static [&'static str],
}

actions!(
    dictate_debug,
    [CloseDebugWindow, NextDebugScenario, PreviousDebugScenario]
);

pub fn run(args: Args) -> Result<()> {
    if args.list {
        println!("{}", list_json()?);
        return Ok(());
    }

    let selection = resolve_selection(args.screen.as_deref(), args.scenario.as_deref())?;

    let window_error = Arc::new(Mutex::new(None));
    let window_error_for_app = Arc::clone(&window_error);

    application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut GpuiApp| {
            cx.bind_keys([
                KeyBinding::new("q", CloseDebugWindow, None),
                KeyBinding::new("right", NextDebugScenario, None),
                KeyBinding::new("tab", NextDebugScenario, None),
                KeyBinding::new("left", PreviousDebugScenario, None),
                KeyBinding::new("shift-tab", PreviousDebugScenario, None),
            ]);
            cx.on_window_closed(|cx, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            if let Err(error) = open_debug_window(cx, selection) {
                *window_error_for_app
                    .lock()
                    .expect("window error lock poisoned") =
                    Some(format!("failed to open debug window: {error:#}"));
                cx.quit();
            }
        });

    if let Some(error) = window_error
        .lock()
        .expect("window error lock poisoned")
        .take()
    {
        bail!(error);
    }

    Ok(())
}

fn open_debug_window(
    cx: &mut GpuiApp,
    selection: Selection,
) -> gpui::Result<WindowHandle<DebugWindow>> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
            ))),
            focus: true,
            is_resizable: true,
            is_minimizable: true,
            app_id: Some("dev.joshthomas.dictate.debug".to_string()),
            ..Default::default()
        },
        |window, cx| {
            let view = cx.new(|cx| DebugWindow::new(selection, cx));
            view.update(cx, |view, cx| {
                view.focus_handle.focus(window, cx);
            });
            view
        },
    )
}

fn list_json() -> Result<String> {
    Ok(serde_json::to_string(&screen_listings())?)
}

fn screen_listings() -> Vec<ScreenListing> {
    let registry = registry::registry();
    validate_registry(&registry).expect("debug registry must contain scenarios");

    registry
        .into_iter()
        .map(|component| ScreenListing {
            name: component.name(),
            description: component.description(),
            scenarios: component.scenarios(),
        })
        .collect()
}

fn validate_registry(registry: &[Box<dyn DebugComponent>]) -> Result<()> {
    for component in registry {
        if component.scenarios().is_empty() {
            bail!(
                "debug screen {:?} must define at least one scenario",
                component.name()
            );
        }
    }

    Ok(())
}

fn resolve_selection(screen: Option<&str>, scenario: Option<&str>) -> Result<Selection> {
    let registry = registry::registry();
    validate_registry(&registry)?;

    let component = match screen {
        Some(screen) => registry
            .iter()
            .find(|component| component.name() == screen)
            .ok_or_else(|| unknown_screen_error(screen, &registry))?,
        None => registry
            .first()
            .ok_or_else(|| anyhow!("debug registry is empty"))?,
    };

    let scenario = match scenario {
        Some(scenario) if component.scenarios().contains(&scenario) => scenario,
        Some(scenario) => bail!(
            "unknown scenario {:?} for screen {:?}; valid scenarios: {}",
            scenario,
            component.name(),
            component.scenarios().join(", ")
        ),
        None => component
            .scenarios()
            .first()
            .copied()
            .ok_or_else(|| anyhow!("debug screen {:?} has no scenarios", component.name()))?,
    };

    Ok(Selection {
        screen: component.name().to_string(),
        scenario: scenario.to_string(),
    })
}

fn unknown_screen_error(screen: &str, registry: &[Box<dyn DebugComponent>]) -> anyhow::Error {
    anyhow!(
        "unknown debug screen {:?}; valid screens: {}",
        screen,
        registry
            .iter()
            .map(|component| component.name())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

struct DebugWindow {
    registry: Vec<Box<dyn DebugComponent>>,
    selected_screen: usize,
    selected_scenario: String,
    preview_started: Instant,
    frame_index: u64,
    focus_handle: FocusHandle,
}

impl DebugWindow {
    fn new(selection: Selection, cx: &mut Context<Self>) -> Self {
        let registry = registry::registry();
        validate_registry(&registry).expect("debug registry must contain scenarios");
        let selected_screen = registry
            .iter()
            .position(|component| component.name() == selection.screen)
            .unwrap_or(0);

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME_INTERVAL).await;

                if this
                    .update(cx, |this, cx| {
                        this.frame_index = this.frame_index.wrapping_add(1);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            registry,
            selected_screen,
            selected_scenario: selection.scenario,
            preview_started: Instant::now(),
            frame_index: 0,
            focus_handle: cx.focus_handle(),
        }
    }

    fn select_screen(&mut self, screen: usize, cx: &mut Context<Self>) {
        self.selected_screen = screen;
        self.selected_scenario = self.registry[screen].scenarios()[0].to_string();
        self.reset_preview_clock();
        cx.notify();
    }

    fn select_scenario(&mut self, scenario: &str, cx: &mut Context<Self>) {
        self.selected_scenario = scenario.to_string();
        self.reset_preview_clock();
        cx.notify();
    }

    fn select_next_scenario(
        &mut self,
        _: &NextDebugScenario,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_scenario(1, cx);
    }

    fn select_previous_scenario(
        &mut self,
        _: &PreviousDebugScenario,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_scenario(-1, cx);
    }

    fn cycle_scenario(&mut self, offset: isize, cx: &mut Context<Self>) {
        let scenarios = self.registry[self.selected_screen].scenarios();
        let current = scenarios
            .iter()
            .position(|scenario| *scenario == self.selected_scenario)
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(scenarios.len() as isize) as usize;
        self.selected_scenario = scenarios[next].to_string();
        self.reset_preview_clock();
        cx.notify();
    }

    fn preview_clock(&self) -> PreviewClock {
        PreviewClock {
            elapsed: self.preview_started.elapsed(),
            frame_index: self.frame_index,
        }
    }

    fn reset_preview_clock(&mut self) {
        self.preview_started = Instant::now();
        self.frame_index = 0;
    }
}

impl Render for DebugWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_screen = self.selected_screen;
        let screen_tabs = self
            .registry
            .iter()
            .enumerate()
            .map(|(screen_ix, component)| {
                let selected = screen_ix == selected_screen;
                div()
                    .id(format!("debug-screen-{}", component.name()))
                    .rounded_md()
                    .px(px(12.0))
                    .py(px(10.0))
                    .cursor_pointer()
                    .bg(if selected {
                        rgb(0x1f2937)
                    } else {
                        rgb(0x111827)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(0x60a5fa)
                    } else {
                        rgb(0x374151)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select_screen(screen_ix, cx)))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(component.name()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x9ca3af))
                            .child(component.description()),
                    )
            })
            .collect::<Vec<_>>();

        let component = &self.registry[self.selected_screen];
        let scenario = self.selected_scenario.as_str();
        let preview = component.preview(scenario, self.preview_clock(), window, cx);
        let scenarios = component
            .scenarios()
            .iter()
            .map(|&scenario| {
                let selected = scenario == self.selected_scenario;
                div()
                    .id(format!("debug-scenario-{scenario}"))
                    .rounded_sm()
                    .px(px(8.0))
                    .py(px(4.0))
                    .cursor_pointer()
                    .bg(if selected {
                        rgb(0x1d4ed8)
                    } else {
                        rgb(0x374151)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select_scenario(scenario, cx)))
                    .text_color(rgb(0xf9fafb))
                    .text_sm()
                    .child(scenario)
            })
            .collect::<Vec<_>>();

        div()
            .on_action(|_: &CloseDebugWindow, window, _| {
                window.remove_window();
            })
            .on_action(cx.listener(Self::select_next_scenario))
            .on_action(cx.listener(Self::select_previous_scenario))
            .track_focus(&self.focus_handle)
            .flex()
            .size_full()
            .bg(rgb(0x030712))
            .text_color(rgb(0xf9fafb))
            .child(
                div()
                    .w(px(280.0))
                    .h_full()
                    .border_r_1()
                    .border_color(rgb(0x1f2937))
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("dictate debug"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x9ca3af))
                            .child("Press q to close."),
                    )
                    .children(screen_tabs),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .p(px(24.0))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(component.name()),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0xd1d5db))
                                    .child(component.description()),
                            )
                            .child(div().flex().gap_2().children(scenarios)),
                    )
                    .child(div().flex_1().child(preview)),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::AnyElement;
    use gpui::App;
    use serde_json::Value;

    use super::*;

    #[test]
    fn list_json_parses_and_enumerates_registry() {
        let json = list_json().unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let screens = parsed.as_array().unwrap();
        let registry = registry::registry();

        assert_eq!(screens.len(), registry.len());
        for (screen, component) in screens.iter().zip(registry) {
            assert_eq!(screen["name"], component.name());
            assert_eq!(screen["description"], component.description());
            let scenarios = screen["scenarios"].as_array().unwrap();
            assert_eq!(scenarios.len(), component.scenarios().len());
            for (scenario, expected) in scenarios.iter().zip(component.scenarios()) {
                assert_eq!(scenario, expected);
            }
        }
    }

    #[test]
    fn list_json_includes_overlay_scenarios() {
        let json = list_json().unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let overlay = parsed
            .as_array()
            .unwrap()
            .iter()
            .find(|screen| screen["name"] == "overlay")
            .expect("overlay screen is registered");

        assert_eq!(
            overlay["scenarios"],
            serde_json::json!([
                "idle",
                "recording-sine",
                "recording-constant",
                "recording-frames",
                "transcribing",
                "unavailable"
            ])
        );
    }

    #[test]
    fn unknown_screen_errors() {
        let error = resolve_selection(Some("nope"), None)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unknown debug screen"));
        assert!(error.contains("nope"));
    }

    #[test]
    fn list_ignores_invalid_selection_flags() {
        run(Args {
            list: true,
            screen: Some("nope".to_string()),
            scenario: None,
        })
        .unwrap();
    }

    #[test]
    fn unknown_scenario_errors() {
        let error = resolve_selection(Some("overlay"), Some("nope"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("unknown scenario"));
        assert!(error.contains("nope"));
        assert!(error.contains("overlay"));
    }

    #[test]
    fn registry_validation_rejects_empty_scenarios() {
        let registry: Vec<Box<dyn DebugComponent>> = vec![Box::new(EmptyScenarioScreen)];
        let error = validate_registry(&registry).unwrap_err().to_string();

        assert!(error.contains("must define at least one scenario"));
        assert!(error.contains("empty"));
    }

    #[test]
    fn selection_defaults_to_first_scenario_for_selected_screen() {
        let selection = resolve_selection(Some("overlay"), None).unwrap();

        assert_eq!(selection.screen, "overlay");
        assert_eq!(selection.scenario, "idle");
    }

    struct EmptyScenarioScreen;

    impl DebugComponent for EmptyScenarioScreen {
        fn name(&self) -> &'static str {
            "empty"
        }

        fn description(&self) -> &'static str {
            "empty scenario test screen"
        }

        fn scenarios(&self) -> &'static [&'static str] {
            &[]
        }

        fn preview(
            &self,
            _scenario: &str,
            _clock: PreviewClock,
            _window: &mut Window,
            _cx: &mut App,
        ) -> AnyElement {
            unreachable!("validation should reject this screen before preview")
        }
    }
}
