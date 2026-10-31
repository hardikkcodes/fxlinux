//! Runs PipeWire's own event loop on a dedicated thread (GTK owns the main
//! thread's loop; the two loops can't share a thread) and applies live
//! control changes to the running "fxlinux_input" filter-chain node.
//!
//! This is the same connect/find-node/set_param sequence proven out by the
//! M2 throwaway CLI, made long-running: it keeps a registry listener active
//! so it re-finds the node if filter-chain.service is restarted (and
//! re-applies the last-selected preset when it does), receives slider/preset
//! changes over a `pipewire::channel` from the GTK thread, and reports
//! connection status back to the GTK thread over a plain `mpsc` channel
//! (glib's `MainContext::channel` isn't available in this version; GTK
//! widgets aren't `Send` so the worker thread can't touch them directly —
//! `PwWorker::spawn` hands the receiver to the caller to poll on the GTK
//! main loop instead).

use crate::audio::controls::{ControlId, updates_for};
use crate::presets::model::Preset;
use pipewire as pw;
use pw::spa::param::ParamType;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{Object, Pod, Property, PropertyFlags, Value};
use pw::spa::utils::SpaTypes;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

// spa/param/props.h: SPA_PROP_params, confirmed against the installed
// libspa-0.2-dev headers and cross-checked with `pw-cli e <id> Props`.
const SPA_PROP_PARAMS: u32 = 524289;
const TARGET_NODE_NAME: &str = "fxlinux_input";
const OUTPUT_NODE_NAME: &str = "fxlinux_output";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteStatus {
    EngineMissing,
    OutputMissing,
    OutputUnlinked,
    OutputMisrouted,
    Healthy { sink_description: Option<String> },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct NodeInfo {
    name: String,
    description: Option<String>,
    media_class: Option<String>,
    is_virtual: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinkInfo {
    output_node: u32,
    input_node: u32,
}

#[derive(Default)]
struct RouteState {
    nodes: HashMap<u32, NodeInfo>,
    links: HashMap<u32, LinkInfo>,
}

impl RouteState {
    fn evaluate(&self) -> RouteStatus {
        if !self
            .nodes
            .values()
            .any(|node| node.name == TARGET_NODE_NAME)
        {
            return RouteStatus::EngineMissing;
        }

        let Some(output_id) = self
            .nodes
            .iter()
            .filter_map(|(&id, node)| (node.name == OUTPUT_NODE_NAME).then_some(id))
            .min()
        else {
            return RouteStatus::OutputMissing;
        };

        let mut downstream: Vec<_> = self
            .links
            .values()
            .filter(|link| link.output_node == output_id)
            .collect();
        if downstream.is_empty() {
            return RouteStatus::OutputUnlinked;
        }

        // HashMap iteration is deliberately normalized so evaluation and its
        // tests stay deterministic if more than one route is present.
        downstream.sort_by_key(|link| link.input_node);
        for link in downstream {
            let Some(sink) = self.nodes.get(&link.input_node) else {
                continue;
            };
            let is_fxlinux_node = sink.name.starts_with("fxlinux_");
            if sink.media_class.as_deref() == Some("Audio/Sink")
                && sink.name != TARGET_NODE_NAME
                && !is_fxlinux_node
                && !sink.is_virtual
            {
                return RouteStatus::Healthy {
                    sink_description: sink.description.clone(),
                };
            }
        }

        RouteStatus::OutputMisrouted
    }

    fn remove_global(&mut self, id: u32) {
        self.nodes.remove(&id);
        self.links.remove(&id);
    }
}

pub enum ControlMsg {
    SetSlider(ControlId, f64),
    ApplyPreset(Preset),
}

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Every `(key, value)` pair needed to fully apply a preset: the four
/// primary sliders' underlying controls and the 8-band EQ. Bass leveling
/// (the multiband compressor) is static in the filter-chain config, not
/// preset-dependent — see presets::model's module doc.
fn preset_updates(preset: &Preset) -> Vec<(String, f32)> {
    let mut updates: Vec<(String, f32)> = Vec::new();

    for (id, value) in [
        (ControlId::Bass, preset.bass),
        (ControlId::Fidelity, preset.fidelity),
        (ControlId::Ambience, preset.ambience),
        (ControlId::Output, preset.output),
    ] {
        updates.extend(
            updates_for(id, value)
                .into_iter()
                .map(|(k, v)| (k.to_string(), v)),
        );
    }

    // Band center frequencies (16/40/100/250/630/1600/4000/10000 Hz) are the
    // plugin's own fixed defaults, documented in presets::model — only gain
    // needs to be pushed here.
    for (i, db) in preset.eq_db.iter().enumerate() {
        updates.push((format!("eq:g_{i}"), db_to_linear(*db)));
    }

    updates
}

pub struct PwWorker {
    sender: pw::channel::Sender<ControlMsg>,
    _thread: JoinHandle<()>,
}

impl PwWorker {
    /// Returns the worker plus a receiver for route-health updates.
    /// Poll the receiver from the GTK main loop (e.g. via
    /// `glib::timeout_add_local`) — never touch GTK widgets from this
    /// worker's own thread.
    pub fn spawn() -> (Self, mpsc::Receiver<RouteStatus>) {
        let (sender, receiver) = pw::channel::channel::<ControlMsg>();
        let (status_tx, status_rx) = mpsc::channel::<RouteStatus>();
        let thread = std::thread::spawn(move || run(receiver, status_tx));
        (
            Self {
                sender,
                _thread: thread,
            },
            status_rx,
        )
    }

    pub fn set_slider(&self, id: ControlId, value: f64) {
        // Only fails if the worker thread has already exited; nothing
        // meaningful to do about that from the UI thread here.
        let _ = self.sender.send(ControlMsg::SetSlider(id, value));
    }

    pub fn apply_preset(&self, preset: Preset) {
        let _ = self.sender.send(ControlMsg::ApplyPreset(preset));
    }
}

fn run(receiver: pw::channel::Receiver<ControlMsg>, status_tx: mpsc::Sender<RouteStatus>) {
    pw::init();
    inner_run(receiver, status_tx);
    unsafe { pw::deinit() };
}

fn inner_run(receiver: pw::channel::Receiver<ControlMsg>, status_tx: mpsc::Sender<RouteStatus>) {
    let mainloop = pw::main_loop::MainLoopRc::new(None).expect("Failed to create main loop");
    let context = pw::context::ContextRc::new(&mainloop, None).expect("Failed to create context");
    let core = context.connect_rc(None).expect("Failed to connect to core");
    let registry = core.get_registry_rc().expect("Failed to get registry");

    let node: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
    let node_id: Rc<RefCell<Option<u32>>> = Rc::new(RefCell::new(None));
    let last_preset: Rc<RefCell<Option<Preset>>> = Rc::new(RefCell::new(None));
    let route_state = Rc::new(RefCell::new(RouteState::default()));
    let last_status: Rc<RefCell<Option<RouteStatus>>> = Rc::new(RefCell::new(None));

    let registry_for_global = registry.clone();
    let node_for_global = node.clone();
    let node_id_for_global = node_id.clone();
    let last_preset_for_global = last_preset.clone();
    let route_state_for_global = route_state.clone();
    let last_status_for_global = last_status.clone();
    let status_tx_for_global = status_tx.clone();
    let _listener_reg = registry
        .add_listener_local()
        .global(move |global| {
            let Some(props) = &global.props else { return };
            match global.type_ {
                pw::types::ObjectType::Node => {
                    let name = props.get("node.name").unwrap_or_default().to_string();
                    route_state_for_global.borrow_mut().nodes.insert(
                        global.id,
                        NodeInfo {
                            name: name.clone(),
                            description: props.get("node.description").map(str::to_string),
                            media_class: props.get("media.class").map(str::to_string),
                            is_virtual: props.get("node.virtual") == Some("true"),
                        },
                    );

                    if name == TARGET_NODE_NAME {
                        match registry_for_global.bind::<pw::node::Node, _>(global) {
                            Ok(bound) => {
                                println!(
                                    "fxlinux-app: found {TARGET_NODE_NAME} (id={})",
                                    global.id
                                );
                                // Re-push the selected preset after a service restart.
                                if let Some(preset) = last_preset_for_global.borrow().as_ref() {
                                    push_controls(&bound, &preset_updates(preset));
                                }
                                *node_for_global.borrow_mut() = Some(bound);
                                *node_id_for_global.borrow_mut() = Some(global.id);
                            }
                            Err(e) => eprintln!("fxlinux-app: bind failed: {e}"),
                        }
                    }
                }
                pw::types::ObjectType::Link => {
                    let endpoints = props
                        .get("link.output.node")
                        .and_then(|id| id.parse().ok())
                        .zip(props.get("link.input.node").and_then(|id| id.parse().ok()));
                    if let Some((output_node, input_node)) = endpoints {
                        route_state_for_global.borrow_mut().links.insert(
                            global.id,
                            LinkInfo {
                                output_node,
                                input_node,
                            },
                        );
                    }
                }
                _ => {}
            }

            report_status(
                &route_state_for_global,
                &last_status_for_global,
                &status_tx_for_global,
            );
        })
        .global_remove({
            let node = node.clone();
            let node_id = node_id.clone();
            let route_state = route_state.clone();
            let last_status = last_status.clone();
            let status_tx = status_tx.clone();
            move |id| {
                if node_id.borrow().as_ref() == Some(&id) {
                    println!("fxlinux-app: {TARGET_NODE_NAME} disappeared (id={id})");
                    *node.borrow_mut() = None;
                    *node_id.borrow_mut() = None;
                }
                route_state.borrow_mut().remove_global(id);
                report_status(&route_state, &last_status, &status_tx);
            }
        })
        .register();

    // Cloned before `node` is moved into the receiver closure below.
    let route_state_for_timer = Rc::downgrade(&route_state);
    let last_status_for_timer = Rc::downgrade(&last_status);

    let _receiver = receiver.attach(mainloop.loop_(), move |msg| {
        // ApplyPreset is always remembered regardless of connection state —
        // sent on startup, this can race node discovery (the channel
        // message may be dispatched before the registry's initial sync
        // finds "fxlinux_input"). Storing it here means the `global`
        // handler's reconnect-reapply logic covers this same race, not
        // just later service restarts.
        if let ControlMsg::ApplyPreset(preset) = &msg {
            *last_preset.borrow_mut() = Some(preset.clone());
        }

        let node_ref = node.borrow();
        let Some(node_ref) = node_ref.as_ref() else {
            eprintln!("fxlinux-app: {TARGET_NODE_NAME} not connected yet, will apply once found");
            return;
        };
        match msg {
            ControlMsg::SetSlider(id, value) => {
                let pairs: Vec<(String, f32)> = updates_for(id, value)
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect();
                push_controls(node_ref, &pairs);
            }
            ControlMsg::ApplyPreset(preset) => {
                println!("fxlinux-app: applying preset '{}'", preset.name);
                push_controls(node_ref, &preset_updates(&preset));
            }
        }
    });

    // Ensure an empty registry still produces a status after startup.
    let status_tx_for_timer = status_tx.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        if let (Some(route_state), Some(last_status)) = (
            route_state_for_timer.upgrade(),
            last_status_for_timer.upgrade(),
        ) {
            report_status(&route_state, &last_status, &status_tx_for_timer);
        }
    });
    timer.update_timer(Some(std::time::Duration::from_secs(3)), None);

    mainloop.run();
}

