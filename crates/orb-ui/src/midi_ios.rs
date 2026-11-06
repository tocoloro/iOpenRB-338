//! CoreMIDI on the iPad. A virtual destination named `iOpenRB-338` is what AUM
//! sends its clock to. USB, Bluetooth and network sources are listed beside it.
//! The client lives on its own thread, because CoreMIDI delivers packets on the
//! run loop of the thread that created the client. The read callback posts to
//! [`MidiSync`] and takes the parser lock; the audio callback never takes that lock.

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use orb_player::midi::{MidiSync, PORT_SOURCE, PORT_VIRTUAL};

use super::MidiChoice;
use super::clock::{Parser, next_arm_packet};

const UTF8: u32 = 0x0800_0100;
/// The virtual port's name, and the name AUM shows.
pub const VIRTUAL_NAME: &str = "iOpenRB-338";

/// One external MIDI source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: u32,
    pub name: String,
}

struct Bridge {
    sync: Arc<MidiSync>,
    parser: Mutex<Parser>,
}

enum Req {
    Select(MidiChoice),
    Stop,
}

/// The CoreMIDI client. Dropping it removes the virtual port.
pub struct MidiIn {
    sync: Arc<MidiSync>,
    tx: Sender<Req>,
    run_loop: Arc<AtomicPtr<c_void>>,
    thread: Option<JoinHandle<()>>,
}

impl MidiIn {
    /// Creates the client, the virtual destination and the input port.
    pub fn open(sync: Arc<MidiSync>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let run_loop = Arc::new(AtomicPtr::new(ptr::null_mut()));
        let run_loop_thread = Arc::clone(&run_loop);
        let sync_thread = Arc::clone(&sync);
        let thread = thread::Builder::new()
            .name("midi".into())
            .spawn(move || midi_thread(sync_thread, rx, ready_tx, run_loop_thread))
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                sync,
                tx,
                run_loop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err("MIDI thread stopped".to_string())
            }
        }
    }

    pub fn choice(&self) -> MidiChoice {
        match self.sync.kind() {
            PORT_VIRTUAL => MidiChoice::Virtual,
            PORT_SOURCE => MidiChoice::Source(self.sync.source()),
            _ => MidiChoice::Off,
        }
    }

    pub fn sources(&self) -> Vec<Source> {
        let n = unsafe { midi_get_number_of_sources() };
        let mut out = Vec::new();
        for i in 0..n {
            let ep = unsafe { midi_get_source(i) };
            let Some(id) = unique_id(ep) else {
                continue;
            };
            if out.iter().any(|s: &Source| s.id == id) {
                continue;
            }
            out.push(Source {
                id,
                name: endpoint_name(ep, id),
            });
        }
        out
    }

    pub fn name_of(&self, id: u32) -> Option<String> {
        self.sources()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.name)
    }

    /// Follows `choice`. An external source is connected; the virtual port is
    /// always published and is followed only while it is the choice.
    pub fn select(&self, choice: MidiChoice) {
        if self.tx.send(Req::Select(choice)).is_ok() {
            self.wake();
        }
    }

    fn wake(&self) {
        let run_loop = self.run_loop.load(Ordering::Acquire);
        if !run_loop.is_null() {
            unsafe { cf_run_loop_wake_up(run_loop) };
        }
    }

    /// Applies a saved `midi_sync` value. A missing source is left off.
    pub fn restore(&self, stored: &str, name: &str) {
        match stored {
            "" | "off" => self.select(MidiChoice::Off),
            "virtual" => self.select(MidiChoice::Virtual),
            other => {
                let sources = self.sources();
                if let Ok(id) = other.parse::<u32>()
                    && sources.iter().any(|s| s.id == id)
                {
                    self.select(MidiChoice::Source(id));
                    return;
                }
                if let Some(s) = sources.iter().find(|s| !name.is_empty() && s.name == name) {
                    self.select(MidiChoice::Source(s.id));
                }
            }
        }
    }
}

