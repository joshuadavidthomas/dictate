mod chrome;
mod feeders;
mod registry;
mod screens;
mod stats;

use std::io;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use chrome::StatBlockOptions;
use chrome::stat_block;
use chrome::stats_row;
use dictate_speech::TranscriptionPlan;
use gpui::AnyElement;
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
use stats::FrameRecord;
use stats::StatsSession;

const WINDOW_WIDTH: f32 = 920.0;
const WINDOW_HEIGHT: f32 = 620.0;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Debug)]
pub struct Args {
    pub list: bool,
    pub screen: Option<String>,
    pub scenario: Option<String>,
    pub stats: Option<StatsFormat>,
    pub duration: Option<Duration>,
    pub frames: Option<u64>,
    pub exit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatsFormat {
    Json,
}

#[derive(Clone, Debug)]
struct Selection {
    screen: String,
    scenario: String,
}

#[derive(Clone, Copy, Debug)]
struct DebugOptions {
    stats: Option<StatsFormat>,
    duration: Option<Duration>,
    frames: Option<u64>,
    exit_on_bound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinalAggregatesState {
    Pending,
    Streamed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatsOutputState {
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StatsStreamState {
    final_aggregates: FinalAggregatesState,
    output: StatsOutputState,
}

impl Default for StatsStreamState {
    fn default() -> Self {
        Self {
            final_aggregates: FinalAggregatesState::Pending,
            output: StatsOutputState::Open,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScenarioStep {
    Next,
    Previous,
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

type PlanFactory = Arc<dyn Fn() -> Result<TranscriptionPlan> + Send + Sync>;

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn run(
    args: &Args,
    create_plan: impl Fn() -> Result<TranscriptionPlan> + Send + Sync + 'static,
) -> Result<()> {
    let plan_factory: PlanFactory = Arc::new(create_plan);

    if args.list {
        println!("{}", list_json(Arc::clone(&plan_factory))?);
        return Ok(());
    }

    let selection = resolve_selection(
        args.screen.as_deref(),
        args.scenario.as_deref(),
        Arc::clone(&plan_factory),
    )?;
    if args.stats.is_some() && selection.screen != "overlay" {
        bail!("--stats is only supported for the overlay debug screen");
    }

    let options = DebugOptions {
        stats: args.stats,
        duration: args.duration,
        frames: args.frames,
        exit_on_bound: args.exit || args.duration.is_some() || args.frames.is_some(),
    };

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

            if let Err(error) = open_debug_window(
                cx,
                selection,
                options,
                Arc::clone(&window_error_for_app),
                plan_factory,
            ) {
                *lock_or_recover(&window_error_for_app) =
                    Some(format!("failed to open debug window: {error:#}"));
                cx.quit();
            }
        });

    if let Some(error) = lock_or_recover(&window_error).take() {
        bail!(error);
    }

    Ok(())
}

fn open_debug_window(
    cx: &mut GpuiApp,
    selection: Selection,
    options: DebugOptions,
    error_sink: Arc<Mutex<Option<String>>>,
    plan_factory: PlanFactory,
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
            app_id: Some(env!("DICTATE_DEBUG_APP_ID").to_owned()),
            ..Default::default()
        },
        |window, cx| {
            let view =
                cx.new(|cx| DebugWindow::new(selection, options, error_sink, plan_factory, cx));
            view.update(cx, |view, cx| {
                view.focus_handle.focus(window, cx);
            });
            view
        },
    )
}

fn list_json(plan_factory: PlanFactory) -> Result<String> {
    Ok(serde_json::to_string(&screen_listings(plan_factory)?)?)
}

fn screen_listings(plan_factory: PlanFactory) -> Result<Vec<ScreenListing>> {
    let registry = registry::registry(plan_factory);
    validate_registry(&registry)?;

    Ok(registry
        .into_iter()
        .map(|component| ScreenListing {
            name: component.name(),
            description: component.description(),
            scenarios: component.scenarios(),
        })
        .collect())
}

fn validate_registry(registry: &[Box<dyn DebugComponent>]) -> Result<()> {
    for component in registry {
        let scenarios = component.scenarios();
        if scenarios.is_empty() {
            bail!(
                "debug screen {:?} must define at least one scenario",
                component.name()
            );
        }

        let mut activatable = Vec::new();
        for row in component.scenario_rows() {
            for chip in &row.chips {
                activatable.push(chip.activates);
                for id in std::iter::once(&chip.activates).chain(&chip.matches) {
                    if !scenarios.contains(id) {
                        bail!(
                            "debug screen {:?} scenario row {:?} references unknown scenario {id:?}",
                            component.name(),
                            row.label
                        );
                    }
                }
            }
        }
        for scenario in scenarios {
            if !activatable.contains(scenario) {
                bail!(
                    "debug screen {:?} scenario {scenario:?} is not activatable from any scenario row",
                    component.name()
                );
            }
        }
    }

    Ok(())
}

fn resolve_selection(
    screen: Option<&str>,
    scenario: Option<&str>,
    plan_factory: PlanFactory,
) -> Result<Selection> {
    let registry = registry::registry(plan_factory);
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
    last_frame: Instant,
    stats: StatsSession,
    stats_format: Option<StatsFormat>,
    duration_bound: Option<Duration>,
    frame_bound: Option<u64>,
    exit_on_bound: bool,
    stats_stream: StatsStreamState,
    close_requested: bool,
    error_sink: Arc<Mutex<Option<String>>>,
    focus_handle: FocusHandle,
}

impl DebugWindow {
    fn new(
        selection: Selection,
        options: DebugOptions,
        error_sink: Arc<Mutex<Option<String>>>,
        plan_factory: PlanFactory,
        cx: &mut Context<Self>,
    ) -> Self {
        let registry = registry::registry(plan_factory);
        if let Err(error) = validate_registry(&registry) {
            *lock_or_recover(&error_sink) = Some(format!("invalid debug registry: {error:#}"));
        }
        let selected_screen = registry
            .iter()
            .position(|component| component.name() == selection.screen)
            .unwrap_or(0);
        let now = Instant::now();
        registry[selected_screen].reset(&selection.scenario, cx);

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME_INTERVAL).await;

                if this
                    .update(cx, |this, cx| {
                        this.advance_frame(cx);
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
            preview_started: now,
            frame_index: 0,
            last_frame: now,
            stats: StatsSession::new(FRAME_INTERVAL),
            stats_format: options.stats,
            duration_bound: options.duration,
            frame_bound: options.frames,
            exit_on_bound: options.exit_on_bound,
            stats_stream: StatsStreamState::default(),
            close_requested: false,
            error_sink,
            focus_handle: cx.focus_handle(),
        }
    }

    fn select_screen(&mut self, screen: usize, cx: &mut Context<Self>) {
        self.registry[self.selected_screen].deactivate();
        self.selected_screen = screen;
        self.selected_scenario = self.registry[screen].scenarios()[0].to_string();
        self.reset_preview_clock(cx);
        cx.notify();
    }

    fn select_scenario(&mut self, scenario: &str, cx: &mut Context<Self>) {
        self.selected_scenario = scenario.to_string();
        self.reset_preview_clock(cx);
        cx.notify();
    }

    fn select_next_scenario(
        &mut self,
        _: &NextDebugScenario,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_scenario(ScenarioStep::Next, cx);
    }

    fn select_previous_scenario(
        &mut self,
        _: &PreviousDebugScenario,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_scenario(ScenarioStep::Previous, cx);
    }

    fn cycle_scenario(&mut self, step: ScenarioStep, cx: &mut Context<Self>) {
        let scenarios = self.registry[self.selected_screen].scenarios();
        let current = scenarios
            .iter()
            .position(|scenario| *scenario == self.selected_scenario)
            .unwrap_or(0);
        let next = match step {
            ScenarioStep::Next => (current + 1) % scenarios.len(),
            ScenarioStep::Previous => current.checked_sub(1).unwrap_or(scenarios.len() - 1),
        };
        self.selected_scenario = scenarios[next].to_string();
        self.reset_preview_clock(cx);
        cx.notify();
    }

    fn preview_clock(&self) -> PreviewClock {
        PreviewClock {
            elapsed: self.preview_started.elapsed(),
            frame_index: self.frame_index,
        }
    }

    fn reset_preview_clock(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.preview_started = now;
        self.last_frame = now;
        self.frame_index = 0;
        self.registry[self.selected_screen].reset(&self.selected_scenario, cx);
        self.stats = StatsSession::new(FRAME_INTERVAL);
        self.stats_stream = StatsStreamState::default();
        self.close_requested = false;
    }

    fn advance_frame(&mut self, cx: &mut Context<Self>) {
        if self.close_requested {
            return;
        }

        let now = Instant::now();
        let frame_delta = now.duration_since(self.last_frame);
        self.last_frame = now;
        self.frame_index = self.frame_index.wrapping_add(1);

        if let Some(frame) = self.registry[self.selected_screen].advance(
            &self.selected_scenario,
            self.preview_clock(),
            frame_delta,
            cx,
        ) {
            let frame = self.stats.record_frame(frame);
            if let Err(error) = self.stream_frame(&frame) {
                self.fail_and_close(&error);
                return;
            }
        }

        if self.bounds_reached() {
            if let Err(error) = self.stream_final_aggregates() {
                self.fail_and_close(&error);
                return;
            }
            if self.exit_on_bound {
                self.close_requested = true;
            }
        }
    }

    fn bounds_reached(&self) -> bool {
        exit_bounds_reached(
            self.frame_index,
            self.preview_started.elapsed(),
            self.frame_bound,
            self.duration_bound,
        )
    }

    fn stream_frame(&mut self, frame: &FrameRecord) -> io::Result<()> {
        self.stream_stats_record(frame)
    }

    fn stream_final_aggregates(&mut self) -> io::Result<()> {
        if self.stats_stream.final_aggregates == FinalAggregatesState::Streamed {
            return Ok(());
        }
        self.stats_stream.final_aggregates = FinalAggregatesState::Streamed;

        let aggregates = self.stats.aggregates();
        self.stream_stats_record(&aggregates)
    }

    fn stream_stats_record(&mut self, record: &impl Serialize) -> io::Result<()> {
        if self.stats_format != Some(StatsFormat::Json)
            || self.stats_stream.output == StatsOutputState::Closed
        {
            return Ok(());
        }

        match write_json_line(record) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                self.stats_stream.output = StatsOutputState::Closed;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn fail_and_close(&mut self, error: &io::Error) {
        *lock_or_recover(&self.error_sink) = Some(format!("failed to stream debug stats: {error}"));
        self.close_requested = true;
    }

    fn render_screen_tabs(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let selected_screen = self.selected_screen;
        self.registry
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
                        rgb(0x001f_2937)
                    } else {
                        rgb(0x0011_1827)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(0x0060_a5fa)
                    } else {
                        rgb(0x0037_4151)
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
                            .text_color(rgb(0x009c_a3af))
                            .child(component.description()),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn render_scenario_picker(
        scenario_rows: &[registry::ScenarioRow],
        scenario: &str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        scenario_rows
            .iter()
            .map(|row| {
                let row_active = row
                    .chips
                    .iter()
                    .any(|chip| chip.matches.contains(&scenario));
                let chips = row
                    .chips
                    .iter()
                    .map(|chip| {
                        let selected = chip.matches.contains(&scenario);
                        let activates = chip.activates;
                        let (bg, border, text) = if selected {
                            (0x001d_4ed8, 0x0060_a5fa, 0x00f9_fafb)
                        } else if row_active {
                            (0x0011_1827, 0x0037_4151, 0x00d1_d5db)
                        } else {
                            (0x000b_1020, 0x001f_2937, 0x006b_7280)
                        };

                        div()
                            .id(format!("debug-scenario-{}-{}", row.label, chip.label))
                            .rounded_sm()
                            .px(px(10.0))
                            .py(px(5.0))
                            .cursor_pointer()
                            .text_sm()
                            .border_1()
                            .border_color(rgb(border))
                            .bg(rgb(bg))
                            .text_color(rgb(text))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !selected {
                                    this.select_scenario(activates, cx);
                                }
                            }))
                            .child(chip.label)
                    })
                    .collect::<Vec<_>>();

                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(64.0))
                            .text_xs()
                            .text_color(rgb(0x006b_7280))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(row.label.to_uppercase()),
                    )
                    .child(div().flex().gap_2().flex_wrap().children(chips))
                    .into_any_element()
            })
            .collect()
    }

