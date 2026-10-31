use crate::audio::controls::ControlId;
use crate::audio::pw_worker::{PwWorker, RouteStatus};
use crate::presets::{model::Preset, store};
use gtk::glib::{ControlFlow, SignalHandlerId};
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

const SLIDER_IDS: &[(ControlId, &str)] = &[
    (ControlId::Bass, "Bass Boost"),
    (ControlId::Fidelity, "Fidelity"),
    (ControlId::Ambience, "Ambience"),
    (ControlId::Output, "Output"),
];

struct SliderRow {
    id: ControlId,
    scale: gtk::Scale,
    handler_id: SignalHandlerId,
}

pub fn build(app: &adw::Application, worker: Rc<PwWorker>, status_rx: mpsc::Receiver<RouteStatus>) {
    let presets = Rc::new(store::load_all());

    let header = gtk::HeaderBar::new();

    let banner = adw::Banner::new("Audio route status is unavailable.");
    banner.set_revealed(false);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let preset_names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    let preset_dropdown = gtk::DropDown::from_strings(&preset_names);
    content.append(&preset_dropdown);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let slider_rows: Rc<RefCell<Vec<SliderRow>>> = Rc::new(RefCell::new(Vec::new()));

    // Music remains the launch default rather than Flat / Reference. Find it
    // by name because preset order can shift once user presets exist.
    let initial_index = presets.iter().position(|p| p.name == "Music").unwrap_or(0);
    let initial = presets.get(initial_index).cloned();
    preset_dropdown.set_selected(initial_index as u32);

    for &(id, label) in SLIDER_IDS {
        let initial_value = initial.as_ref().map(|p| slider_value(p, id)).unwrap_or(0.5);
        let (row_widget, row) = build_slider_row(id, label, initial_value, worker.clone());
        content.append(&row_widget);
        slider_rows.borrow_mut().push(row);
    }

    {
        let worker = worker.clone();
        let presets = presets.clone();
        let slider_rows = slider_rows.clone();
        preset_dropdown.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let Some(preset) = presets.get(idx) else {
                return;
            };
            apply_preset_to_ui(preset, &slider_rows);
            worker.apply_preset(preset.clone());
        });
    }

    if let Some(preset) = initial {
        worker.apply_preset(preset);
    }

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.add_top_bar(&banner);
    toolbar_view.set_content(Some(&content));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("FxLinux")
        .default_width(420)
        .default_height(420)
        .content(&toolbar_view)
        .build();

    // status_rx is fed from the PipeWire worker thread; poll it here on the
    // GTK main loop rather than touching the banner from that thread (GTK
    // widgets aren't Send).
    gtk::glib::timeout_add_local(Duration::from_millis(300), move || {
        let mut last = None;
        while let Ok(status) = status_rx.try_recv() {
            last = Some(status);
        }
        if let Some(status) = last {
            match status {
                RouteStatus::EngineMissing => {
                    banner.set_title("Audio engine is not running. Start filter-chain.service.");
                    banner.set_revealed(true);
                }
                RouteStatus::OutputMissing => {
                    banner.set_title(
                        "FxLinux output stream is missing. Restart filter-chain.service.",
                    );
                    banner.set_revealed(true);
                }
                RouteStatus::OutputUnlinked => {
                    banner.set_title("FxLinux output is not connected to an audio sink.");
                    banner.set_revealed(true);
                }
                RouteStatus::OutputMisrouted => {
                    banner
                        .set_title("FxLinux output is connected, but not to a usable audio sink.");
                    banner.set_revealed(true);
                }
                RouteStatus::Healthy { .. } => banner.set_revealed(false),
            }
        }
        ControlFlow::Continue
    });

    window.present();
}

fn slider_value(preset: &Preset, id: ControlId) -> f64 {
    match id {
        ControlId::Bass => preset.bass,
        ControlId::Fidelity => preset.fidelity,
        ControlId::Ambience => preset.ambience,
        ControlId::Output => preset.output,
    }
}

/// Moves each slider to match `preset` without re-triggering their own
/// `value-changed` handlers because the preset apply already pushes the full
/// parameter set in one batch, so a handler firing here would just resend a
/// redundant, partially-stale update.
fn apply_preset_to_ui(preset: &Preset, slider_rows: &Rc<RefCell<Vec<SliderRow>>>) {
    for row in slider_rows.borrow().iter() {
        row.scale.block_signal(&row.handler_id);
        row.scale.set_value(slider_value(preset, row.id));
        row.scale.unblock_signal(&row.handler_id);
    }
}

fn build_slider_row(
    id: ControlId,
    label: &str,
    initial_value: f64,
    worker: Rc<PwWorker>,
) -> (gtk::Box, SliderRow) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    let title = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .build();
    title.add_css_class("heading");

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    scale.set_value(initial_value);
    scale.set_hexpand(true);
    scale.set_draw_value(false);

    let handler_id = scale.connect_value_changed(move |s| {
        worker.set_slider(id, s.value());
    });

    row.append(&title);
    row.append(&scale);

    (
        row,
        SliderRow {
            id,
            scale,
            handler_id,
        },
    )
}
