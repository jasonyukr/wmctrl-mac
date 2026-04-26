#![allow(unexpected_cfgs)]

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{Boolean, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, CFDictionaryGetValue, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::{
    CFBooleanGetValue, CFBooleanRef, CFNumberGetValue, CFNumberRef, kCFBooleanTrue,
    kCFNumberFloat64Type, kCFNumberSInt32Type,
};
use core_foundation_sys::string::{
    CFStringGetCString, CFStringGetCStringPtr, CFStringGetLength,
    CFStringGetMaximumSizeForEncoding, CFStringRef, kCFStringEncodingUTF8,
};
use libc::{c_char, c_int, c_uint, c_void, pid_t};
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;
use std::process;

type CGWindowID = c_uint;
type AXError = c_int;
type AXUIElementRef = *const c_void;
type OSStatus = i32;
type OSType = u32;
type DescType = OSType;
type AEEventClass = OSType;
type AEEventID = OSType;
type AEReturnID = i16;
type AETransactionID = i32;
type AESendMode = i32;
type Size = libc::c_long;
type AEAddressDesc = AEDesc;
type AppleEvent = AEDesc;

#[repr(C, packed(2))]
struct AEDesc {
    descriptor_type: DescType,
    data_handle: *mut c_void,
}

const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: c_uint = 16;
const K_CG_NULL_WINDOW_ID: CGWindowID = 0;
const K_AX_ERROR_SUCCESS: AXError = 0;
const NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS: u64 = 1;
const TYPE_KERNEL_PROCESS_ID: DescType = fourcc(*b"kpid");
const K_CORE_EVENT_CLASS: AEEventClass = fourcc(*b"aevt");
const K_AE_REOPEN_APPLICATION: AEEventID = fourcc(*b"rapp");
const K_AUTO_GENERATE_RETURN_ID: AEReturnID = -1;
const K_ANY_TRANSACTION_ID: AETransactionID = 0;
const K_AE_NO_REPLY: AESendMode = 1;
const K_AE_DEFAULT_TIMEOUT: libc::c_long = -1;

const fn fourcc(value: [u8; 4]) -> OSType {
    u32::from_be_bytes(value)
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowOwnerName: CFStringRef;
    static kCGWindowName: CFStringRef;
    static kCGWindowBounds: CFStringRef;
    static kCGWindowLayer: CFStringRef;
    static kCGWindowAlpha: CFStringRef;
    static kCGWindowIsOnscreen: CFStringRef;

    fn CGWindowListCopyWindowInfo(option: c_uint, relative_to_window: CGWindowID) -> CFArrayRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;

    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn _AXUIElementGetWindow(element: AXUIElementRef, identifier: *mut CGWindowID) -> AXError;

    fn AECreateDesc(
        descriptor_type: DescType,
        data_ptr: *const c_void,
        data_size: Size,
        result: *mut AEDesc,
    ) -> OSStatus;
    fn AECreateAppleEvent(
        event_class: AEEventClass,
        event_id: AEEventID,
        target: *const AEAddressDesc,
        return_id: AEReturnID,
        transaction_id: AETransactionID,
        result: *mut AppleEvent,
    ) -> OSStatus;
    fn AESendMessage(
        event: *const AppleEvent,
        reply: *mut AppleEvent,
        send_mode: AESendMode,
        time_out_in_ticks: libc::c_long,
    ) -> OSStatus;
    fn AEDisposeDesc(desc: *mut AEDesc) -> OSStatus;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSOpenCFURLRef(in_url: CFTypeRef, out_launched_url: *mut CFTypeRef) -> OSStatus;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    QuerySpaces,
    QueryWindows { space: Option<i32> },
    ListWnd { sort: bool, space: Option<i32> },
    FocusWindow { id: CGWindowID },
    FocusAdjacentWindow { direction: FocusDirection },
    FocusOtherWindow { direction: FocusDirection },
    SendToBack,
    LaunchOrFocus { app_name: String },
}

const HELP: &str = "Usage:\n  wmctrl-mac --help\n  wmctrl-mac -h\n  wmctrl-mac -m query --spaces\n  wmctrl-mac -m query --windows\n  wmctrl-mac -m query --windows --space <index>\n  wmctrl-mac -m window --focus <id>\n  wmctrl-mac -m listwnd [-s] [space]\n  wmctrl-mac listwnd [-s] [space]\n  wmctrl-mac -m focus-next-window\n  wmctrl-mac -m focus-prev-window\n  wmctrl-mac -m focus-other-next-window\n  wmctrl-mac -m focus-other-prev-window\n  wmctrl-mac -m send-to-back\n  wmctrl-mac -m launch-or-focus <app name>";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum FocusDirection {
    Next,
    Prev,
}

#[derive(Serialize)]
struct Frame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Serialize)]
struct Window {
    id: i32,
    pid: i32,
    app: String,
    title: String,
    scratchpad: String,
    frame: Frame,
    role: String,
    subrole: String,
    #[serde(rename = "root-window")]
    root_window: bool,
    display: i32,
    space: i32,
    level: i32,
    #[serde(rename = "sub-level")]
    sub_level: i32,
    layer: String,
    #[serde(rename = "sub-layer")]
    sub_layer: String,
    opacity: f64,
    #[serde(rename = "split-type")]
    split_type: String,
    #[serde(rename = "split-child")]
    split_child: String,
    #[serde(rename = "stack-index")]
    stack_index: i32,
    #[serde(rename = "can-move")]
    can_move: bool,
    #[serde(rename = "can-resize")]
    can_resize: bool,
    #[serde(rename = "has-focus")]
    has_focus: bool,
    #[serde(rename = "has-shadow")]
    has_shadow: bool,
    #[serde(rename = "has-parent-zoom")]
    has_parent_zoom: bool,
    #[serde(rename = "has-fullscreen-zoom")]
    has_fullscreen_zoom: bool,
    #[serde(rename = "has-ax-reference")]
    has_ax_reference: bool,
    #[serde(rename = "is-native-fullscreen")]
    is_native_fullscreen: bool,
    #[serde(rename = "is-visible")]
    is_visible: bool,
    #[serde(rename = "is-minimized")]
    is_minimized: bool,
    #[serde(rename = "is-hidden")]
    is_hidden: bool,
    #[serde(rename = "is-floating")]
    is_floating: bool,
    #[serde(rename = "is-sticky")]
    is_sticky: bool,
    #[serde(rename = "is-grabbed")]
    is_grabbed: bool,
}

#[derive(Serialize)]
struct Space {
    id: i32,
    uuid: String,
    index: i32,
    label: String,
    #[serde(rename = "type")]
    space_type: String,
    display: i32,
    windows: Vec<i32>,
    #[serde(rename = "first-window")]
    first_window: i32,
    #[serde(rename = "last-window")]
    last_window: i32,
    #[serde(rename = "has-focus")]
    has_focus: bool,
    #[serde(rename = "is-visible")]
    is_visible: bool,
    #[serde(rename = "is-native-fullscreen")]
    is_native_fullscreen: bool,
}

struct AxInfo {
    pid: pid_t,
    app: String,
    element: AXUIElementRef,
    role: Option<String>,
    subrole: Option<String>,
    title: Option<String>,
    minimized: Option<bool>,
}

struct FocusCandidate {
    id: i32,
    space: i32,
    app: String,
    has_focus: bool,
}

struct FrontmostApplication {
    pid: pid_t,
    name: String,
}

