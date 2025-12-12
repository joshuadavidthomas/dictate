use gtk::{
    gdk::{
        Key,
        ModifierType
    },
    glib::Propagation,
};

use adw::prelude::*;

use relm4::prelude::*;


pub struct IntentionPromptDialog {
    buffer: gtk::TextBuffer,
}

#[derive(Debug)]
pub enum IntentionPromptResult {
    NewIntention(String),
    EmptyIntention,
    Cancelled,
}

#[relm4::component(pub)]
impl Component for IntentionPromptDialog
{
    type Input = bool;
    type Output = IntentionPromptResult;
    type CommandOutput = ();

    type Init = Option<String>;

    view! {
        #[root]
        window = adw::Window {
            set_hide_on_close: false,
            set_resizable: false,
            set_modal: true,

            add_css_class: "intention-prompt",

            connect_close_request[sender] => move |_| {
                _ = sender.output(IntentionPromptResult::Cancelled);
                Propagation::Proceed
            },

            add_controller = gtk::EventControllerKey {
                connect_key_pressed[sender] => move |_, key, _, modifier| {
                    match (modifier, key) {
                        (_, Key::Escape) => {
                            sender.input(false);
                            Propagation::Stop
                        }

                        _ => Propagation::Proceed
                    }
                }
            },

            adw::ToolbarView {
                set_top_bar_style: adw::ToolbarStyle::Flat,
                set_bottom_bar_style: adw::ToolbarStyle::Raised,

                add_top_bar = &adw::HeaderBar {
                    set_show_end_title_buttons: false,

                    set_title_widget: Some(
                        &adw::WindowTitle::new("What is your intention?", ""))
                },

                #[wrap(Some)]
                set_content = &gtk::Frame {
                    set_margin_all: 20,
                    set_margin_bottom: 0,

                    add_css_class: "card",

                    gtk::ScrolledWindow {
                        set_min_content_height: 280,
                        set_margin_all: 10,

                        #[wrap(Some)]
                        set_child = &gtk::TextView {
                            set_buffer: Some(&model.buffer),
                            set_wrap_mode: gtk::WrapMode::Word,
                            set_accepts_tab: false,

                            add_controller = gtk::EventControllerKey {
                                connect_key_pressed[sender] => move |_, key, _, modifier| {
                                    match (modifier, key) {
                                        (ModifierType::CONTROL_MASK, Key::Return) => {
                                            sender.input(true);
                                            Propagation::Stop
                                        }

                                        _ => Propagation::Proceed
                                    }
                                }
                            },
                        },
                    }
                },

                add_bottom_bar = &gtk::ActionBar {
                    add_css_class: "toolbar",

                    pack_start = &gtk::Button {
                        set_label: "Cancel",
                        connect_clicked => false,
                    },

                    pack_end = &gtk::Button {
                        set_label: "Let's go",
                        connect_clicked => true,
                    }
                }
            }
        }
    }

    fn init(init: Self::Init, root: Self::Root, sender: ComponentSender<Self>)
        -> ComponentParts<Self>
    {
        let buffer = gtk::TextBuffer::new(None);

        if let Some(intent) = init {
            buffer.set_text(&intent);
        }

        let model = Self {
            buffer,
        };

        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root)
    {
        let res = if msg {
            let intention = self.buffer
                .text(&self.buffer.start_iter(), 
                    &self.buffer.end_iter(), false);

            let trimmed = intention.trim();

            if trimmed.is_empty() {
                IntentionPromptResult::EmptyIntention
            } else {
                IntentionPromptResult::NewIntention(trimmed.into())
            }
        } else {
            IntentionPromptResult::Cancelled
        };

        _ = sender.output(res);
        root.destroy();
    }
}
