//! The Get view: the search panel as a page of its own in the main stack, beside Albums and
//! Artists, with the pages switcher in its header.

use adw::prelude::*;
use gtk::{glib, glib::clone};
use std::rc::Rc;

use super::finder::Finder;
use crate::window::EuphonicaWindow;

impl std::fmt::Debug for GetView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GetView")
    }
}

pub struct GetView {
    pub widget: adw::ToolbarView,
    finder: Rc<Finder>,
}

/// A header-bar button that shows the sidebar while the split view is collapsed, as every
/// other view has.
pub(super) fn show_sidebar_button(window: &EuphonicaWindow) -> gtk::Button {
    let split_view = window.get_split_view();
    let button = gtk::Button::builder()
        .icon_name("dock-left-symbolic")
        .tooltip_text("Show sidebar")
        .visible(false)
        .build();
    split_view
        .bind_property("collapsed", &button, "visible")
        .sync_create()
        .build();
    button.connect_clicked(clone!(
        #[weak]
        split_view,
        move |_| split_view.set_show_sidebar(true)
    ));
    button
}

impl GetView {
    pub fn new(window: &EuphonicaWindow) -> Rc<Self> {
        let finder = Finder::new(window);
        let header = adw::HeaderBar::new();
        header.pack_start(&show_sidebar_button(window));
        header.set_title_widget(Some(&finder.switcher));
        let widget = adw::ToolbarView::new();
        widget.add_top_bar(&header);
        widget.set_content(Some(&finder.root));
        Rc::new(Self { widget, finder })
    }

    pub fn search_for(&self, term: &str) {
        self.finder.search_for(term);
    }

    pub fn focus(&self) {
        self.finder.focus_entry();
    }
}