impl Drop for AxInfo {
    fn drop(&mut self) {
        if !self.element.is_null() {
            unsafe { CFRelease(self.element as CFTypeRef) }
        }
    }
}

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("wmctrl-mac: {error}");
        process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match parse_command(&args)? {
        Command::Help => {
            println!("{HELP}");
            Ok(())
        }
        Command::QuerySpaces => {
            let windows = query_windows()?;
            print_json(&vec![focused_space(
                windows.iter().map(|window| window.id).collect(),
            )])
        }
        Command::QueryWindows { space } => {
            let windows = if space.unwrap_or(1) == 1 {
                query_windows()?
            } else {
                Vec::new()
            };
            print_json(&windows)
        }
        Command::ListWnd { sort, space } => print_listwnd(query_windows()?, sort, space),
        Command::FocusWindow { id } => focus_window(id),
        Command::FocusAdjacentWindow { direction } => focus_adjacent_window(direction),
        Command::FocusOtherWindow { direction } => focus_other_window(direction),
        Command::SendToBack => send_focused_window_to_back(),
        Command::LaunchOrFocus { app_name } => launch_or_focus_application(&app_name),
    }
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    match args {
        [command] if command == "--help" || command == "-h" => Ok(Command::Help),
        [mode, query, target] if mode == "-m" && query == "query" && target == "--spaces" => {
            Ok(Command::QuerySpaces)
        }
        [mode, query, target] if mode == "-m" && query == "query" && target == "--windows" => {
            Ok(Command::QueryWindows { space: None })
        }
        [mode, command, args @ ..] if mode == "-m" && command == "listwnd" => {
            parse_listwnd_command(args)
        }
        [mode, command, args @ ..] if mode == "-m" && command == "launch-or-focus" => {
            parse_launch_or_focus_command(args)
        }
        [mode, command] if mode == "-m" && command == "focus-other-next-window" => {
            Ok(Command::FocusOtherWindow {
                direction: FocusDirection::Next,
            })
        }
        [mode, command] if mode == "-m" && command == "focus-other-prev-window" => {
            Ok(Command::FocusOtherWindow {
                direction: FocusDirection::Prev,
            })
        }
        [mode, command] if mode == "-m" && command == "focus-next-window" => {
            Ok(Command::FocusAdjacentWindow {
                direction: FocusDirection::Next,
            })
        }
        [mode, command] if mode == "-m" && command == "focus-prev-window" => {
            Ok(Command::FocusAdjacentWindow {
                direction: FocusDirection::Prev,
            })
        }
        [mode, command] if mode == "-m" && command == "send-to-back" => Ok(Command::SendToBack),
        [command, args @ ..] if command == "listwnd" => parse_listwnd_command(args),
        [mode, query, target, space_flag, space]
            if mode == "-m"
                && query == "query"
                && target == "--windows"
                && space_flag == "--space" =>
        {
            let space = space
                .parse::<i32>()
                .map_err(|_| "--space requires a numeric index".to_string())?;
            Ok(Command::QueryWindows { space: Some(space) })
        }
        [mode, window, focus, id] if mode == "-m" && window == "window" && focus == "--focus" => {
            let id = id
                .parse::<CGWindowID>()
                .map_err(|_| "--focus requires a numeric window id".to_string())?;
            Ok(Command::FocusWindow { id })
        }
        _ => Err("unsupported command".to_string()),
    }
}

fn parse_launch_or_focus_command(args: &[String]) -> Result<Command, String> {
    let app_name = args.join(" ");
    if app_name.trim().is_empty() {
        return Err("launch-or-focus requires an app name".to_string());
    }
    Ok(Command::LaunchOrFocus { app_name })
}

fn parse_listwnd_command(args: &[String]) -> Result<Command, String> {
    let mut sort = false;
    let mut space = None;
    let mut has_space_arg = false;

    for arg in args {
        match arg.as_str() {
            "-s" if !sort => sort = true,
            arg if is_listwnd_space_arg(arg) && !has_space_arg => {
                space = parse_listwnd_space(arg);
                has_space_arg = true;
            }
            _ => return Err("unsupported command".to_string()),
        }
    }

    Ok(Command::ListWnd { sort, space })
}

fn is_listwnd_space_arg(arg: &str) -> bool {
    !arg.starts_with('-') || arg.parse::<i32>().is_ok()
}

fn parse_listwnd_space(space: &str) -> Option<i32> {
    match space.parse::<i32>() {
        Ok(space) if (1..=1).contains(&space) => Some(space),
        _ => None,
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    println!("{json}");
    Ok(())
}

fn print_listwnd(windows: Vec<Window>, sort: bool, space: Option<i32>) -> Result<(), String> {
    for line in listwnd_lines(&windows, sort, space) {
        println!("{line}");
    }
    Ok(())
}

fn listwnd_lines(windows: &[Window], sort: bool, space: Option<i32>) -> Vec<String> {
    let mut windows = windows
        .iter()
        .filter(|window| space.is_none_or(|space| window.space == space))
        .collect::<Vec<_>>();
    if sort {
        windows.sort_by_key(|window| window.id);
    }
    windows
        .iter()
        .map(|window| {
            format!(
                "{} {} {} \"{}\"",
                window.space, window.has_focus, window.id, window.app
            )
        })
        .collect()
}

fn focus_other_window(direction: FocusDirection) -> Result<(), String> {
    ensure_ax_trusted()?;
    let raw_windows = focus_raw_windows()?;
    let ax_windows = collect_focus_ax_windows(&raw_window_applications(&raw_windows));
    let focused_window = focused_window_id();
    let focus_candidates = focus_candidates(raw_windows, &ax_windows, focused_window);
    let focus_target = select_other_window(&focus_candidates, direction)?;
    if let Some(id) = focus_target {
        let ax = ax_windows.get(&(id as CGWindowID)).ok_or_else(|| {
            "unable to resolve AX window; grant Accessibility permission or focus the window's space".to_string()
        })?;
        focus_ax_window(ax)?;
    }
    Ok(())
}

fn focus_adjacent_window(direction: FocusDirection) -> Result<(), String> {
    ensure_ax_trusted()?;
    let Some((frontmost, focused_window)) = focused_frontmost_window() else {
        return focus_adjacent_window_fallback(direction);
    };
    let raw_windows = focus_raw_windows_for_pid(frontmost.pid)?;
    let ax_windows =
        collect_focus_ax_windows(&frontmost_raw_window_applications(&raw_windows, &frontmost));
    let focus_candidates = focus_candidates(raw_windows, &ax_windows, Some(focused_window));
    let focus_target = select_adjacent_window(&focus_candidates, direction);
    if let Some(id) = focus_target {
        let ax = ax_windows.get(&(id as CGWindowID)).ok_or_else(|| {
            "unable to resolve AX window; grant Accessibility permission or focus the window's space".to_string()
        })?;
        focus_ax_window(ax)?;
    }
    Ok(())
}

fn focus_adjacent_window_fallback(direction: FocusDirection) -> Result<(), String> {
    let raw_windows = focus_raw_windows()?;
    let ax_windows = collect_focus_ax_windows(&raw_window_applications(&raw_windows));
    let focused_window = focused_window_id();
    let focus_candidates = focus_candidates(raw_windows, &ax_windows, focused_window);
    let focus_target = select_adjacent_window(&focus_candidates, direction);
    if let Some(id) = focus_target {
        let ax = ax_windows.get(&(id as CGWindowID)).ok_or_else(|| {
            "unable to resolve AX window; grant Accessibility permission or focus the window's space".to_string()
        })?;
        focus_ax_window(ax)?;
    }
    Ok(())
}

fn send_focused_window_to_back() -> Result<(), String> {
    let Some(focused_window) = focused_window_id() else {
        return Ok(());
    };
    ensure_ax_trusted()?;
    let raw_windows = focus_raw_windows()?;
    let ax_windows = collect_focus_ax_windows(&raw_window_applications(&raw_windows));
    for id in send_to_back_raise_order(&raw_windows, &ax_windows, focused_window) {
        if let Some(ax) = ax_windows.get(&id) {
            raise_ax_window(ax)?;
        }
    }
    Ok(())
}

fn send_to_back_raise_order(
    raw_windows: &[RawWindow],
    ax_windows: &HashMap<CGWindowID, AxInfo>,
    focused_window: CGWindowID,
) -> Vec<CGWindowID> {
    let visible_ids = raw_windows
        .iter()
        .map(|raw| raw.id as CGWindowID)
        .collect::<HashSet<_>>();
    let mut ids = raw_windows
        .iter()
        .rev()
        .map(|raw| raw.id as CGWindowID)
        .filter(|id| *id != focused_window)
        .filter(|id| ax_windows.get(id).is_some_and(is_compatible_ax_window))
        .collect::<Vec<_>>();
    ids.extend(
        sorted_ax_windows(ax_windows)
            .into_iter()
            .filter(|(id, ax)| {
                *id != focused_window && !visible_ids.contains(id) && is_compatible_ax_window(ax)
            })
            .map(|(id, _)| id),
    );
    ids
}

fn select_adjacent_window(windows: &[FocusCandidate], direction: FocusDirection) -> Option<i32> {
    let (qlines, focused) = focus_qlines(windows)?;
    let list = qlines
        .iter()
        .filter(|window| window.space == focused.space && window.app == focused.app)
        .map(|window| window.id)
        .collect::<Vec<_>>();
    let focused_index = list.iter().position(|id| *id == focused.id)?;

    match direction {
        FocusDirection::Next => list.get((focused_index + 1) % list.len()).copied(),
        FocusDirection::Prev => list
            .get((focused_index + list.len() - 1) % list.len())
            .copied(),
    }
}

fn focus_candidates(
    raw_windows: Vec<RawWindow>,
    ax_windows: &HashMap<CGWindowID, AxInfo>,
    focused_window: Option<CGWindowID>,
) -> Vec<FocusCandidate> {
    let mut visible_window_ids = HashSet::new();
    let mut candidates = Vec::new();

    for raw in raw_windows {
        let id = raw.id as CGWindowID;
        let Some(ax) = ax_windows.get(&id) else {
            continue;
        };
        if !is_compatible_ax_window(ax) {
            continue;
        }
        visible_window_ids.insert(id);
        candidates.push(FocusCandidate {
            id: raw.id,
            space: 1,
            app: if raw.app.is_empty() {
                ax.app.clone()
            } else {
                raw.app
            },
            has_focus: focused_window == Some(id),
        });
    }

    candidates.extend(
        ax_windows
            .iter()
            .map(|(id, ax)| (*id, ax))
            .filter(|(id, _)| !visible_window_ids.contains(id))
            .filter(|(_, ax)| is_compatible_ax_window(ax))
            .map(|(id, ax)| FocusCandidate {
                id: id as i32,
                space: 1,
                app: ax.app.clone(),
                has_focus: focused_window == Some(id),
            }),
    );

    candidates.sort_by_key(|window| window.id);
    candidates
}

fn select_other_window(
    windows: &[FocusCandidate],
    direction: FocusDirection,
) -> Result<Option<i32>, String> {
    let Some((qlines, focused)) = focus_qlines(windows) else {
        return Ok(None);
    };

    let state_file = focus_other_window_state_file(focused.space);
    let mut remembered = read_focus_other_window_state(&state_file)?;
    remembered.insert(focused.app.clone(), focused.id);
    write_focus_other_window_state(&state_file, &remembered)?;

    Ok(select_representative_window(
        &qlines,
        focused,
        &remembered,
        direction,
    ))
}

fn focus_qlines(windows: &[FocusCandidate]) -> Option<(Vec<&FocusCandidate>, &FocusCandidate)> {
    let mut qlines = windows.iter().collect::<Vec<_>>();
    qlines.sort_by_key(|window| window.id);
    if qlines.is_empty() {
        return None;
    }

    let focused = qlines.iter().find(|window| window.has_focus).copied()?;
    if focused.id == 0 || focused.space == 0 || focused.app.is_empty() {
        return None;
    }
    Some((qlines, focused))
}

fn select_representative_window(
    qlines: &[&FocusCandidate],
    focused: &FocusCandidate,
    remembered: &HashMap<String, i32>,
    direction: FocusDirection,
) -> Option<i32> {
    let desktop_window_ids = qlines
        .iter()
        .filter(|window| window.space == focused.space)
        .map(|window| window.id)
        .collect::<HashSet<_>>();
    let mut app_names = HashSet::new();
    let mut list = Vec::new();
    let mut focused_index = None;

    for window in qlines
        .iter()
        .filter(|window| window.space == focused.space)
        .copied()
    {
        if window.app == focused.app {
            if window.id == focused.id {
                focused_index = Some(list.len());
                list.push(window.id);
            }
        } else if app_names.insert(window.app.clone()) {
            let id = remembered
                .get(&window.app)
                .filter(|id| desktop_window_ids.contains(id))
                .copied()
                .unwrap_or(window.id);
            list.push(id);
        }
    }

    let focused_index = focused_index?;
    match direction {
        FocusDirection::Next => list.get((focused_index + 1) % list.len()).copied(),
        FocusDirection::Prev => list
            .get((focused_index + list.len() - 1) % list.len())
            .copied(),
    }
}

fn focus_other_window_state_file(space: i32) -> PathBuf {
    let temp_dir = env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(temp_dir).join(format!("move_focus_to_other_window_reps_{space}"))
}

fn read_focus_other_window_state(path: &PathBuf) -> Result<HashMap<String, i32>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(format!("unable to read focus state: {error}")),
    };
    let mut remembered = HashMap::new();
    for line in contents.lines() {
        let mut fields = line.splitn(2, '\t');
        let Some(id) = fields.next().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(app) = fields.next().filter(|app| !app.is_empty()) else {
            continue;
        };
        if let Ok(id) = id.parse::<i32>() {
            remembered.insert(app.to_string(), id);
        }
    }
    Ok(remembered)
}