    fn render_scenario_stats(
        scenario_rows: &[registry::ScenarioRow],
        scenario: &str,
    ) -> Vec<AnyElement> {
        scenario_rows
            .iter()
            .map(|row| {
                let value = row
                    .chips
                    .iter()
                    .find(|chip| chip.matches.contains(&scenario))
                    .map_or("—", |chip| chip.label);

                stat_block(
                    row.label,
                    value,
                    StatBlockOptions::fixed(scenario_stat_width(row.label)),
                )
                .into_any_element()
            })
            .collect()
    }

    fn render_sidebar(screen_tabs: Vec<AnyElement>) -> AnyElement {
        div()
            .w(px(280.0))
            .h_full()
            .border_r_1()
            .border_color(rgb(0x001f_2937))
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(concat!(env!("DICTATE_DISPLAY_NAME"), " debug")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x009c_a3af))
                    .child("Press q to close."),
            )
            .children(screen_tabs)
            .into_any_element()
    }

    fn render_live_stats(
        scenario_stats: Vec<AnyElement>,
        stats_frame_count: u64,
        measured_fps: f64,
        gate_state: &str,
        gate_open: bool,
    ) -> AnyElement {
        stats_row()
            .children(scenario_stats)
            .child(stat_block(
                "frames",
                stats_frame_count.to_string(),
                StatBlockOptions::fixed(96.0).tabular(),
            ))
            .child(stat_block(
                "fps",
                format!("{measured_fps:.1}"),
                StatBlockOptions::fixed(96.0).unit("fps").tabular(),
            ))
            .child(stat_block(
                "gate",
                gate_state,
                StatBlockOptions::fixed(96.0).value_color(if gate_open {
                    0x0060_a5fa
                } else {
                    0x009c_a3af
                }),
            ))
            .into_any_element()
    }
}