fn report_status(
    state: &Rc<RefCell<RouteState>>,
    last_status: &Rc<RefCell<Option<RouteStatus>>>,
    sender: &mpsc::Sender<RouteStatus>,
) {
    let status = state.borrow().evaluate();
    if last_status.borrow().as_ref() != Some(&status) {
        *last_status.borrow_mut() = Some(status.clone());
        let _ = sender.send(status);
    }
}

/// Pushes every `(key, value)` pair as a single `Props` update — one struct
/// with all pairs interleaved, one `set_param` call — rather than one call
/// per control, so a multi-control change (a preset, or Ambience's
/// stereo+reverb pair) lands atomically instead of as visibly separate steps.
fn push_controls(node: &pw::node::Node, pairs: &[(String, f32)]) {
    if pairs.is_empty() {
        return;
    }
    let mut fields = Vec::with_capacity(pairs.len() * 2);
    for (key, value) in pairs {
        fields.push(Value::String(key.clone()));
        fields.push(Value::Float(*value));
    }
    let obj = Object {
        type_: SpaTypes::ObjectParamProps.as_raw(),
        id: ParamType::Props.as_raw(),
        properties: vec![Property {
            key: SPA_PROP_PARAMS,
            flags: PropertyFlags::empty(),
            value: Value::Struct(fields),
        }],
    };
    let bytes = match PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj)) {
        Ok((cursor, _)) => cursor.into_inner(),
        Err(e) => {
            eprintln!("fxlinux-app: pod serialize failed: {e:?}");
            return;
        }
    };
    let Some(pod) = Pod::from_bytes(&bytes) else {
        eprintln!("fxlinux-app: Pod::from_bytes failed");
        return;
    };
    node.set_param(ParamType::Props, 0, pod);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, media_class: Option<&str>, description: Option<&str>) -> NodeInfo {
        NodeInfo {
            name: name.to_string(),
            description: description.map(str::to_string),
            media_class: media_class.map(str::to_string),
            is_virtual: false,
        }
    }

    fn state_with_engine() -> RouteState {
        let mut state = RouteState::default();
        state.nodes.insert(
            10,
            node(TARGET_NODE_NAME, Some("Audio/Sink"), Some("FxLinux")),
        );
        state.nodes.insert(20, node(OUTPUT_NODE_NAME, None, None));
        state
    }

    #[test]
    fn reports_missing_engine() {
        assert_eq!(RouteState::default().evaluate(), RouteStatus::EngineMissing);
    }

    #[test]
    fn reports_present_but_unlinked_output() {
        assert_eq!(state_with_engine().evaluate(), RouteStatus::OutputUnlinked);
    }

    #[test]
    fn reports_healthy_real_sink_with_description() {
        let mut state = state_with_engine();
        state.nodes.insert(
            30,
            node(
                "alsa_output.pci-0000_00_1f.3.analog-stereo",
                Some("Audio/Sink"),
                Some("Built-in Audio Analog Stereo"),
            ),
        );
        state.links.insert(
            40,
            LinkInfo {
                output_node: 20,
                input_node: 30,
            },
        );

        assert_eq!(
            state.evaluate(),
            RouteStatus::Healthy {
                sink_description: Some("Built-in Audio Analog Stereo".to_string())
            }
        );
    }

    #[test]
    fn rejects_fxlinux_or_virtual_sink_routes() {
        let mut state = state_with_engine();
        state.nodes.insert(
            30,
            node(
                "fxlinux_monitor",
                Some("Audio/Sink"),
                Some("FxLinux monitor"),
            ),
        );
        state.links.insert(
            40,
            LinkInfo {
                output_node: 20,
                input_node: 30,
            },
        );

        assert_eq!(state.evaluate(), RouteStatus::OutputMisrouted);

        state.nodes.get_mut(&30).unwrap().name = "other_virtual_sink".to_string();
        state.nodes.get_mut(&30).unwrap().is_virtual = true;
        assert_eq!(state.evaluate(), RouteStatus::OutputMisrouted);
    }

    #[test]
    fn route_removal_returns_to_unlinked() {
        let mut state = state_with_engine();
        state.nodes.insert(
            30,
            node(
                "alsa_output.usb-headset",
                Some("Audio/Sink"),
                Some("USB Headset"),
            ),
        );
        state.links.insert(
            40,
            LinkInfo {
                output_node: 20,
                input_node: 30,
            },
        );
        assert!(matches!(state.evaluate(), RouteStatus::Healthy { .. }));

        state.remove_global(40);

        assert_eq!(state.evaluate(), RouteStatus::OutputUnlinked);
    }
}
