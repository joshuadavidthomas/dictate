use adw::prelude::*;
use relm4::prelude::*;


pub struct MotivationDialog;

#[relm4::component(pub)]
impl Component for MotivationDialog
{
    type Init = ();

    type Input = ();
    type Output = ();
    type CommandOutput = ();

    view! {
        #[root]
        root = adw::Window {
            set_hide_on_close: false,
            set_resizable: false,
            set_modal: true,

            add_css_class: "reflection",
            add_css_class: "motivation",

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_valign: gtk::Align::Start,

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    add_css_class: "margin20",

                    gtk::Label {
                        set_label: "Time waits for nobody.",
                        set_css_classes: &["intention"],

                        set_halign: gtk::Align::Center,
                        set_wrap: true,
                        set_max_width_chars: 35
                    },
                },

                gtk::Label {
                    set_label: "Consider setting an intention\nfor your next block!",
                    set_max_width_chars: 15,
                    set_halign: gtk::Align::Center,
                    set_justify: gtk::Justification::Center,
                    set_wrap: true,
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,

                    gtk::Button {
                        set_label: "Perhaps I will",
                        set_css_classes: &["suggested-action", "raised"],
                        connect_clicked => (),
                    }
                }
            }
        }
    }

    fn init(_: Self::Init, root: Self::Root, sender: ComponentSender<Self>)
        -> ComponentParts<Self>
    {
        let model = Self;
        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, _: Self::Input, sender: ComponentSender<Self>, root: &Self::Root)
    {
        println!("yea");
        _ = sender.output(());
        root.destroy();
    }
}
