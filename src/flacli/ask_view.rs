//! The Ask flacli page: a console for any flacli command, for the cases the buttons do not
//! cover. What is typed after `flacli` runs as typed, with `--compact`, and the answer is shown
//! as flacli gives it. The command list and the agent guide are one click away.

use adw::prelude::*;
use gtk::{glib, glib::clone};
use std::rc::Rc;

use super::{controller::run_raw, get_view::show_sidebar_button};
use crate::window::EuphonicaWindow;

const INTRO: &str = "Everything the other pages do is a flacli command, and so is everything they do not. Type what would follow `flacli` on the command line and press Enter. The answer comes back as flacli gives it, JSON where it speaks JSON. Commands lists them all; Guide is the agent guide, with the rules.";

/// A shell-like split: spaces separate, single or double quotes group, a backslash escapes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut have = false;
    for c in line.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            have = true;
            continue;
        }
        match (c, quote) {
            ('\\', _) => escaped = true,
            (q @ ('"' | '\''), None) => {
                quote = Some(q);
                have = true;
            }
            (q, Some(open)) if q == open => quote = None,
            (' ' | '\t', None) => {
                if have {
                    out.push(std::mem::take(&mut current));
                    have = false;
                }
            }
            _ => {
                current.push(c);
                have = true;
            }
        }
    }
    if have {
        out.push(current);
    }
    out
}

impl std::fmt::Debug for AskView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AskView")
    }
}

pub struct AskView {
    pub widget: adw::ToolbarView,
    entry: gtk::Entry,
    output: gtk::TextView,
    spinner: gtk::Spinner,
    run_btn: gtk::Button,
}

impl AskView {
    pub fn new(window: &EuphonicaWindow) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        header.pack_start(&show_sidebar_button(window));
        header.set_title_widget(Some(&adw::WindowTitle::new("Ask flacli", "")));
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        header.pack_end(&spinner);
        let guide_btn = gtk::Button::builder().label("Guide").tooltip_text("flacli guide: the workflow, the commands, the rules").build();
        let commands_btn = gtk::Button::builder().label("Commands").tooltip_text("flacli --help").build();
        let clear_btn = gtk::Button::builder()
            .icon_name("edit-clear-all-symbolic")
            .tooltip_text("Clear the console")
            .build();
        header.pack_end(&clear_btn);
        header.pack_end(&guide_btn);
        header.pack_end(&commands_btn);

        let intro = gtk::Label::builder()
            .label(INTRO)
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let prompt = gtk::Label::builder()
            .label("flacli")
            .css_classes(["monospace", "dim-label"])
            .valign(gtk::Align::Center)
            .build();
        let entry = gtk::Entry::builder()
            .placeholder_text("status · status 1 · skip 1 --remaining · tidy --new · get \"Artist - Title\" · doctor")
            .hexpand(true)
            .css_classes(["monospace"])
            .build();
        let run_btn = gtk::Button::builder()
            .label("Run")
            .css_classes(["suggested-action"])
            .build();
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&prompt);
        line.append(&entry);
        line.append(&run_btn);

        let output = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        let pane = gtk::ScrolledWindow::builder()
            .child(&output)
            .vexpand(true)
            .css_classes(["card"])
            .build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .vexpand(true)
            .build();
        content.append(&intro);
        content.append(&line);
        content.append(&pane);
        let clamp = adw::Clamp::builder().maximum_size(960).child(&content).vexpand(true).build();
        let widget = adw::ToolbarView::new();
        widget.add_top_bar(&header);
        widget.set_content(Some(&clamp));

        let this = Rc::new(Self {
            widget,
            entry,
            output,
            spinner,
            run_btn,
        });
        let hook = |button: &gtk::Button, view: &Rc<Self>, line: &'static str| {
            let view = Rc::downgrade(view);
            button.connect_clicked(move |_| {
                if let Some(view) = view.upgrade() {
                    view.run_line(line);
                }
            });
        };
        hook(&guide_btn, &this, "guide");
        hook(&commands_btn, &this, "--help");
        this.run_btn.connect_clicked(clone!(
            #[weak(rename_to = view)]
            this,
            move |_| view.run_typed()
        ));
        this.entry.connect_activate(clone!(
            #[weak(rename_to = view)]
            this,
            move |_| view.run_typed()
        ));
        clear_btn.connect_clicked(clone!(
            #[weak(rename_to = view)]
            this,
            move |_| view.output.buffer().set_text("")
        ));
        this
    }

    pub fn focus(&self) {
        self.entry.grab_focus();
    }

    fn append(&self, text: &str) {
        let buffer = self.output.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, text);
        let mark = buffer.create_mark(None, &buffer.end_iter(), false);
        self.output.scroll_mark_onscreen(&mark);
    }

    fn run_typed(self: &Rc<Self>) {
        let line = self.entry.text().to_string();
        if line.trim().is_empty() {
            return;
        }
        self.entry.set_text("");
        self.run_line_owned(line);
    }

    fn run_line(self: &Rc<Self>, line: &str) {
        self.run_line_owned(line.to_owned());
    }

    fn run_line_owned(self: &Rc<Self>, line: String) {
        let args = split_args(&line);
        if args.is_empty() {
            return;
        }
        self.append(&format!("$ flacli {}\n", line.trim()));
        self.spinner.set_visible(true);
        self.run_btn.set_sensitive(false);
        let view = self.clone();
        glib::spawn_future_local(async move {
            let text = match run_raw(args).await {
                Ok(answer) => answer.pretty(),
                Err(e) => format!("could not run flacli: {e}"),
            };
            view.append(&format!("{}\n\n", text.trim_end()));
            view.spinner.set_visible(false);
            view.run_btn.set_sensitive(true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_split_like_a_shell() {
        assert_eq!(split_args("status 1"), vec!["status", "1"]);
        assert_eq!(split_args(r#"get "Lorde - Royals" 'Boards of Canada - Geogaddi (album)'"#), vec!["get", "Lorde - Royals", "Boards of Canada - Geogaddi (album)"]);
        assert_eq!(split_args(r#"get {"kind":"track","title":"a b"}"#), vec!["get", r#"{kind:track,title:a b}"#]);
        assert_eq!(split_args("  skip   1  --remaining "), vec!["skip", "1", "--remaining"]);
        assert_eq!(split_args(r"say it\ so"), vec!["say", "it so"]);
        assert!(split_args("   ").is_empty());
    }
}
