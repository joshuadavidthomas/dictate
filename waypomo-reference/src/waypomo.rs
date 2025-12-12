use std::time::Duration;
use std::fmt;

use chrono::TimeDelta;

use compact_str::{
    CompactString,
    format_compact
};

use adw::prelude::*;

use relm4::{
    prelude::*,
    actions::{
        RelmActionGroup,
        RelmAction,
    },
    WorkerController
};

use gtk4_layer_shell::{
    Edge, Layer, LayerShell
};


use crate::{
    workers::{
        timer::*,
        subprocess::*
    },

    dialogs::*,

    config::{
        WaypomoConfig,
        TimeBlockConfig,
        StartIn
    },

    report::*,
    time_block::*
};


#[derive(Debug, Clone)]
pub enum State {
    Work,

    // used for displaying the reflection window and/or waiting for the completion handler to
    // finish
    PreBreak,
    Break,

    PreWork,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result
    {
        match &self {
            State::Work => write!(f, "work"),
            State::PreBreak => write!(f, "pre-break"),
            State::Break  => write!(f, "break"),
            State::PreWork => write!(f, "pre-work"),
        }
    }
}

pub struct Waypomo {
    conf: WaypomoConfig,
    sender: ComponentSender<Self>,
    root: gtk::Window,

    // underscore because we never read from this and otherwise rustc complains that it's dead code
    _timer: Option<WorkerController<Timer>>,
    completion_worker: Option<WorkerController<Subprocess>>,

    time_block: Option<TimeBlock>,
    remaining_formatted: CompactString,
    completion: f32,

    state: State,
    intention: Option<String>,
    paused: bool,

    num_work_blocks: usize,

    last_tick: chrono::DateTime<chrono::Local>,

    staged_reflection: Option<String>,
    reflect_dlg: Option<Controller<ReflectionDialog>>,
    motivation_dlg: Option<Controller<MotivationDialog>>,
    intent_dlg: Option<Controller<IntentionPromptDialog>>,

    staged_report: Option<PomodoroReport>
}

#[derive(Debug)]
pub enum Message {
    Tick,
    Reset,
    PauseToggle,

    Reflection(ReflectionResult),
    MotivationClosed,

    OpenIntentionPrompt,
    SetIntention(IntentionPromptResult),

    ChangeToOppositeCorner,

    CompletionWorkerDone(State),
}

relm4::new_action_group!(WaypomoGroup, "waypomo");

relm4::new_stateless_action!(ResetAction, WaypomoGroup, "reset");
relm4::new_stateless_action!(IntentionAction, WaypomoGroup, "intention");
relm4::new_stateless_action!(OppositeCornerAction, WaypomoGroup, "corner");

relm4::new_stateless_action!(QuitAction, WaypomoGroup, "quit");

impl Waypomo
{
    fn setup_actions(sender: &ComponentSender<Self>) -> RelmActionGroup<WaypomoGroup>
    {
        let mut actions = RelmActionGroup::<WaypomoGroup>::new();

        actions.add_action({
            let sender = sender.clone();
            RelmAction::<ResetAction>::new_stateless(move |_| {
                sender.input(Message::Reset);
            })
        });

        actions.add_action({
            let sender = sender.clone();
            RelmAction::<IntentionAction>::new_stateless(move |_| {
                sender.input(Message::OpenIntentionPrompt);
            })
        });

        actions.add_action({
            let sender = sender.clone();
            RelmAction::<OppositeCornerAction>::new_stateless(move |_| {
                sender.input(Message::ChangeToOppositeCorner);
            })
        });

        actions.add_action({
            RelmAction::<QuitAction>::new_stateless(move |_| {
                relm4::main_application().quit();
            })
        });

        actions
    }

    fn setup_layer_shell(&self, w: &gtk::Window)
    {
        let a = self.conf.display.timer_position.anchors();

        w.set_anchor(Edge::Top,    a.top);
        w.set_anchor(Edge::Bottom, a.bottom);
        w.set_anchor(Edge::Right,  a.right);
        w.set_anchor(Edge::Left,   a.left);
    }