impl Drop for MidiIn {
    fn drop(&mut self) {
        if self.tx.send(Req::Stop).is_ok() {
            self.wake();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Session {
    bridge: Arc<Bridge>,
    client: u32,
    port: u32,
    connected: u32,
}

impl Session {
    fn open(sync: Arc<MidiSync>) -> Result<Self, String> {
        let name = Cf::new(VIRTUAL_NAME)?;
        let mut client = 0u32;
        let err = unsafe { midi_client_create(name.0, None, ptr::null_mut(), &mut client) };
        if err != 0 || client == 0 {
            return Err(format!("MIDI client: {err}"));
        }
        let bridge = Arc::new(Bridge {
            sync,
            parser: Mutex::new(Parser::new()),
        });
        let ptr = Arc::as_ptr(&bridge) as *mut c_void;
        let mut dest = 0u32;
        let err = unsafe { midi_destination_create(client, name.0, dest_proc, ptr, &mut dest) };
        if err != 0 || dest == 0 {
            unsafe { midi_client_dispose(client) };
            return Err(format!("MIDI port: {err}"));
        }
        let mut port = 0u32;
        let err = unsafe { midi_input_port_create(client, name.0, source_proc, ptr, &mut port) };
        if err != 0 || port == 0 {
            unsafe { midi_client_dispose(client) };
            return Err(format!("MIDI input: {err}"));
        }
        Ok(Self {
            bridge,
            client,
            port,
            connected: 0,
        })
    }

    fn follow(&mut self, choice: MidiChoice) {
        match choice {
            MidiChoice::Off => self.bridge.sync.set_off(),
            MidiChoice::Virtual => self.bridge.sync.set_virtual(),
            MidiChoice::Source(id) => self.bridge.sync.set_source(id),
        }
        if self.connected != 0 {
            unsafe { midi_port_disconnect_source(self.port, self.connected) };
            self.connected = 0;
        }
        if let MidiChoice::Source(id) = choice
            && let Some(ep) = endpoint(id)
        {
            let err =
                unsafe { midi_port_connect_source(self.port, ep, id as usize as *mut c_void) };
            if err == 0 {
                self.connected = ep;
            }
        }
        if let Ok(mut parser) = self.bridge.parser.lock() {
            parser.reset();
        }
    }

    fn dispose(&mut self) {
        unsafe { midi_client_dispose(self.client) };
    }
}

fn endpoint(id: u32) -> Option<u32> {
    let n = unsafe { midi_get_number_of_sources() };
    for i in 0..n {
        let ep = unsafe { midi_get_source(i) };
        if unique_id(ep) == Some(id) {
            return Some(ep);
        }
    }
    None
}

fn midi_thread(
    sync: Arc<MidiSync>,
    rx: Receiver<Req>,
    ready: Sender<Result<(), String>>,
    run_loop: Arc<AtomicPtr<c_void>>,
) {
    let mut session = match Session::open(sync) {
        Ok(session) => session,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let current = unsafe { cf_run_loop_get_current() };
    run_loop.store(current, Ordering::Release);
    enable_network_session();
    if ready.send(Ok(())).is_err() {
        session.dispose();
        return;
    }
    let mode = unsafe { run_loop_default_mode };
    loop {
        unsafe { cf_run_loop_run_in_mode(mode, 0.05, 0) };
        let mut stop = false;
        while let Ok(req) = rx.try_recv() {
            match req {
                Req::Select(choice) => session.follow(choice),
                Req::Stop => stop = true,
            }
        }
        if stop {
            break;
        }
    }
    run_loop.store(ptr::null_mut(), Ordering::Release);
    session.dispose();
}

/// Joins the network MIDI session and accepts any computer on this Wi-Fi.
/// AUM's session belongs to AUM; it does not deliver clock into this app.
fn enable_network_session() {
    unsafe {
        let class = objc_get_class(c"MIDINetworkSession".as_ptr());
        if class.is_null() {
            return;
        }
        let session = objc_msg_send(class, sel_register_name(c"defaultSession".as_ptr()));
        if session.is_null() {
            return;
        }
        // `MIDINetworkConnectionPolicy_Anyone` is 2.
        msg_usize(
            session,
            sel_register_name(c"setConnectionPolicy:".as_ptr()),
            2,
        );
        msg_u8(session, sel_register_name(c"setEnabled:".as_ptr()), 1);
    }
}

unsafe fn msg_usize(obj: *mut c_void, sel: *mut c_void, arg: usize) {
    let send: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) =
        unsafe { std::mem::transmute(objc_msg_send as *const ()) };
    unsafe { send(obj, sel, arg) };
}

unsafe fn msg_u8(obj: *mut c_void, sel: *mut c_void, arg: u8) {
    let send: unsafe extern "C" fn(*mut c_void, *mut c_void, u8) =
        unsafe { std::mem::transmute(objc_msg_send as *const ()) };
    unsafe { send(obj, sel, arg) };
}

struct Cf(*const c_void);

impl Cf {
    fn new(text: &str) -> Result<Self, String> {
        let s = unsafe {
            cf_string_create_with_bytes(ptr::null(), text.as_ptr(), text.len() as isize, UTF8, 0)
        };
        if s.is_null() {
            Err(format!("MIDI name: {text}"))
        } else {
            Ok(Self(s))
        }
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { cf_release(self.0) };
        }
    }
}

fn unique_id(ep: u32) -> Option<u32> {
    if ep == 0 {
        return None;
    }
    let mut value = 0i32;
    let err = unsafe { midi_object_get_integer_property(ep, unique_id_key, &mut value) };
    if err != 0 || value == 0 {
        None
    } else {
        Some(value as u32)
    }
}

fn endpoint_name(ep: u32, id: u32) -> String {
    let mut string = ptr::null();
    let err = unsafe { midi_object_get_string_property(ep, display_name_key, &mut string) };
    if err != 0 || string.is_null() {
        return format!("MIDI {id}");
    }
    let mut buf = [0u8; 256];
    let ok = unsafe { cf_string_get_c_string(string, buf.as_mut_ptr(), buf.len() as isize, UTF8) };
    unsafe { cf_release(string) };
    if ok == 0 {
        return format!("MIDI {id}");
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..n]);
    if name.is_empty() {
        format!("MIDI {id}")
    } else {
        name.into_owned()
    }
}

fn host_ns(host: u64) -> u64 {
    let (numer, denom) = timebase();
    (u128::from(host) * u128::from(numer) / u128::from(denom)) as u64
}

fn timebase() -> (u32, u32) {
    static TB: OnceLock<(u32, u32)> = OnceLock::new();
    *TB.get_or_init(|| {
        let mut info = Timebase { numer: 1, denom: 1 };
        unsafe { mach_timebase_info(&mut info) };
        if info.denom == 0 {
            (1, 1)
        } else {
            (info.numer, info.denom)
        }
    })
}

/// Walks one packet list into the parser.
///
/// The virtual destination and the input port have separate callbacks. A packet
/// that arrives at the destination is the virtual port, whatever `src` CoreMIDI
/// passes. A packet on the input port carries the source's unique id.
///
/// SAFETY: CoreMIDI calls these with a live packet list and the `ref_con` passed
/// to `MIDIDestinationCreate` / `MIDIInputPortCreate`, which is the `Bridge`
/// kept alive by [`Session`]. The client is disposed before that `Arc` drops.
unsafe extern "C" fn dest_proc(list: *const c_void, ref_con: *mut c_void, _src: *mut c_void) {
    let Some(bridge) = bridge_ref(ref_con) else {
        return;
    };
    if bridge.sync.accepts_virtual() {
        packets(list, bridge);
    }
}

unsafe extern "C" fn source_proc(list: *const c_void, ref_con: *mut c_void, src: *mut c_void) {
    let Some(bridge) = bridge_ref(ref_con) else {
        return;
    };
    if bridge.sync.accepts_source(src as usize as u32) {
        packets(list, bridge);
    }
}

fn bridge_ref<'a>(ref_con: *mut c_void) -> Option<&'a Bridge> {
    if ref_con.is_null() {
        None
    } else {
        Some(unsafe { &*ref_con.cast::<Bridge>() })
    }
}

fn packets(list: *const c_void, bridge: &Bridge) {
    if list.is_null() {
        return;
    }
    let mut parser = bridge.parser.lock().unwrap_or_else(|e| e.into_inner());
    // The list is a u32 count, then packets packed to 4 bytes. The first
    // packet therefore starts at offset 4, so its time stamp is not 8-aligned.
    let list = list.cast::<u8>();
    let n = unsafe { std::ptr::read_unaligned(list.cast::<u32>()) }.min(1024);
    let mut pkt = unsafe { list.byte_add(4) };
    for _ in 0..n {
        let time_stamp = unsafe { std::ptr::read_unaligned(pkt.cast::<u64>()) };
        let length = unsafe { std::ptr::read_unaligned(pkt.byte_add(8).cast::<u16>()) };
        let len = usize::from(length).min(4096);
        let bytes = unsafe { std::slice::from_raw_parts(pkt.byte_add(10), len) };
        parser.feed(bytes, host_ns(time_stamp), &bridge.sync);
        pkt = next_arm_packet(pkt, length);
    }
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    #[link_name = "kCFRunLoopDefaultMode"]
    static run_loop_default_mode: *const c_void;

    #[link_name = "CFRunLoopGetCurrent"]
    fn cf_run_loop_get_current() -> *mut c_void;

    #[link_name = "CFRunLoopRunInMode"]
    fn cf_run_loop_run_in_mode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;

    #[link_name = "CFRunLoopWakeUp"]
    fn cf_run_loop_wake_up(run_loop: *mut c_void);

    #[link_name = "CFStringCreateWithBytes"]
    fn cf_string_create_with_bytes(
        alloc: *const c_void,
        bytes: *const u8,
        num_bytes: isize,
        encoding: u32,
        is_external: u8,
    ) -> *const c_void;

    #[link_name = "CFStringGetCString"]
    fn cf_string_get_c_string(
        string: *const c_void,
        buffer: *mut u8,
        size: isize,
        encoding: u32,
    ) -> u8;

    #[link_name = "CFRelease"]
    fn cf_release(cf: *const c_void);
}

#[link(name = "CoreMIDI", kind = "framework")]
unsafe extern "C" {
    #[link_name = "kMIDIPropertyDisplayName"]
    static display_name_key: *const c_void;

    #[link_name = "kMIDIPropertyUniqueID"]
    static unique_id_key: *const c_void;

    #[link_name = "MIDIClientCreate"]
    fn midi_client_create(
        name: *const c_void,
        notify: Option<unsafe extern "C" fn(*const c_void, *mut c_void)>,
        ref_con: *mut c_void,
        out_client: *mut u32,
    ) -> i32;

    #[link_name = "MIDIClientDispose"]
    fn midi_client_dispose(client: u32) -> i32;

    #[link_name = "MIDIDestinationCreate"]
    fn midi_destination_create(
        client: u32,
        name: *const c_void,
        read: unsafe extern "C" fn(*const c_void, *mut c_void, *mut c_void),
        ref_con: *mut c_void,
        out_dest: *mut u32,
    ) -> i32;

    #[link_name = "MIDIInputPortCreate"]
    fn midi_input_port_create(
        client: u32,
        name: *const c_void,
        read: unsafe extern "C" fn(*const c_void, *mut c_void, *mut c_void),
        ref_con: *mut c_void,
        out_port: *mut u32,
    ) -> i32;

    #[link_name = "MIDIPortConnectSource"]
    fn midi_port_connect_source(port: u32, source: u32, ref_con: *mut c_void) -> i32;

    #[link_name = "MIDIPortDisconnectSource"]
    fn midi_port_disconnect_source(port: u32, source: u32) -> i32;

    #[link_name = "MIDIGetNumberOfSources"]
    fn midi_get_number_of_sources() -> usize;

    #[link_name = "MIDIGetSource"]
    fn midi_get_source(index: usize) -> u32;

    #[link_name = "MIDIObjectGetIntegerProperty"]
    fn midi_object_get_integer_property(obj: u32, key: *const c_void, value: *mut i32) -> i32;

    #[link_name = "MIDIObjectGetStringProperty"]
    fn midi_object_get_string_property(
        obj: u32,
        key: *const c_void,
        string: *mut *const c_void,
    ) -> i32;
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_getClass"]
    fn objc_get_class(name: *const c_char) -> *mut c_void;

    #[link_name = "sel_registerName"]
    fn sel_register_name(name: *const c_char) -> *mut c_void;

    #[link_name = "objc_msgSend"]
    fn objc_msg_send(obj: *mut c_void, sel: *mut c_void) -> *mut c_void;
}