fn write_focus_other_window_state(
    path: &PathBuf,
    remembered: &HashMap<String, i32>,
) -> Result<(), String> {
    let contents = remembered
        .iter()
        .map(|(app, id)| format!("{id}\t{app}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{contents}\n"))
        .map_err(|error| format!("unable to write focus state: {error}"))
}

fn focused_space(windows: Vec<i32>) -> Space {
    Space {
        id: 1,
        uuid: "compact-space-1".to_string(),
        index: 1,
        label: String::new(),
        space_type: "bsp".to_string(),
        display: 1,
        first_window: windows.first().copied().unwrap_or(0),
        last_window: windows.last().copied().unwrap_or(0),
        windows,
        has_focus: true,
        is_visible: true,
        is_native_fullscreen: false,
    }
}

fn query_windows() -> Result<Vec<Window>, String> {
    let raw_windows = cg_windows()?;
    let ax_windows = collect_ax_windows(&raw_window_applications(&raw_windows));
    let focused_window = focused_window_id();

    Ok(compatible_windows(raw_windows, &ax_windows, focused_window))
}

fn focus_window(id: CGWindowID) -> Result<(), String> {
    ensure_ax_trusted()?;
    let ax_windows = collect_ax_windows(&running_applications());
    let ax = ax_windows.get(&id).ok_or_else(|| {
        "unable to resolve AX window; grant Accessibility permission or focus the window's space".to_string()
    })?;

    focus_ax_window(ax)
}

fn focus_ax_window(ax: &AxInfo) -> Result<(), String> {
    activate_application(ax.pid);
    unsafe {
        let raise_action = cf_string_create("AXRaise");
        AXUIElementPerformAction(ax.element, raise_action);
        CFRelease(raise_action as CFTypeRef);
        let app = AXUIElementCreateApplication(ax.pid);
        if app.is_null() {
            return Err("unable to create AX application reference".to_string());
        }
        let focused_window_attribute = cf_string_create("AXFocusedWindow");
        let set_result =
            AXUIElementSetAttributeValue(app, focused_window_attribute, ax.element as CFTypeRef);
        CFRelease(focused_window_attribute as CFTypeRef);
        CFRelease(app as CFTypeRef);
        if set_result != K_AX_ERROR_SUCCESS {
            return Err("unable to focus AX window; check Accessibility permission".to_string());
        }
    }
    Ok(())
}

fn raise_ax_window(ax: &AxInfo) -> Result<(), String> {
    unsafe {
        let raise_action = cf_string_create("AXRaise");
        let result = AXUIElementPerformAction(ax.element, raise_action);
        CFRelease(raise_action as CFTypeRef);
        if result != K_AX_ERROR_SUCCESS {
            return Err("unable to raise AX window; check Accessibility permission".to_string());
        }
    }
    Ok(())
}

fn compatible_windows(
    raw_windows: Vec<RawWindow>,
    ax_windows: &HashMap<CGWindowID, AxInfo>,
    focused_window: Option<CGWindowID>,
) -> Vec<Window> {
    let mut visible_windows = Vec::new();
    let mut other_windows = Vec::new();
    let mut visible_window_ids = HashSet::new();

    for raw in raw_windows {
        let id = raw.id as CGWindowID;
        let Some(ax) = ax_windows.get(&id) else {
            continue;
        };
        if !is_compatible_ax_window(ax) {
            continue;
        }
        visible_window_ids.insert(id);
        let window = raw
            .clone_with_ax_defaults(ax)
            .into_window(Some(ax), focused_window);
        if window.is_visible {
            visible_windows.push(window);
        } else {
            other_windows.push(window);
        }
    }

    other_windows.extend(
        sorted_ax_windows(ax_windows)
            .into_iter()
            .filter(|(id, _)| !visible_window_ids.contains(id))
            .filter(|(_, ax)| is_compatible_ax_window(ax))
            .map(|(id, ax)| RawWindow::from_ax(id, ax).into_window(Some(ax), focused_window)),
    );

    if let Some(focused_window) = focused_window {
        if let Some(index) = visible_windows
            .iter()
            .position(|window| window.id as CGWindowID == focused_window)
        {
            let focused = visible_windows.remove(index);
            visible_windows.insert(0, focused);
        }
    }

    visible_windows.extend(other_windows);
    visible_windows
}