    #[inline]
    fn starting_state(conf: &WaypomoConfig) -> (State, TimeBlockConfig)
    {
        let (state, tbc) = match conf.pomodoro.start_in {
            StartIn::Work       => (State::Work,  &conf.pomodoro.work),
            StartIn::ShortBreak => (State::Break, &conf.pomodoro.short_break),
            StartIn::LongBreak  => (State::Break, &conf.pomodoro.long_break)
        };

        (state, tbc.clone())
    }

    fn open_reflection(&mut self, intention: String)
    {
        let reflect_dlg = ReflectionDialog::builder()
            .transient_for(&self.root)
            .launch(intention)
            .forward(self.sender.input_sender(), Message::Reflection);

        reflect_dlg.widget().present();
        self.reflect_dlg = Some(reflect_dlg);
    }

    fn open_motivation(&mut self)
    {
        let motivation_dlg = MotivationDialog::builder()
            .transient_for(&self.root)
            .launch(())
            .forward(self.sender.input_sender(),
                |_| Message::MotivationClosed);

        motivation_dlg.widget().present();
        self.motivation_dlg = Some(motivation_dlg);
    }

    fn timeblock_start(&mut self, conf: TimeBlockConfig)
    {
        self.completion = 0.0;
        self.paused = false;

        self.time_block = Some(TimeBlock::new_with_conf(conf));
    }

    fn cleanup_completion_worker(&mut self)
    {
        self.completion_worker.take();
    }

    fn start_completion_worker(&mut self, next_state: State)
    {
        self.cleanup_completion_worker();

        let cmdline = self.time_block.as_ref().and_then(|x| x.conf.on_completion.as_deref());

        let Some(cmdline) = cmdline else {
            return;
        };

        let worker = Subprocess::builder()
            .detach_worker(cmdline.to_string())
            .forward(self.sender.input_sender(), move |msg| match msg {
                SubprocessMessage::Done(_res) =>
                    Message::CompletionWorkerDone(next_state.clone()),
            });

        self.completion_worker = Some(worker);
    }

    fn transition_to(&mut self, new_state: State)
    {
        let time_block = match (&mut self.state, &new_state) {
            (State::Break, State::Work) =>
				unreachable!("never transition immediately from break to work, use PreWork"),

            (State::Break, State::PreWork) => {
                self.start_completion_worker(State::Work);
                self.state = new_state;
                return;
            }

            (State::Work, State::PreBreak) => {
                // leaving time block around until the reflection delay has expired
                //
                // theoretically we should probably end/take the block here and use a separate
                // timeout mechanism for the delay but that's just going to be a pain in the ass,
                // so.

                let report = PomodoroReport::new_from_time_block(
                    self.time_block.as_ref()
                        .unwrap()
                        .clone(),
                    self.intention.take());

                self.staged_report = Some(report);

                self.start_completion_worker(State::Break);
                self.state = new_state;
                return;
            },

            (State::PreBreak, State::Break) => {
                self.cleanup_completion_worker();

                if let Some(report) = self.staged_report.take() {
                    println!(" >>>> {report:?}");

                    match (report.intention, report.reflection) {
                        (Some(_), Reflection::Achieved | Reflection::Focused) =>
                            self.num_work_blocks += 1,

                        _ => ()
                    }
                }

                if self.num_work_blocks >= self.conf.pomodoro.work_blocks_for_long_break {
                    self.num_work_blocks = 0;

                    &self.conf.pomodoro.long_break
                } else {
                    &self.conf.pomodoro.short_break
                }
            },

            (State::PreWork, State::Work) => {
                self.cleanup_completion_worker();
                &self.conf.pomodoro.work
            }

            _ => panic!("invalid state transition")
        };

        self.timeblock_start(time_block.clone());
        self.state = new_state;
    }