fn exit_bounds_reached(
    frame_index: u64,
    elapsed: Duration,
    frame_bound: Option<u64>,
    duration_bound: Option<Duration>,
) -> bool {
    frame_bound.is_some_and(|frames| frame_index >= frames)
        || duration_bound.is_some_and(|duration| elapsed >= duration)
}

fn write_json_line(record: &impl Serialize) -> io::Result<()> {
    let line = serde_json::to_string(record).map_err(io::Error::other)?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{line}")?;
    stdout.flush()
}

fn scenario_stat_width(label: &str) -> f32 {
    if label == "scenario" { 150.0 } else { 110.0 }
}

impl Drop for DebugWindow {
    fn drop(&mut self) {
        if let Err(error) = self.stream_final_aggregates() {
            *lock_or_recover(&self.error_sink) =
                Some(format!("failed to stream debug stats: {error}"));
        }
    }
}

impl Render for DebugWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.close_requested {
            window.remove_window();
        }

        let screen_tabs = self.render_screen_tabs(cx);

        let component = &self.registry[self.selected_screen];
        let scenario = self.selected_scenario.as_str();
        let scenario_rows = component.scenario_rows();
        let latest_frame = self.stats.latest_frame();
        let preview = component.preview(scenario, window, cx);
        let stats_frame_count = self.stats.frame_count();
        let stats_elapsed = self.stats.elapsed();
        let measured_fps = if stats_elapsed.is_zero() {
            0.0
        } else {
            let counted_frames =
                u32::try_from(stats_frame_count).map_or(f64::from(u32::MAX), f64::from);
            counted_frames / stats_elapsed.as_secs_f64()
        };
        let gate_open = latest_frame.is_some_and(|frame| frame.gate_state.is_open());
        let gate_state = if gate_open { "open" } else { "closed" };
        let scenario_picker = Self::render_scenario_picker(&scenario_rows, scenario, cx);
        let scenario_stats = Self::render_scenario_stats(&scenario_rows, scenario);

        div()
            .on_action(cx.listener(|this, _: &CloseDebugWindow, window, _cx| {
                if let Err(error) = this.stream_final_aggregates() {
                    this.fail_and_close(&error);
                }
                window.remove_window();
            }))
            .on_action(cx.listener(Self::select_next_scenario))
            .on_action(cx.listener(Self::select_previous_scenario))
            .track_focus(&self.focus_handle)
            .flex()
            .size_full()
            .bg(rgb(0x0003_0712))
            .text_color(rgb(0x00f9_fafb))
            .child(Self::render_sidebar(screen_tabs))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
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
                                    .text_color(rgb(0x00d1_d5db))
                                    .child(component.description()),
                            )
                            .child(div().flex().flex_col().gap_2().children(scenario_picker)),
                    )
                    .when(component.produces_stats(), |this| {
                        this.child(Self::render_live_stats(
                            scenario_stats,
                            stats_frame_count,
                            measured_fps,
                            gate_state,
                            gate_open,
                        ))
                    })
                    .child(div().flex_1().min_w_0().child(preview)),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::AnyElement;
    use gpui::App;
    use serde_json::Value;

    use super::*;

    fn test_plan() -> TranscriptionPlan {
        TranscriptionPlan::new(
            dictate_speech::default_model(),
            dictate_speech::DictationContext::default(),
        )
    }

    fn test_plan_factory() -> PlanFactory {
        Arc::new(|| Ok(test_plan()))
    }

    fn unused_plan_factory() -> PlanFactory {
        Arc::new(|| panic!("transcription plan should not be created"))
    }

    fn parsed_list_json() -> Value {
        let json = list_json(unused_plan_factory()).expect("list JSON should render");
        serde_json::from_str(&json).expect("list JSON should parse")
    }

    fn screens_array(value: &Value) -> &[Value] {
        value.as_array().expect("list JSON should be an array")
    }

    fn screen_named<'a>(screens: &'a [Value], name: &str) -> &'a Value {
        screens
            .iter()
            .find(|screen| screen["name"] == name)
            .expect("named screen should be registered")
    }

    fn result_error<T>(result: Result<T>) -> anyhow::Error {
        match result {
            Ok(_) => panic!("operation should fail"),
            Err(error) => error,
        }
    }

    #[test]
    fn list_json_parses_and_enumerates_registry() {
        let parsed = parsed_list_json();
        let screens = screens_array(&parsed);
        let registry = registry::registry(unused_plan_factory());

        assert_eq!(screens.len(), registry.len());
        for (screen, component) in screens.iter().zip(registry) {
            assert_eq!(screen["name"], component.name());
            assert_eq!(screen["description"], component.description());
            let scenarios = screen["scenarios"]
                .as_array()
                .expect("screen scenarios should be an array");
            assert_eq!(scenarios.len(), component.scenarios().len());
            for (scenario, expected) in scenarios.iter().zip(component.scenarios()) {
                assert_eq!(scenario, expected);
            }
        }
    }

    #[test]
    fn list_json_includes_shipped_scenarios() {
        let parsed = parsed_list_json();
        let screens = screens_array(&parsed);
        let overlay = screen_named(screens, "overlay");
        let bench = screen_named(screens, "bench");

        assert_eq!(
            overlay["scenarios"],
            serde_json::json!([
                "recording-sine",
                "recording-constant",
                "recording-frames",
                "recording-live",
                "transcribing",
                "pending-transcript",
                "insertion-uncertain",
                "delivery-failed",
                "no-transcript",
                "nothing-to-paste"
            ])
        );
        assert_eq!(
            bench["scenarios"],
            serde_json::json!(["spoken-commands", "cmu-arctic", "ljspeech"])
        );
    }

    #[test]
    fn unknown_screen_errors() {
        let error =
            result_error(resolve_selection(Some("nope"), None, unused_plan_factory())).to_string();

        assert!(error.contains("unknown debug screen"));
        assert!(error.contains("nope"));
    }

    #[test]
    fn list_ignores_invalid_selection_flags() {
        run(
            &Args {
                list: true,
                screen: Some("nope".to_string()),
                scenario: None,
                stats: None,
                duration: None,
                frames: None,
                exit: false,
            },
            || panic!("list mode must not create a transcription plan"),
        )
        .expect("list mode should ignore invalid selection");
    }

    #[test]
    fn unknown_scenario_errors() {
        let error = result_error(resolve_selection(
            Some("overlay"),
            Some("nope"),
            unused_plan_factory(),
        ))
        .to_string();

        assert!(error.contains("unknown scenario"));
        assert!(error.contains("nope"));
        assert!(error.contains("overlay"));
    }

    #[test]
    fn registry_validation_rejects_empty_scenarios() {
        let registry: Vec<Box<dyn DebugComponent>> = vec![Box::new(EmptyScenarioScreen)];
        let error = result_error(validate_registry(&registry)).to_string();

        assert!(error.contains("must define at least one scenario"));
        assert!(error.contains("empty"));
    }

    #[test]
    fn selection_defaults_to_first_scenario_for_selected_screen() {
        let selection = resolve_selection(Some("overlay"), None, unused_plan_factory())
            .expect("overlay selection should resolve without creating a transcription plan");

        assert_eq!(selection.screen, "overlay");
        assert_eq!(selection.scenario, "recording-sine");
    }

    #[test]
    fn stats_are_rejected_for_bench_screen() {
        let error = run(
            &Args {
                list: false,
                screen: Some("bench".to_string()),
                scenario: None,
                stats: Some(StatsFormat::Json),
                duration: None,
                frames: None,
                exit: false,
            },
            || Ok(test_plan()),
        )
        .map_or_else(
            |error| error.to_string(),
            |()| panic!("bench stats should fail"),
        );

        assert!(error.contains("--stats is only supported"));
    }

    #[test]
    fn exit_bounds_use_preview_state_without_overlay_stats() {
        assert!(!exit_bounds_reached(
            9,
            Duration::from_millis(90),
            Some(10),
            Some(Duration::from_millis(100)),
        ));
        assert!(exit_bounds_reached(
            10,
            Duration::from_millis(90),
            Some(10),
            Some(Duration::from_millis(100)),
        ));
        assert!(exit_bounds_reached(
            9,
            Duration::from_millis(100),
            Some(10),
            Some(Duration::from_millis(100)),
        ));
    }

    #[test]
    fn registry_validation_rejects_scenario_rows_referencing_unknown_ids() {
        let registry: Vec<Box<dyn DebugComponent>> = vec![Box::new(DriftingRowScreen)];
        let error = result_error(validate_registry(&registry)).to_string();

        assert!(error.contains("unknown scenario \"typo\""));
    }

    #[test]
    fn registry_validation_rejects_scenarios_missing_from_rows() {
        let registry: Vec<Box<dyn DebugComponent>> = vec![Box::new(MissingChipScreen)];
        let error = result_error(validate_registry(&registry)).to_string();

        assert!(error.contains("\"second\" is not activatable"));
    }

    #[test]
    fn shipped_registry_passes_validation() {
        validate_registry(&registry::registry(test_plan_factory()))
            .expect("shipped registry should validate");
    }

    struct DriftingRowScreen;

    impl DebugComponent for DriftingRowScreen {
        fn name(&self) -> &'static str {
            "drifting"
        }

        fn description(&self) -> &'static str {
            "scenario row drift test screen"
        }

        fn scenarios(&self) -> &'static [&'static str] {
            &["real"]
        }

        fn scenario_rows(&self) -> Vec<registry::ScenarioRow> {
            vec![registry::ScenarioRow {
                label: "scenario",
                chips: vec![registry::ScenarioChip {
                    label: "real",
                    activates: "real",
                    matches: vec!["typo"],
                }],
            }]
        }

        fn preview(&self, _scenario: &str, _window: &mut Window, _cx: &mut App) -> AnyElement {
            div().into_any_element()
        }
    }

    struct MissingChipScreen;

    impl DebugComponent for MissingChipScreen {
        fn name(&self) -> &'static str {
            "missing"
        }

        fn description(&self) -> &'static str {
            "missing chip test screen"
        }

        fn scenarios(&self) -> &'static [&'static str] {
            &["first", "second"]
        }

        fn scenario_rows(&self) -> Vec<registry::ScenarioRow> {
            vec![registry::ScenarioRow {
                label: "scenario",
                chips: vec![registry::ScenarioChip {
                    label: "first",
                    activates: "first",
                    matches: vec!["first"],
                }],
            }]
        }

        fn preview(&self, _scenario: &str, _window: &mut Window, _cx: &mut App) -> AnyElement {
            div().into_any_element()
        }
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

        fn preview(&self, _scenario: &str, _window: &mut Window, _cx: &mut App) -> AnyElement {
            div().into_any_element()
        }
    }
}
