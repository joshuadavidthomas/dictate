use adw::prelude::*;
use relm4::prelude::*;


pub struct ReflectionDialog {
    intention: String,
    reflection: Option<Reflection>
}

#[derive(Debug, Clone, Copy)]
pub enum Reflection {
    Achieved,
    Focused,
    Distracted,
    NoComment
}

#[derive(Debug, Clone)]
pub struct ReflectionResult {
    pub reflection: Reflection,
    pub next_block_intention: Option<String>
}

#[derive(Debug, Clone, Copy)]
pub enum NextBlockIntention {
    Keep,
    New
}

#[derive(Debug)]
pub enum Message {
    SetReflection(Reflection),
    Close(NextBlockIntention)
}

#[relm4::component(pub)]
impl Component for ReflectionDialog
{
    type Init = String;

    type Input = Message;
    type Output = ReflectionResult;
    type CommandOutput = ();

    view! {
        #[root]
        root = adw::Window {
            set_hide_on_close: false,
            set_resizable: false,
            set_modal: true,

            connect_close_request[sender] => move |_| {
                _ = sender.output(ReflectionResult {
                    reflection: Reflection::NoComment,
                    next_block_intention: None
                });

                adw::glib::Propagation::Proceed
            },

            add_css_class: "reflection",

            #[transition = "SlideLeftRight"]
            match model.reflection {
                None => gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        add_css_class: "margin20",

                        gtk::Label {
                            set_label: &model.intention,
                            set_css_classes: &["intention"],

                            set_halign: gtk::Align::Center,
                            set_wrap: true,
                            set_max_width_chars: 35
                        },
                    },

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Label {
                            set_label: "How did it go?"
                        },

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            add_css_class: "margin-v-10",
                            add_css_class: "linked",

                            gtk::Button {
                                set_label: "I achieved it",
                                set_css_classes: &["suggested-action", "raised"],
                                connect_clicked =>
                                    Message::SetReflection(Reflection::Achieved),
                            },

                            gtk::Button {
                                set_label: "I focused on it",
                                set_css_classes: &["raised"],
                                connect_clicked =>
                                    Message::SetReflection(Reflection::Focused),
                            },

                            gtk::Button {
                                set_label: "I was distracted",
                                set_css_classes: &["raised"],
                                connect_clicked =>
                                    Message::SetReflection(Reflection::Distracted),
                            },
                        },

                        gtk::Button {
                            set_label: "I'd rather not say",
                            set_css_classes: &["flat"],
                            connect_clicked =>
                                Message::SetReflection(Reflection::NoComment),
                        }
                    }
                },

                Some(Reflection::Achieved) => gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_valign: gtk::Align::Start,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        add_css_class: "margin20",

                        gtk::Label {
                            set_label: "Excellent work!",
                            set_css_classes: &["intention"],

                            set_halign: gtk::Align::Center,
                            set_wrap: true,
                            set_max_width_chars: 35
                        },
                    },

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        add_css_class: "margin-v-10",

                        gtk::Button {
                            set_label: "Enjoy your break",
                            set_css_classes: &["suggested-action", "raised"],
                            connect_clicked => Message::Close(NextBlockIntention::New),
                        }
                    }
                }

                Some(Reflection::Focused) => gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_valign: gtk::Align::Start,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        add_css_class: "margin20",

                        gtk::Label {
                            set_label: "Nicely done!",
                            set_css_classes: &["intention"],

                            set_halign: gtk::Align::Center,
                            set_wrap: true,
                            set_max_width_chars: 35
                        },
                    },

                    gtk::Label {
                        set_label: "Would you like to keep the same intention after the break?",
                        set_max_width_chars: 15,
                        set_halign: gtk::Align::Center,
                        set_wrap: true,
                    },

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        add_css_class: "margin-v-10",
                        add_css_class: "linked",

                        gtk::Button {
                            set_label: "Yeah, sounds good",
                            set_css_classes: &["suggested-action", "raised"],
                            connect_clicked => Message::Close(NextBlockIntention::Keep),
                        },

                        gtk::Button {
                            set_label: "No, I'll set a new intention myself",
                            set_css_classes: &["raised"],
                            connect_clicked => Message::Close(NextBlockIntention::New),
                        }
                    }
                }

                Some(Reflection::Distracted) | _ => gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_valign: gtk::Align::Start,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        add_css_class: "margin20",

                        gtk::Label {
                        set_label: "It's not how many you win, it's how many you show up for.",
                            set_css_classes: &["intention"],

                            set_halign: gtk::Align::Center,
                            set_wrap: true,
                            set_max_width_chars: 35
                        },
                    },

                    gtk::Label {
                        set_label: "Would you like to keep the same intention after the break?",
                        set_max_width_chars: 25,
                        set_halign: gtk::Align::Center,
                        set_wrap: true,
                    },

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        add_css_class: "margin-v-10",
                        add_css_class: "linked",

                        gtk::Button {
                            set_label: "Yeah, sounds good",
                            set_css_classes: &["suggested-action", "raised"],
                            connect_clicked => Message::Close(NextBlockIntention::Keep),
                        },

                        gtk::Button {
                            set_label: "No, I'll set a new intention myself",
                            set_css_classes: &["raised"],
                            connect_clicked => Message::Close(NextBlockIntention::New),
                        }
                    }
                }
            }
        }
    }

    fn init(intention: Self::Init, root: Self::Root, sender: ComponentSender<Self>)
        -> ComponentParts<Self>
    {
        let model = Self {
            intention,
            reflection: None
        };

        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root)
    {
        use Message::*;

        match msg {
            Close(nbi) => {
                let reflection = self.reflection
                    .unwrap_or(Reflection::NoComment);

                let next_block_intention = match nbi {
                    NextBlockIntention::Keep => Some(self.intention.clone()),
                    NextBlockIntention::New => None,
                };

                _ = sender.output(ReflectionResult {
                    reflection,
                    next_block_intention
                });

                root.destroy();
            }

            SetReflection(Reflection::NoComment) => {
                let reflection = self.reflection
                    .unwrap_or(Reflection::NoComment);

                _ = sender.output(ReflectionResult {
                    reflection,
                    next_block_intention: None
                });

                root.destroy();
            }

            SetReflection(r) => self.reflection = Some(r),
        }
    }
}