    fn timeblock_done(&mut self)
    {
        match &self.state {
            State::Break =>
                self.transition_to(State::PreWork),

            State::PreWork =>
                self.transition_to(State::Work),

            State::Work =>
                self.transition_to(State::PreBreak),

            State::PreBreak => {
                let Some(tb) = &self.time_block else {
                    return;
                };

                let Some(report) = &self.staged_report else {
                    return;
                };

                let delay = TimeDelta::zero()
                    - self.conf.pomodoro.reflection_prompt_delay
                        .unwrap_or(TimeDelta::zero());

                if tb.remaining < delay {
                    self.time_block.take();

                    if let Some(intention) = report.intention.clone() {
                        self.open_reflection(intention);
                    } else if self.conf.pomodoro.motivation_dialog_after_intentionless_block {
                        self.open_motivation();
                    } else if self.completion_worker.is_none() {
                        self.transition_to(State::Break);
                    }
                }
            }
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn tick(&mut self)
    {
        let now = chrono::offset::Local::now();

        let diff = now - self.last_tick;
        self.last_tick = now;

        if self.paused {
            return;
        }

        let Some(tb) = &mut self.time_block else {
            return
        };

        tb.remaining -= diff;

        if tb.remaining < TimeDelta::zero() {
            self.timeblock_done();
            return;
        }

        let rem = tb.remaining
            .min(tb.conf.duration)
            .max(chrono::TimeDelta::zero());

        self.completion = 1.0 -
            ((tb.remaining.num_milliseconds() as f32)
            / (tb.conf.duration.num_milliseconds() as f32));

        let min = rem.num_minutes();
        let sec = rem.num_seconds() - (min * 60);

        self.remaining_formatted =
            format_compact!("{:02}:{:02.0}", min, sec);
    }
}

#[allow(clippy::cast_possible_truncation)]
#[relm4::component(pub)]
impl Component for Waypomo
{
    type Init = WaypomoConfig;

    type Input = Message;
    type Output = ();
    type CommandOutput = ();


    view! {
        #[root]
        window = gtk::Window {
            set_title: Some("waypomo timer"),

            init_layer_shell: (),
            set_layer: Layer::Overlay,
            auto_exclusive_zone_enable: (),
            set_namespace: Some("waypomo"),

            set_css_classes: &["pomodoro-timer"],

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_spacing: 0,

                #[watch]
                set_css_classes: match (model.paused, &model.state, &model.intention) {
                    (true, _, _) => &["paused"],

                    (_, State::Work, None) =>
                        &["without-intention"],

                    (_, State::Work, Some(_)) =>
                        &["with-intention"],

                    (_, State::Break, _) =>
                        &["break"],

                    _ => &[],
                },

                add_controller = gtk::GestureClick {
                    set_button: 3,

                    connect_pressed[cpm, window] => move |_, _, x, _| {
                        let y = {
                            let a = &model.conf.display.timer_position.clone().anchors();

                            if a.bottom {
                                7
                            } else {
                                window.height() - 7
                            }
                        };

                        cpm.set_pointing_to(Some(
                            &gtk::gdk::Rectangle::new(x as i32, y, 1, 1)));
                        cpm.popup();
                    }
                },

                add_controller = gtk::GestureClick {
                    set_button: 1,

                    connect_pressed[sender] => move |_, _, _, _| {
                        sender.input(Message::PauseToggle);
                    }
                },

                #[name(cpm)]
                gtk::PopoverMenu::from_model(Some(&context_menu)),

                #[name(progress)]
                match &model.state {
                    State::PreBreak => gtk::ProgressBar {
                        set_margin_bottom: 0,
                        set_margin_top: 0,

                        set_fraction: 0.0,

                        // // i don't actually think this looks very good, but may revisit
                        // #[watch]
                        // pulse: ()
                    },

                    _ => gtk::ProgressBar {
                        #[watch]
                        set_fraction: match &model.state {
                            State::Break => f64::from(model.completion),
                            _ => 1.0 - f64::from(model.completion)
                        },

                        set_margin_bottom: 0,
                        set_margin_top: 0,

                        #[watch]
                        set_inverted: matches!(&model.state, State::Break),
                    },
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 10,

                    set_vexpand: true,
                    set_valign: gtk::Align::Center,
                    set_halign: gtk::Align::Center,

                    gtk::Label {
                        #[watch]
                        set_label: &model.remaining_formatted,

                        set_margin_top: 0,
                        set_css_classes: &["remaining"],
                    },
                }
            }
        }
    }

    menu! {
        context_menu: {
            "Reset current block" => ResetAction,
            "Set Intention" => IntentionAction,
            "Switch corner" => OppositeCornerAction,
            section! {
                "Quit" => QuitAction,
            }
        }
    }

    fn init(conf: Self::Init, root: Self::Root, sender: ComponentSender<Self>)
        -> ComponentParts<Self>
    {
        adw::StyleManager::default()
            .set_color_scheme(
                if conf.display.dark_mode {
                    adw::ColorScheme::PreferDark
                } else {
                    adw::ColorScheme::Default
                });

        let timer = Timer::builder()
            .detach_worker(Duration::from_millis(250))
            .forward(sender.input_sender(), |msg| match msg {
                TimerMessage:: Tick => Message::Tick
            });

        let model = {
            // this block is just here to reduce the amount of mutability we're carrying around

            let (state, starting_block_config) = Self::starting_state(&conf);

            let mut m = Self {
                conf,
                sender: sender.clone(),
                root: root.clone(),

                _timer: Some(timer),
                completion_worker: None,

                time_block: None,
                intention: None,

                state,
                paused: false,

                remaining_formatted: CompactString::new(""),
                completion: 0.0,

                num_work_blocks: 0,

                last_tick: chrono::offset::Local::now(),

                staged_reflection: None,
                reflect_dlg: None,
                motivation_dlg: None,
                intent_dlg: None,

                staged_report: None
            };

            m.timeblock_start(starting_block_config);

            m
        };

        let widgets = view_output!();

        model.setup_layer_shell(&widgets.window);

        Self::setup_actions(&sender)
            .register_for_widget(&widgets.window);

        // FIXME: do we need this?
        sender.input(Message::Tick);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root)
    {
        use Message::*;


        match msg {
            Tick => self.tick(),

            PauseToggle => {
                self.paused = !self.paused;
            }

            Reset => if let Some(tb) = &mut self.time_block {
                tb.reset();

                self.completion = 0.0;
                self.last_tick = chrono::offset::Local::now();
                self.paused = false;

                sender.input(Message::Tick);
            }

            Reflection(mut r) => {
                self.reflect_dlg.take();

                r.next_block_intention.take()
                    .map(|nbi| self.intention = Some(nbi));

                if let Some(report) = &mut self.staged_report {
                    report.reflection = r.reflection;
                }

                self.transition_to(State::Break);
            }

            MotivationClosed => {
                self.motivation_dlg.take();
                self.transition_to(State::Break);
            },

            OpenIntentionPrompt => {
                let intent_dlg = IntentionPromptDialog::builder()
                    .transient_for(root)
                    .launch(self.intention.clone())
                    .forward(sender.input_sender(), Message::SetIntention);

                intent_dlg.widget().present();
                self.intent_dlg = Some(intent_dlg);

                self.paused = true;
            }

            SetIntention(new_intent) => {
                self.intent_dlg.take();

                // FIXME: if we were paused when we opened the intention prompt, should we remain
                // paused here (rather than unconditionally unpausing)?
                self.paused = false;

                self.intention = match new_intent {
                    IntentionPromptResult::Cancelled
                        if self.intention.is_some() => return,

                    IntentionPromptResult::NewIntention(i) => Some(i),
                    _ => None,
                };
            }

            CompletionWorkerDone(state) => {
                let waiting_for_dialogs =
                    self.reflect_dlg.is_some()
                    || self.motivation_dlg.is_some()
                    || self.staged_reflection.is_some();

                if !waiting_for_dialogs {
                    self.transition_to(state);
                }
            }

            ChangeToOppositeCorner => {
                root.set_anchor(Edge::Right, !root.is_anchor(Edge::Right));
                root.set_anchor(Edge::Left, !root.is_anchor(Edge::Left));
            }
        }
    }
}