fn sorted_ax_windows(ax_windows: &HashMap<CGWindowID, AxInfo>) -> Vec<(CGWindowID, &AxInfo)> {
    let mut windows = ax_windows
        .iter()
        .map(|(id, ax)| (*id, ax))
        .collect::<Vec<_>>();
    windows.sort_by_key(|(id, _)| *id);
    windows
}

fn is_compatible_ax_window(ax: &AxInfo) -> bool {
    ax.role.as_deref().is_none_or(|role| role == "AXWindow")
        && ax
            .subrole
            .as_deref()
            .is_none_or(|subrole| subrole == "AXStandardWindow")
}

fn raw_window_applications(raw_windows: &[RawWindow]) -> HashMap<pid_t, String> {
    let mut applications: HashMap<pid_t, String> = HashMap::new();
    for raw in raw_windows {
        applications
            .entry(raw.pid)
            .and_modify(|app| {
                if app.is_empty() && !raw.app.is_empty() {
                    *app = raw.app.clone();
                }
            })
            .or_insert_with(|| raw.app.clone());
    }
    applications
}

fn frontmost_raw_window_applications(
    raw_windows: &[RawWindow],
    frontmost: &FrontmostApplication,
) -> HashMap<pid_t, String> {
    let mut applications = raw_window_applications(raw_windows);
    applications
        .entry(frontmost.pid)
        .and_modify(|app| {
            if app.is_empty() {
                *app = frontmost.name.clone();
            }
        })
        .or_insert_with(|| frontmost.name.clone());
    applications
}

fn ensure_ax_trusted() -> Result<(), String> {
    if ax_is_process_trusted(false) {
        Ok(())
    } else {
        ax_is_process_trusted(true);
        Err("Accessibility permission is required; approve this wmctrl-mac binary in System Settings > Privacy & Security > Accessibility, then run the command again".to_string())
    }
}

fn ax_is_process_trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrustedWithOptions(std::ptr::null()) != 0;
        }

        let keys = [kAXTrustedCheckOptionPrompt as *const c_void];
        let values = [kCFBooleanTrue as *const c_void];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        if options.is_null() {
            return false;
        }

        let trusted = AXIsProcessTrustedWithOptions(options) != 0;
        CFRelease(options as CFTypeRef);
        trusted
    }
}

struct RawWindow {
    id: i32,
    pid: i32,
    app: String,
    title: String,
    frame: Frame,
    level: i32,
    opacity: f64,
    is_visible: bool,
}

