//! M2 throwaway CLI: proves live Props control of the running FxLinux
//! filter-chain node works through the `pipewire` Rust crate (not just
//! `pw-cli`), before any GTK UI is built.
//!
//! Usage: fxlinux-app <control-key> <float-value>
//! Example: fxlinux-app bass:amount 4.0

use pipewire as pw;
use pw::spa::param::ParamType;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{Object, Pod, Property, PropertyFlags, Value};
use pw::spa::utils::SpaTypes;
use std::cell::{Cell, RefCell};
use std::io::Cursor;
use std::rc::Rc;

// spa/param/props.h: SPA_PROP_params. Confirmed against the installed
// libspa-0.2-dev headers and cross-checked with `pw-cli e <id> Props` output
// (key 524289) before hardcoding here.
const SPA_PROP_PARAMS: u32 = 524289;

const TARGET_NODE_NAME: &str = "fxlinux_input";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: fxlinux-app <control-key> <float-value>");
        eprintln!("example: fxlinux-app bass:amount 4.0");
        std::process::exit(1);
    }
    let control_key = args[1].clone();
    let control_value: f32 = args[2].parse().expect("value must be a float");

    pw::init();
    // All PipeWire objects (mainloop/context/core/registry/listeners) must be
    // dropped before deinit() runs, hence the separate scope: pipewire-rs's
    // own roundtrip.rs example uses the same split for the same reason.
    let found = set_control(&control_key, control_value);
    unsafe { pw::deinit() };

    if !found {
        eprintln!("{TARGET_NODE_NAME} node not found (is filter-chain.service running?)");
        std::process::exit(1);
    }
}

fn set_control(control_key: &str, control_value: f32) -> bool {
    let control_key = control_key.to_string();
    let mainloop = pw::main_loop::MainLoopRc::new(None).expect("Failed to create main loop");
    let context = pw::context::ContextRc::new(&mainloop, None).expect("Failed to create context");
    let core = context.connect_rc(None).expect("Failed to connect to core");
    let registry = core.get_registry_rc().expect("Failed to get registry");

    let done = Rc::new(Cell::new(false));
    let found = Rc::new(Cell::new(false));
    // Kept alive only to prove the proxy stays valid long enough to send the
    // request; not otherwise used before the process exits.
    let node_holder: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));

    let registry_for_global = registry.clone();
    let node_holder_for_global = node_holder.clone();
    let found_for_global = found.clone();
    let control_key_for_global = control_key.clone();

    let _listener_reg = registry
        .add_listener_local()
        .global(move |global| {
            if found_for_global.get() {
                return;
            }
            let Some(props) = &global.props else {
                return;
            };
            if props.get("node.name") != Some(TARGET_NODE_NAME) {
                return;
            }
            println!("found {TARGET_NODE_NAME} node id={}", global.id);

            let node: pw::node::Node = match registry_for_global.bind(global) {
                Ok(node) => node,
                Err(e) => {
                    eprintln!("bind failed: {e}");
                    return;
                }
            };

            let obj = Object {
                type_: SpaTypes::ObjectParamProps.as_raw(),
                id: ParamType::Props.as_raw(),
                properties: vec![Property {
                    key: SPA_PROP_PARAMS,
                    flags: PropertyFlags::empty(),
                    value: Value::Struct(vec![
                        Value::String(control_key_for_global.clone()),
                        Value::Float(control_value),
                    ]),
                }],
            };

            let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
                .expect("pod serialize failed")
                .0
                .into_inner();
            let pod = Pod::from_bytes(&bytes).expect("Pod::from_bytes failed");

            node.set_param(ParamType::Props, 0, pod);
            println!("set {control_key_for_global} = {control_value} (live, no restart)");

            *node_holder_for_global.borrow_mut() = Some(node);
            found_for_global.set(true);
        })
        .register();

    // Same sync/done roundtrip pattern as pipewire-rs's own roundtrip.rs
    // example: quit once the server confirms it has sent every global that
    // existed when we connected (our target node included, if running).
    let pending = core.sync(0).expect("sync failed");
    let done_clone = done.clone();
    let loop_clone = mainloop.clone();
    let _listener_core = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                done_clone.set(true);
                loop_clone.quit();
            }
        })
        .register();

    while !done.get() {
        mainloop.run();
    }

    found.get()
}