impl RawWindow {
    fn from_ax(id: CGWindowID, ax: &AxInfo) -> Self {
        Self {
            id: id as i32,
            pid: ax.pid,
            app: ax.app.clone(),
            title: ax.title.clone().unwrap_or_default(),
            frame: Frame {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            level: 0,
            opacity: 1.0,
            is_visible: false,
        }
    }

    fn clone_with_ax_defaults(&self, ax: &AxInfo) -> Self {
        Self {
            id: self.id,
            pid: self.pid,
            app: if self.app.is_empty() {
                ax.app.clone()
            } else {
                self.app.clone()
            },
            title: self.title.clone(),
            frame: Frame {
                x: self.frame.x,
                y: self.frame.y,
                w: self.frame.w,
                h: self.frame.h,
            },
            level: self.level,
            opacity: self.opacity,
            is_visible: self.is_visible,
        }
    }

    fn into_window(self, ax: Option<&AxInfo>, focused_window: Option<CGWindowID>) -> Window {
        let has_ax_reference = ax.is_some();
        Window {
            id: self.id,
            pid: self.pid,
            app: self.app,
            title: ax.and_then(|info| info.title.clone()).unwrap_or(self.title),
            scratchpad: String::new(),
            frame: self.frame,
            role: ax
                .and_then(|info| info.role.clone())
                .unwrap_or_else(|| "AXWindow".to_string()),
            subrole: ax
                .and_then(|info| info.subrole.clone())
                .unwrap_or_else(|| "AXStandardWindow".to_string()),
            root_window: false,
            display: 1,
            space: 1,
            level: self.level,
            sub_level: 0,
            layer: if self.level == 0 { "normal" } else { "above" }.to_string(),
            sub_layer: "normal".to_string(),
            opacity: self.opacity,
            split_type: "none".to_string(),
            split_child: "none".to_string(),
            stack_index: 0,
            can_move: has_ax_reference,
            can_resize: has_ax_reference,
            has_focus: focused_window == Some(self.id as CGWindowID),
            has_shadow: true,
            has_parent_zoom: false,
            has_fullscreen_zoom: false,
            has_ax_reference,
            is_native_fullscreen: false,
            is_visible: self.is_visible,
            is_minimized: ax.and_then(|info| info.minimized).unwrap_or(false),
            is_hidden: false,
            is_floating: true,
            is_sticky: false,
            is_grabbed: false,
        }
    }
}

fn cg_windows() -> Result<Vec<RawWindow>, String> {
    unsafe {
        let options = K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS;
        let list = CGWindowListCopyWindowInfo(options, K_CG_NULL_WINDOW_ID);
        if list.is_null() {
            return Err("unable to enumerate windows".to_string());
        }

        let mut windows = Vec::new();
        for index in 0..CFArrayGetCount(list) {
            let dictionary = CFArrayGetValueAtIndex(list, index) as CFDictionaryRef;
            if dictionary.is_null() {
                continue;
            }

            let id = dictionary_i32(dictionary, kCGWindowNumber).unwrap_or(0);
            let pid = dictionary_i32(dictionary, kCGWindowOwnerPID).unwrap_or(0);
            let frame = dictionary_frame(dictionary, kCGWindowBounds).unwrap_or(Frame {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            });
            if id == 0 || pid == 0 || frame.w <= 0.0 || frame.h <= 0.0 {
                continue;
            }

            windows.push(RawWindow {
                id,
                pid,
                app: dictionary_string(dictionary, kCGWindowOwnerName).unwrap_or_default(),
                title: dictionary_string(dictionary, kCGWindowName).unwrap_or_default(),
                frame,
                level: dictionary_i32(dictionary, kCGWindowLayer).unwrap_or(0),
                opacity: dictionary_f64(dictionary, kCGWindowAlpha).unwrap_or(1.0),
                is_visible: dictionary_bool(dictionary, kCGWindowIsOnscreen).unwrap_or(true),
            });
        }
        CFRelease(list as CFTypeRef);
        Ok(windows)
    }
}

fn focus_raw_windows() -> Result<Vec<RawWindow>, String> {
    focus_raw_windows_matching(None)
}

fn focus_raw_windows_for_pid(pid: pid_t) -> Result<Vec<RawWindow>, String> {
    focus_raw_windows_matching(Some(pid))
}

fn focus_raw_windows_matching(pid_filter: Option<pid_t>) -> Result<Vec<RawWindow>, String> {
    unsafe {
        let options = K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS;
        let list = CGWindowListCopyWindowInfo(options, K_CG_NULL_WINDOW_ID);
        if list.is_null() {
            return Err("unable to enumerate windows".to_string());
        }

        let mut windows = Vec::new();
        for index in 0..CFArrayGetCount(list) {
            let dictionary = CFArrayGetValueAtIndex(list, index) as CFDictionaryRef;
            if dictionary.is_null() {
                continue;
            }

            let id = dictionary_i32(dictionary, kCGWindowNumber).unwrap_or(0);
            let pid = dictionary_i32(dictionary, kCGWindowOwnerPID).unwrap_or(0);
            if id == 0 || pid == 0 || pid_filter.is_some_and(|expected| pid != expected) {
                continue;
            }

            let app = dictionary_string(dictionary, kCGWindowOwnerName).unwrap_or_default();
            let level = dictionary_i32(dictionary, kCGWindowLayer).unwrap_or(0);
            if !is_focus_raw_window_level(level, &app) {
                continue;
            }

            let frame = dictionary_frame(dictionary, kCGWindowBounds).unwrap_or(Frame {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            });
            if frame.w <= 0.0 || frame.h <= 0.0 {
                continue;
            }

            windows.push(RawWindow {
                id,
                pid,
                app,
                title: String::new(),
                frame,
                level,
                opacity: 1.0,
                is_visible: true,
            });
        }
        CFRelease(list as CFTypeRef);
        Ok(windows)
    }
}

fn is_focus_raw_window_level(level: i32, app: &str) -> bool {
    level == 0 || !matches!(app, "Control Center" | "Dock" | "SystemUIServer")
}

fn collect_ax_windows(applications: &HashMap<pid_t, String>) -> HashMap<CGWindowID, AxInfo> {
    collect_ax_windows_with_attributes(applications, true)
}

fn collect_focus_ax_windows(applications: &HashMap<pid_t, String>) -> HashMap<CGWindowID, AxInfo> {
    collect_ax_windows_with_attributes(applications, false)
}

fn collect_ax_windows_with_attributes(
    applications: &HashMap<pid_t, String>,
    include_extra_attributes: bool,
) -> HashMap<CGWindowID, AxInfo> {
    let mut windows = HashMap::new();
    for (pid, app_name) in applications {
        unsafe {
            let app = AXUIElementCreateApplication(*pid);
            if app.is_null() {
                continue;
            }
            let windows_attribute = cf_string_create("AXWindows");
            let array = copy_ax_attribute(app, windows_attribute);
            CFRelease(windows_attribute as CFTypeRef);
            let Some(array) = array else {
                CFRelease(app as CFTypeRef);
                continue;
            };

            for index in 0..CFArrayGetCount(array as CFArrayRef) {
                let element = CFArrayGetValueAtIndex(array as CFArrayRef, index) as AXUIElementRef;
                let mut id = 0;
                if _AXUIElementGetWindow(element, &mut id) != K_AX_ERROR_SUCCESS || id == 0 {
                    continue;
                }
                windows.entry(id).or_insert_with(|| AxInfo {
                    pid: *pid,
                    app: app_name.clone(),
                    element: retain_ax_element(element),
                    role: copy_ax_string_named(element, "AXRole"),
                    subrole: copy_ax_string_named(element, "AXSubrole"),
                    title: include_extra_attributes
                        .then(|| copy_ax_string_named(element, "AXTitle"))
                        .flatten(),
                    minimized: include_extra_attributes
                        .then(|| copy_ax_bool_named(element, "AXMinimized"))
                        .flatten(),
                });
            }

            CFRelease(array);
            CFRelease(app as CFTypeRef);
        }
    }
    windows
}

fn running_applications() -> HashMap<pid_t, String> {
    unsafe {
        let Some(class) = Class::get("NSWorkspace") else {
            return HashMap::new();
        };
        let workspace: *mut Object = msg_send![class, sharedWorkspace];
        if workspace.is_null() {
            return HashMap::new();
        }
        let applications: *mut Object = msg_send![workspace, runningApplications];
        if applications.is_null() {
            return HashMap::new();
        }

        let mut names = HashMap::new();
        let count: usize = msg_send![applications, count];
        for index in 0..count {
            let application: *mut Object = msg_send![applications, objectAtIndex: index];
            if application.is_null() {
                continue;
            }
            let pid: pid_t = msg_send![application, processIdentifier];
            if pid == 0 {
                continue;
            }
            let name: *mut Object = msg_send![application, localizedName];
            names.insert(pid, cf_string(name as CFStringRef).unwrap_or_default());
        }
        names
    }
}

fn launch_or_focus_application(app_name: &str) -> Result<(), String> {
    if app_name.contains('/') {
        return Err("launch-or-focus only supports application names, not paths".to_string());
    }

    unsafe {
        let Some(class) = Class::get("NSWorkspace") else {
            return Err("NSWorkspace is unavailable".to_string());
        };
        let workspace: *mut Object = msg_send![class, sharedWorkspace];
        if workspace.is_null() {
            return Err("NSWorkspace is unavailable".to_string());
        }
        let applications: *mut Object = msg_send![workspace, runningApplications];
        if applications.is_null() {
            return Err("NSWorkspace runningApplications is unavailable".to_string());
        }

        let count: usize = msg_send![applications, count];
        for index in 0..count {
            let application: *mut Object = msg_send![applications, objectAtIndex: index];
            if application.is_null() {
                continue;
            }
            let name: *mut Object = msg_send![application, localizedName];
            let Some(name) = cf_string(name as CFStringRef) else {
                continue;
            };
            if app_name_matches(&name, app_name) {
                let pid: pid_t = msg_send![application, processIdentifier];
                let activated: bool = msg_send![application, activateWithOptions: NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS];
                if !activated {
                    return Err(format!("failed to activate {app_name}"));
                }
                if running_application_has_usable_windows(pid, &name) {
                    return Ok(());
                }

                let lsopen_status = launch_running_application_url(workspace, application);
                let reopen_status = if lsopen_status == 0 {
                    0
                } else {
                    send_reopen_application(pid)
                };
                let activated: bool = msg_send![application, activateWithOptions: NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS];
                if lsopen_status == 0 {
                    return Ok(());
                }
                if !activated {
                    return Err(format!("failed to activate {app_name}"));
                }
                return Err(format!(
                    "failed to reopen {app_name}: AppleEvent error {reopen_status}; LSOpenCFURLRef error {lsopen_status}"
                ));
            }
        }

        let launched = launch_workspace_application(workspace, app_name);
        launched
            .then_some(())
            .ok_or_else(|| format!("failed to launch {app_name}"))
    }
}

fn running_application_has_usable_windows(pid: pid_t, app_name: &str) -> bool {
    let mut applications = HashMap::new();
    applications.insert(pid, app_name.to_string());
    collect_ax_windows(&applications)
        .values()
        .any(is_usable_ax_window)
}

fn is_usable_ax_window(ax: &AxInfo) -> bool {
    is_compatible_ax_window(ax) && !ax.minimized.unwrap_or(false)
}

unsafe fn launch_workspace_application(workspace: *mut Object, app_name: &str) -> bool {
    let name = cf_string_create(app_name);
    let launched: bool = unsafe { msg_send![workspace, launchApplication: name] };
    unsafe { CFRelease(name as CFTypeRef) };
    launched
}

unsafe fn launch_running_application_url(
    workspace: *mut Object,
    application: *mut Object,
) -> OSStatus {
    let bundle_identifier: *mut Object = unsafe { msg_send![application, bundleIdentifier] };
    if bundle_identifier.is_null() {
        return -1;
    }

    let url: *mut Object =
        unsafe { msg_send![workspace, URLForApplicationWithBundleIdentifier: bundle_identifier] };
    if url.is_null() {
        return -1;
    }

    unsafe { LSOpenCFURLRef(url as CFTypeRef, std::ptr::null_mut()) }
}

unsafe fn send_reopen_application(pid: pid_t) -> OSStatus {
    let mut target = AEDesc {
        descriptor_type: 0,
        data_handle: std::ptr::null_mut(),
    };
    let status = unsafe {
        AECreateDesc(
            TYPE_KERNEL_PROCESS_ID,
            &pid as *const pid_t as *const c_void,
            std::mem::size_of_val(&pid) as Size,
            &mut target,
        )
    };
    if status != 0 {
        return status;
    }

    let mut event = AEDesc {
        descriptor_type: 0,
        data_handle: std::ptr::null_mut(),
    };
    let status = unsafe {
        AECreateAppleEvent(
            K_CORE_EVENT_CLASS,
            K_AE_REOPEN_APPLICATION,
            &target,
            K_AUTO_GENERATE_RETURN_ID,
            K_ANY_TRANSACTION_ID,
            &mut event,
        )
    };
    if status != 0 {
        unsafe { AEDisposeDesc(&mut target) };
        return status;
    }

    let status = unsafe {
        AESendMessage(
            &event,
            std::ptr::null_mut(),
            K_AE_NO_REPLY,
            K_AE_DEFAULT_TIMEOUT,
        )
    };
    unsafe { AEDisposeDesc(&mut event) };
    unsafe { AEDisposeDesc(&mut target) };
    status
}

fn app_name_matches(running_name: &str, requested_name: &str) -> bool {
    running_name == requested_name || running_name.eq_ignore_ascii_case(requested_name)
}

fn focused_window_id() -> Option<CGWindowID> {
    focused_frontmost_window_id().or_else(focused_system_window_id)
}

fn focused_frontmost_window_id() -> Option<CGWindowID> {
    focused_frontmost_window().map(|(_, id)| id)
}

fn focused_frontmost_window() -> Option<(FrontmostApplication, CGWindowID)> {
    let frontmost = frontmost_application()?;
    unsafe {
        let app = AXUIElementCreateApplication(frontmost.pid);
        if app.is_null() {
            return None;
        }
        let focused = focused_ax_window_id(app);
        CFRelease(app as CFTypeRef);
        focused.map(|id| (frontmost, id))
    }
}

fn frontmost_application() -> Option<FrontmostApplication> {
    unsafe {
        let class = Class::get("NSWorkspace")?;
        let workspace: *mut Object = msg_send![class, sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let application: *mut Object = msg_send![workspace, frontmostApplication];
        if application.is_null() {
            return None;
        }
        let pid: pid_t = msg_send![application, processIdentifier];
        if pid == 0 {
            return None;
        }
        let name: *mut Object = msg_send![application, localizedName];
        Some(FrontmostApplication {
            pid,
            name: cf_string(name as CFStringRef).unwrap_or_default(),
        })
    }
}

fn focused_system_window_id() -> Option<CGWindowID> {
    unsafe {
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return None;
        }
        let focused = focused_ax_window_id(system);
        CFRelease(system as CFTypeRef);
        focused
    }
}

fn focused_ax_window_id(element: AXUIElementRef) -> Option<CGWindowID> {
    let focused_window_attribute = cf_string_create("AXFocusedWindow");
    let focused = copy_ax_attribute(element, focused_window_attribute);
    unsafe { CFRelease(focused_window_attribute as CFTypeRef) };
    let focused = focused?;
    let mut id = 0;
    let result = unsafe { _AXUIElementGetWindow(focused as AXUIElementRef, &mut id) };
    unsafe { CFRelease(focused) };
    (result == K_AX_ERROR_SUCCESS && id != 0).then_some(id)
}

fn copy_ax_attribute(element: AXUIElementRef, attribute: CFStringRef) -> Option<CFTypeRef> {
    unsafe {
        let mut value = std::ptr::null();
        (AXUIElementCopyAttributeValue(element, attribute, &mut value) == K_AX_ERROR_SUCCESS
            && !value.is_null())
        .then_some(value)
    }
}

fn copy_ax_string_named(element: AXUIElementRef, attribute: &str) -> Option<String> {
    let attribute = cf_string_create(attribute);
    let value = copy_ax_attribute(element, attribute);
    unsafe { CFRelease(attribute as CFTypeRef) };
    let value = value?;
    let string = cf_string(value as CFStringRef);
    unsafe { CFRelease(value) };
    string
}

fn copy_ax_bool_named(element: AXUIElementRef, attribute: &str) -> Option<bool> {
    let attribute = cf_string_create(attribute);
    let value = copy_ax_attribute(element, attribute);
    unsafe { CFRelease(attribute as CFTypeRef) };
    let value = value?;
    let bool_value = cf_bool(value as CFBooleanRef);
    unsafe { CFRelease(value) };
    bool_value
}

fn retain_ax_element(element: AXUIElementRef) -> AXUIElementRef {
    unsafe { core_foundation_sys::base::CFRetain(element as CFTypeRef) as AXUIElementRef }
}

fn activate_application(pid: pid_t) {
    unsafe {
        let Some(class) = Class::get("NSRunningApplication") else {
            return;
        };
        let app: *mut Object = msg_send![class, runningApplicationWithProcessIdentifier: pid];
        if !app.is_null() {
            let _: bool =
                msg_send![app, activateWithOptions: NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS];
        }
    }
}

fn dictionary_value(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<CFTypeRef> {
    unsafe {
        let value = CFDictionaryGetValue(dictionary, key as *const c_void);
        (!value.is_null()).then_some(value as CFTypeRef)
    }
}

fn dictionary_i32(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
    cf_i32(dictionary_value(dictionary, key)? as CFNumberRef)
}

fn dictionary_f64(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<f64> {
    cf_f64(dictionary_value(dictionary, key)? as CFNumberRef)
}

fn dictionary_bool(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<bool> {
    cf_bool(dictionary_value(dictionary, key)? as CFBooleanRef)
}

fn dictionary_string(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    cf_string(dictionary_value(dictionary, key)? as CFStringRef)
}

fn dictionary_frame(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<Frame> {
    let bounds = dictionary_value(dictionary, key)? as CFDictionaryRef;
    let x = bounds_number(bounds, "X")?;
    let y = bounds_number(bounds, "Y")?;
    let w = bounds_number(bounds, "Width")?;
    let h = bounds_number(bounds, "Height")?;
    Some(Frame { x, y, w, h })
}

fn bounds_number(bounds: CFDictionaryRef, key: &str) -> Option<f64> {
    let key = cf_string_create(key);
    let value = dictionary_f64(bounds, key);
    unsafe { CFRelease(key as CFTypeRef) };
    value
}

fn cf_i32(number: CFNumberRef) -> Option<i32> {
    unsafe {
        let mut value = 0i32;
        CFNumberGetValue(
            number,
            kCFNumberSInt32Type,
            &mut value as *mut i32 as *mut c_void,
        )
        .then_some(value)
    }
}

fn cf_f64(number: CFNumberRef) -> Option<f64> {
    unsafe {
        let mut value = 0.0f64;
        CFNumberGetValue(
            number,
            kCFNumberFloat64Type,
            &mut value as *mut f64 as *mut c_void,
        )
        .then_some(value)
    }
}

fn cf_bool(value: CFBooleanRef) -> Option<bool> {
    (!value.is_null()).then(|| unsafe { CFBooleanGetValue(value) })
}

fn cf_string(value: CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }

    unsafe {
        let ptr = CFStringGetCStringPtr(value, kCFStringEncodingUTF8);
        if !ptr.is_null() {
            return Some(CStr::from_ptr(ptr).to_string_lossy().into_owned());
        }

        let length = CFStringGetLength(value);
        let size = CFStringGetMaximumSizeForEncoding(length, kCFStringEncodingUTF8) + 1;
        let mut buffer = vec![0u8; size as usize];
        if CFStringGetCString(
            value,
            buffer.as_mut_ptr() as *mut c_char,
            size,
            kCFStringEncodingUTF8,
        ) != 0
        {
            Some(
                CStr::from_ptr(buffer.as_ptr() as *const c_char)
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        }
    }
}

fn cf_string_create(value: &str) -> CFStringRef {
    unsafe {
        core_foundation_sys::string::CFStringCreateWithBytes(
            std::ptr::null(),
            value.as_ptr(),
            value.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_supported_commands() {
        assert_eq!(parse_command(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse_command(&args(&["-h"])), Ok(Command::Help));
        assert_eq!(
            parse_command(&args(&["-m", "query", "--spaces"])),
            Ok(Command::QuerySpaces)
        );
        assert_eq!(
            parse_command(&args(&["-m", "query", "--windows"])),
            Ok(Command::QueryWindows { space: None })
        );
        assert_eq!(
            parse_command(&args(&["-m", "query", "--windows", "--space", "2"])),
            Ok(Command::QueryWindows { space: Some(2) })
        );
        assert_eq!(
            parse_command(&args(&["listwnd"])),
            Ok(Command::ListWnd {
                sort: false,
                space: None
            })
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "-s"])),
            Ok(Command::ListWnd {
                sort: true,
                space: None
            })
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "1"])),
            Ok(Command::ListWnd {
                sort: false,
                space: Some(1)
            })
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "-s", "1"])),
            Ok(Command::ListWnd {
                sort: true,
                space: Some(1)
            })
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "abc"])),
            Ok(Command::ListWnd {
                sort: false,
                space: None
            })
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "2"])),
            Ok(Command::ListWnd {
                sort: false,
                space: None
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "listwnd"])),
            Ok(Command::ListWnd {
                sort: false,
                space: None
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "listwnd", "-s"])),
            Ok(Command::ListWnd {
                sort: true,
                space: None
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "listwnd", "1"])),
            Ok(Command::ListWnd {
                sort: false,
                space: Some(1)
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "listwnd", "-s", "1"])),
            Ok(Command::ListWnd {
                sort: true,
                space: Some(1)
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "focus-other-next-window"])),
            Ok(Command::FocusOtherWindow {
                direction: FocusDirection::Next
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "focus-other-prev-window"])),
            Ok(Command::FocusOtherWindow {
                direction: FocusDirection::Prev
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "focus-next-window"])),
            Ok(Command::FocusAdjacentWindow {
                direction: FocusDirection::Next
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "focus-prev-window"])),
            Ok(Command::FocusAdjacentWindow {
                direction: FocusDirection::Prev
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "send-to-back"])),
            Ok(Command::SendToBack)
        );
        assert_eq!(
            parse_command(&args(&["-m", "window", "--focus", "42"])),
            Ok(Command::FocusWindow { id: 42 })
        );
        assert_eq!(
            parse_command(&args(&["-m", "launch-or-focus", "Finder"])),
            Ok(Command::LaunchOrFocus {
                app_name: "Finder".to_string()
            })
        );
        assert_eq!(
            parse_command(&args(&["-m", "launch-or-focus", "System", "Settings"])),
            Ok(Command::LaunchOrFocus {
                app_name: "System Settings".to_string()
            })
        );
    }

    #[test]
    fn rejects_unsupported_commands() {
        assert_eq!(
            parse_command(&args(&["-m", "query", "--displays"])),
            Err("unsupported command".to_string())
        );
        assert_eq!(
            parse_command(&args(&["-m", "--help"])),
            Err("unsupported command".to_string())
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "-h"])),
            Err("unsupported command".to_string())
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "-x"])),
            Err("unsupported command".to_string())
        );
        assert_eq!(
            parse_command(&args(&["listwnd", "-f"])),
            Err("unsupported command".to_string())
        );
        assert_eq!(
            parse_command(&args(&["-m", "listwnd", "-f"])),
            Err("unsupported command".to_string())
        );
        assert_eq!(
            parse_command(&args(&["-m", "window", "--focus", "abc"])),
            Err("--focus requires a numeric window id".to_string())
        );
        assert_eq!(
            parse_command(&args(&["-m", "launch-or-focus"])),
            Err("launch-or-focus requires an app name".to_string())
        );
    }

    #[test]
    fn app_name_matches_exact_or_case_insensitive() {
        assert!(app_name_matches("Finder", "Finder"));
        assert!(app_name_matches("Finder", "finder"));
        assert!(!app_name_matches("System Settings", "Settings"));
    }

    #[test]
    fn usable_ax_windows_require_compatible_non_minimized_window() {
        let mut standard = test_ax_info(42, "Edge", "AXStandardWindow");
        assert!(is_usable_ax_window(&standard));

        standard.minimized = Some(true);
        assert!(!is_usable_ax_window(&standard));
        assert!(!is_usable_ax_window(&test_ax_info(
            42,
            "Edge",
            "AXSystemDialog"
        )));
    }

    #[test]
    fn fourcc_uses_big_endian_values() {
        assert_eq!(fourcc(*b"kpid"), 0x6b706964);
        assert_eq!(fourcc(*b"aevt"), 0x61657674);
        assert_eq!(fourcc(*b"rapp"), 0x72617070);
    }

    #[test]
    fn space_json_has_listwnd_shape() {
        let value = serde_json::to_value(focused_space(vec![10, 20])).unwrap();
        for field in [
            "id",
            "uuid",
            "index",
            "label",
            "type",
            "display",
            "windows",
            "first-window",
            "last-window",
            "has-focus",
            "is-visible",
            "is-native-fullscreen",
        ] {
            assert!(value.get(field).is_some(), "missing {field}");
        }
    }

    #[test]
    fn window_json_has_listwnd_shape() {
        let raw = RawWindow {
            id: 7,
            pid: 8,
            app: "App".to_string(),
            title: "Title".to_string(),
            frame: Frame {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            },
            level: 0,
            opacity: 1.0,
            is_visible: true,
        };
        let value = serde_json::to_value(raw.into_window(None, Some(7))).unwrap();
        let object = value.as_object().unwrap();
        for field in [
            "id",
            "pid",
            "app",
            "title",
            "scratchpad",
            "frame",
            "role",
            "subrole",
            "root-window",
            "display",
            "space",
            "level",
            "sub-level",
            "layer",
            "sub-layer",
            "opacity",
            "split-type",
            "split-child",
            "stack-index",
            "can-move",
            "can-resize",
            "has-focus",
            "has-shadow",
            "has-parent-zoom",
            "has-fullscreen-zoom",
            "has-ax-reference",
            "is-native-fullscreen",
            "is-visible",
            "is-minimized",
            "is-hidden",
            "is-floating",
            "is-sticky",
            "is-grabbed",
        ] {
            assert!(object.contains_key(field), "missing {field}");
        }
        assert!(matches!(value["frame"], Value::Object(_)));
    }

    #[test]
    fn listwnd_lines_match_old_output_shape() {
        let windows = vec![test_window(42, "App", true), test_window(7, "Other", false)];

        assert_eq!(
            listwnd_lines(&windows, false, None),
            vec![
                "1 true 42 \"App\"".to_string(),
                "1 false 7 \"Other\"".to_string()
            ]
        );
        assert_eq!(
            listwnd_lines(&windows, true, Some(1)),
            vec![
                "1 false 7 \"Other\"".to_string(),
                "1 true 42 \"App\"".to_string()
            ]
        );
        assert!(listwnd_lines(&windows, false, Some(2)).is_empty());
    }

    #[test]
    fn focus_adjacent_window_wraps_same_app_next_and_prev() {
        let windows = vec![
            test_focus_candidate(1, "App", false),
            test_focus_candidate(2, "App", true),
            test_focus_candidate(3, "App", false),
            test_focus_candidate(4, "Other", false),
        ];

        assert_eq!(
            select_adjacent_window(&windows, FocusDirection::Next),
            Some(3)
        );
        assert_eq!(
            select_adjacent_window(&windows, FocusDirection::Prev),
            Some(1)
        );

        let windows = vec![
            test_focus_candidate(1, "App", true),
            test_focus_candidate(2, "App", false),
            test_focus_candidate(3, "Other", false),
        ];

        assert_eq!(
            select_adjacent_window(&windows, FocusDirection::Prev),
            Some(2)
        );
    }

    #[test]
    fn focus_adjacent_window_uses_current_space() {
        let focused = test_focus_candidate(1, "App", true);
        let other_app = test_focus_candidate(2, "App", false);
        let mut other_space = test_focus_candidate(3, "App", false);
        other_space.space = 2;
        let windows = vec![focused, other_app, other_space];

        assert_eq!(
            select_adjacent_window(&windows, FocusDirection::Next),
            Some(2)
        );
    }

    #[test]
    fn focus_candidates_include_ax_only_windows() {
        let raw_windows = vec![test_raw_window(2, "App", true)];
        let ax_windows = HashMap::from([
            (1, test_ax_info(8, "Other", "AXStandardWindow")),
            (2, test_ax_info(8, "App", "AXStandardWindow")),
        ]);

        let candidates = focus_candidates(raw_windows, &ax_windows, Some(2));

        assert_eq!(
            candidates
                .iter()
                .map(|window| window.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(candidates[1].has_focus);
    }

    #[test]
    fn send_to_back_raise_order_uses_reverse_cg_order_then_ax_only() {
        let raw_windows = vec![
            test_raw_window(1, "App", true),
            test_raw_window(2, "App", true),
            test_raw_window(3, "Other", true),
        ];
        let ax_windows = HashMap::from([
            (1, test_ax_info(8, "App", "AXStandardWindow")),
            (2, test_ax_info(8, "App", "AXStandardWindow")),
            (3, test_ax_info(9, "Other", "AXSystemDialog")),
            (4, test_ax_info(10, "Third", "AXStandardWindow")),
        ]);

        assert_eq!(
            send_to_back_raise_order(&raw_windows, &ax_windows, 1),
            vec![2, 4]
        );
    }

    #[test]
    fn focus_other_window_wraps_next_and_prev() {
        let windows = vec![
            test_focus_candidate(1, "App", true),
            test_focus_candidate(2, "Other", false),
            test_focus_candidate(3, "Third", false),
        ];
        let (qlines, focused) = focus_qlines(&windows).unwrap();
        let remembered = HashMap::new();

        assert_eq!(
            select_representative_window(&qlines, focused, &remembered, FocusDirection::Prev),
            Some(3)
        );

        let windows = vec![
            test_focus_candidate(1, "App", false),
            test_focus_candidate(2, "Other", false),
            test_focus_candidate(3, "Third", true),
        ];
        let (qlines, focused) = focus_qlines(&windows).unwrap();

        assert_eq!(
            select_representative_window(&qlines, focused, &remembered, FocusDirection::Next),
            Some(1)
        );
    }

    #[test]
    fn focus_other_window_uses_one_representative_per_app() {
        let windows = vec![
            test_focus_candidate(1, "App", true),
            test_focus_candidate(2, "Other", false),
            test_focus_candidate(3, "Other", false),
            test_focus_candidate(4, "Third", false),
        ];
        let (qlines, focused) = focus_qlines(&windows).unwrap();

        assert_eq!(
            select_representative_window(&qlines, focused, &HashMap::new(), FocusDirection::Next),
            Some(2)
        );
        assert_eq!(
            select_representative_window(&qlines, focused, &HashMap::new(), FocusDirection::Prev),
            Some(4)
        );
    }

    #[test]
    fn focus_other_window_uses_remembered_window_on_same_desktop() {
        let windows = vec![
            test_focus_candidate(1, "App", true),
            test_focus_candidate(2, "Other", false),
            test_focus_candidate(3, "Other", false),
        ];
        let (qlines, focused) = focus_qlines(&windows).unwrap();
        let remembered = HashMap::from([("Other".to_string(), 3)]);

        assert_eq!(
            select_representative_window(&qlines, focused, &remembered, FocusDirection::Next),
            Some(3)
        );
    }

    #[test]
    fn focus_other_window_ignores_other_same_app_windows() {
        let windows = vec![
            test_focus_candidate(1, "App", true),
            test_focus_candidate(2, "App", false),
            test_focus_candidate(3, "Other", false),
        ];
        let (qlines, focused) = focus_qlines(&windows).unwrap();

        assert_eq!(
            select_representative_window(&qlines, focused, &HashMap::new(), FocusDirection::Next),
            Some(3)
        );
        assert_eq!(
            select_representative_window(&qlines, focused, &HashMap::new(), FocusDirection::Prev),
            Some(3)
        );
    }

    #[test]
    fn focus_other_window_keeps_ax_candidates() {
        let other = test_focus_candidate(1, "Other", false);
        let focused = test_focus_candidate(3, "App", true);

        let windows = [other, focused];
        let (qlines, focused) = focus_qlines(&windows).unwrap();

        assert_eq!(
            qlines.iter().map(|window| window.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(focused.id, 3);
    }

    #[test]
    fn focus_other_window_no_focused_row_is_noop() {
        let windows = vec![
            test_focus_candidate(1, "App", false),
            test_focus_candidate(2, "Other", false),
        ];

        assert!(focus_qlines(&windows).is_none());
    }

    #[test]
    fn compatible_windows_exclude_cg_only_and_include_ax_only() {
        let raw_windows = vec![RawWindow {
            id: 7,
            pid: 8,
            app: "Dock".to_string(),
            title: String::new(),
            frame: Frame {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            },
            level: 20,
            opacity: 1.0,
            is_visible: true,
        }];
        let ax_windows = HashMap::from([(42, test_ax_info(9, "Edge", "AXStandardWindow"))]);

        let windows = compatible_windows(raw_windows, &ax_windows, Some(42));

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, 42);
        assert_eq!(windows[0].app, "Edge");
        assert!(windows[0].has_ax_reference);
        assert!(windows[0].has_focus);
        assert!(!windows[0].is_visible);
    }

    #[test]
    fn compatible_windows_orders_focused_visible_then_cg_then_ax_only() {
        let raw_windows = vec![
            test_raw_window(266, "Finder", true),
            test_raw_window(98, "kitty", true),
        ];
        let ax_windows = HashMap::from([
            (109, test_ax_info(10, "Edge", "AXStandardWindow")),
            (98, test_ax_info(11, "kitty", "AXStandardWindow")),
            (266, test_ax_info(12, "Finder", "AXStandardWindow")),
            (75, test_ax_info(13, "Notes", "AXStandardWindow")),
        ]);

        let windows = compatible_windows(raw_windows, &ax_windows, Some(98));

        assert_eq!(
            windows.iter().map(|window| window.id).collect::<Vec<_>>(),
            vec![98, 266, 75, 109]
        );
        assert!(windows[0].has_focus);
        assert!(windows[0].is_visible);
        assert!(windows[1].is_visible);
        assert!(!windows[2].is_visible);
        assert!(!windows[3].is_visible);
    }

    #[test]
    fn compatible_windows_keeps_cg_order_when_focus_unknown() {
        let raw_windows = vec![
            test_raw_window(280, "App", true),
            test_raw_window(98, "kitty", true),
        ];
        let ax_windows = HashMap::from([
            (98, test_ax_info(11, "kitty", "AXStandardWindow")),
            (280, test_ax_info(12, "App", "AXStandardWindow")),
        ]);

        let windows = compatible_windows(raw_windows, &ax_windows, None);

        assert_eq!(
            windows.iter().map(|window| window.id).collect::<Vec<_>>(),
            vec![280, 98]
        );
        assert!(windows.iter().all(|window| !window.has_focus));
    }

    #[test]
    fn compatible_windows_exclude_non_standard_ax_windows() {
        let ax_windows = HashMap::from([(7, test_ax_info(8, "System", "AXSystemDialog"))]);

        assert!(compatible_windows(Vec::new(), &ax_windows, None).is_empty());
    }

    #[test]
    fn compatible_windows_leaves_ax_windows_available_for_focus() {
        let raw_windows = vec![test_raw_window(42, "App", true)];
        let ax_windows = HashMap::from([(42, test_ax_info(8, "App", "AXStandardWindow"))]);

        let windows = compatible_windows(raw_windows, &ax_windows, Some(42));

        assert_eq!(windows[0].id, 42);
        assert!(ax_windows.contains_key(&42));
    }

    #[test]
    fn raw_window_applications_keep_non_empty_name_for_duplicate_pid() {
        let mut first = test_raw_window(1, "", true);
        first.pid = 10;
        let mut second = test_raw_window(2, "Edge", false);
        second.pid = 10;

        let applications = raw_window_applications(&[first, second]);

        assert_eq!(applications, HashMap::from([(10, "Edge".to_string())]));
    }

    #[test]
    fn frontmost_raw_window_applications_adds_frontmost_without_raw_window() {
        let frontmost = FrontmostApplication {
            pid: 10,
            name: "Edge".to_string(),
        };

        let applications = frontmost_raw_window_applications(&[], &frontmost);

        assert_eq!(applications, HashMap::from([(10, "Edge".to_string())]));
    }

    #[test]
    fn focus_raw_window_level_skips_only_known_system_overlays() {
        assert!(is_focus_raw_window_level(0, "Dock"));
        assert!(is_focus_raw_window_level(24, "Deskflow"));
        assert!(is_focus_raw_window_level(24, "Edge"));
        assert!(!is_focus_raw_window_level(24, "Dock"));
        assert!(!is_focus_raw_window_level(24, "Control Center"));
        assert!(!is_focus_raw_window_level(24, "SystemUIServer"));
    }

    fn test_window(id: i32, app: &str, has_focus: bool) -> Window {
        let mut window =
            test_raw_window(id, app, true).into_window(None, has_focus.then_some(id as CGWindowID));
        window.has_ax_reference = true;
        window
    }

    fn test_focus_candidate(id: i32, app: &str, has_focus: bool) -> FocusCandidate {
        FocusCandidate {
            id,
            space: 1,
            app: app.to_string(),
            has_focus,
        }
    }

    fn test_raw_window(id: i32, app: &str, is_visible: bool) -> RawWindow {
        RawWindow {
            id,
            pid: 8,
            app: app.to_string(),
            title: "Title".to_string(),
            frame: Frame {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            },
            level: 0,
            opacity: 1.0,
            is_visible,
        }
    }

    fn test_ax_info(pid: pid_t, app: &str, subrole: &str) -> AxInfo {
        AxInfo {
            pid,
            app: app.to_string(),
            element: std::ptr::null(),
            role: Some("AXWindow".to_string()),
            subrole: Some(subrole.to_string()),
            title: None,
            minimized: None,
        }
    }
}
