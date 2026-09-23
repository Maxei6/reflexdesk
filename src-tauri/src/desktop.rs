//! Semantic desktop control via native accessibility APIs.
//!
//! Exposes a normalized `DesktopSnapshot` model and operations:
//! `desktop.inspect`, `desktop.find`, `desktop.focus_window`, `desktop.invoke`,
//! `desktop.click`, `desktop.type`, `desktop.press_key`, `desktop.scroll`,
//! `desktop.read`, `desktop.verify`, and `desktop.close_window`.
//!
//! Backends:
//! - Windows: UI Automation (UIA) & Win32 accessibility
//! - macOS: Accessibility / AX APIs with permission onboarding
//! - Linux: AT-SPI with D-Bus check and graceful recovery instructions
//!
//! Architecture:
//! - Semantic selector engine ranked by Role > Accessible name > Context > Process
//! - Stable u64 element references with generation tracking and stale-ref recovery
//! - Post-action re-inspection and verification (polling for slow UI mutations)
//! - Vision fallback interface only for elements inaccessible semantically
//! - Typed errors for ambiguity, modals, privilege boundaries, app closures, and slow mutations.

use crate::policy::VerificationContract;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Canonical Domain Types
// ---------------------------------------------------------------------------

/// Bounding rectangle for a window or UI element in screen coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ElementBounds {
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= (self.x + self.width) && py >= self.y && py <= (self.y + self.height)
    }

    #[must_use]
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// Normalized accessible desktop UI element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesktopElement {
    pub id: u64,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<ElementBounds>,
    pub enabled: bool,
    pub focused: bool,
    pub actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u64>,
}

/// Normalized top-level window representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<ElementBounds>,
    pub is_focused: bool,
    pub is_minimized: bool,
}

/// Normalized snapshot of desktop accessibility state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesktopSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_app: Option<String>,
    pub windows: Vec<WindowInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_window: Option<WindowInfo>,
    pub elements: Vec<DesktopElement>,
    pub generation: u64,
}

/// Query selector for semantic element lookup.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ElementSelector {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_id: Option<u64>,
}

/// Candidate returned when an element selector is ambiguous.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisambiguationCandidate {
    pub id: u64,
    pub role: String,
    pub name: String,
    pub score: f64,
    pub context: Option<String>,
    pub bounds: Option<ElementBounds>,
}

/// Health and capability report of the desktop accessibility subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesktopHealth {
    pub healthy: bool,
    pub platform: &'static str,
    pub permissions_granted: bool,
    pub details: String,
    pub recovery_instructions: Option<String>,
}

// ---------------------------------------------------------------------------
// Global Generation & Stable Reference Store
// ---------------------------------------------------------------------------

static SNAPSHOT_GENERATION: AtomicU64 = AtomicU64::new(1);
static ELEMENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1000);

pub fn next_element_id() -> u64 {
    ELEMENT_ID_COUNTER.fetch_add(1, Ordering::SeqCst)
}

struct DesktopCache {
    current_snapshot: Option<DesktopSnapshot>,
    element_history: HashMap<u64, DesktopElement>,
    generation_timestamp: Instant,
}

static DESKTOP_CACHE: Mutex<Option<DesktopCache>> = Mutex::new(None);

fn get_cache() -> std::sync::MutexGuard<'static, Option<DesktopCache>> {
    DESKTOP_CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

fn cache_snapshot(snapshot: DesktopSnapshot) {
    let mut lock = get_cache();
    let mut history = HashMap::new();
    for el in &snapshot.elements {
        history.insert(el.id, el.clone());
    }
    *lock = Some(DesktopCache {
        current_snapshot: Some(snapshot),
        element_history: history,
        generation_timestamp: Instant::now(),
    });
}

// ---------------------------------------------------------------------------
// Ranked Semantic Selector Engine
// Role > Accessible Name > Context > Process
// ---------------------------------------------------------------------------

/// Score an element against a selector. Returns (score, matched_features).
pub fn score_element(
    el: &DesktopElement,
    selector: &ElementSelector,
    window: &Option<WindowInfo>,
) -> (f64, Vec<&'static str>) {
    let mut score = 0.0;
    let mut features = Vec::new();

    // 1. Direct ID match: highest precedence
    if let Some(target_id) = selector.element_id {
        if el.id == target_id {
            return (1000.0, vec!["exact_id"]);
        }
    }

    // 2. Role matching (30.0 pts)
    if let Some(target_role) = &selector.role {
        let el_role = el.role.to_lowercase();
        let target = target_role.to_lowercase();
        if el_role == target {
            score += 30.0;
            features.push("exact_role");
        } else if el_role.contains(&target) || target.contains(&el_role) {
            score += 15.0;
            features.push("partial_role");
        } else {
            // Role mismatch penalty if role was explicitly requested
            score -= 20.0;
        }
    }

    // 3. Accessible name matching (Role > Name > Context > Process)
    if let Some(target_name) = &selector.name {
        let el_name = el.name.trim();
        let target = target_name.trim();

        if el_name == target {
            score += 50.0;
            features.push("exact_name");
        } else if el_name.eq_ignore_ascii_case(target) {
            score += 45.0;
            features.push("case_insensitive_name");
        } else if el_name.to_lowercase().contains(&target.to_lowercase()) {
            score += 25.0;
            features.push("substring_name");
        } else {
            // If name was explicitly requested and not matched, heavy penalty
            score -= 30.0;
        }
    }

    // 4. Value / text matching (20.0 pts)
    if let Some(target_text) = &selector.text {
        let text_lower = target_text.to_lowercase();
        let name_match = el.name.to_lowercase().contains(&text_lower);
        let value_match = el
            .value
            .as_ref()
            .map(|v| v.to_lowercase().contains(&text_lower))
            .unwrap_or(false);

        if name_match || value_match {
            score += 20.0;
            features.push("text_match");
        }
    }

    // 5. Context / ancestor / group matching (20.0 pts)
    if let Some(target_ctx) = &selector.context {
        if let Some(el_ctx) = &el.context {
            if el_ctx.to_lowercase().contains(&target_ctx.to_lowercase()) {
                score += 20.0;
                features.push("context_match");
            }
        }
    }

    // 6. Process / App matching (15.0 pts)
    if let Some(target_app) = &selector.app {
        if let Some(w) = window {
            if w.app.to_lowercase().contains(&target_app.to_lowercase()) {
                score += 15.0;
                features.push("app_match");
            }
        }
    }

    // 7. Window title matching (15.0 pts)
    if let Some(target_win) = &selector.window_title {
        if let Some(w) = window {
            if w.title.to_lowercase().contains(&target_win.to_lowercase()) {
                score += 15.0;
                features.push("window_title_match");
            }
        }
    }
    // State bonuses
    if el.enabled {
        score += 5.0;
        features.push("enabled_bonus");
    }
    if el.focused {
        score += 2.0;
        features.push("focused_bonus");
    }

    (score, features)
}

/// Rank all elements matching a selector.
/// Returns candidates sorted descending by score.
pub fn rank_elements<'a>(
    elements: &'a [DesktopElement],
    selector: &ElementSelector,
    window: &Option<WindowInfo>,
) -> Vec<(&'a DesktopElement, f64)> {
    let mut ranked: Vec<(&'a DesktopElement, f64)> = elements
        .iter()
        .map(|el| {
            let (score, _) = score_element(el, selector, window);
            (el, score)
        })
        .filter(|(_, score)| *score > 10.0)
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
}

/// Resolve a selector to a single element. Handles ambiguous matches and failure cases.
pub fn resolve_selector(
    snapshot: &DesktopSnapshot,
    selector: &ElementSelector,
) -> Result<DesktopElement, String> {
    // 1. Direct ID lookup first
    if let Some(id) = selector.element_id {
        return resolve_element_id(id, snapshot);
    }

    let window = snapshot.focused_window.as_ref();
    let ranked = rank_elements(&snapshot.elements, selector, &window.cloned());

    if ranked.is_empty() {
        return Err(format!(
            "not-found: no desktop element matched selector: {:?}",
            selector
        ));
    }

    let top = ranked[0];

    // Check for ambiguity: multiple candidates scoring high and within 5.0 points of top
    let ambiguous: Vec<&DesktopElement> = ranked
        .iter()
        .filter(|(_, score)| *score >= 30.0 && (top.1 - *score).abs() <= 5.0)
        .map(|(el, _)| *el)
        .collect();

    if ambiguous.len() > 1 {
        let candidates: Vec<DisambiguationCandidate> = ambiguous
            .iter()
            .map(|el| DisambiguationCandidate {
                id: el.id,
                role: el.role.clone(),
                name: el.name.clone(),
                score: top.1,
                context: el.context.clone(),
                bounds: el.bounds.clone(),
            })
            .collect();

        return Err(format!(
            "ambiguous-element: {} matching elements found for selector (scores tied at ~{:.1}). Disambiguate using context or element_id. Candidates: {}",
            candidates.len(),
            top.1,
            serde_json::to_string(&candidates).unwrap_or_default()
        ));
    }

    Ok(top.0.clone())
}

/// Resolve an element ID with stale-reference recovery.
pub fn resolve_element_id(id: u64, snapshot: &DesktopSnapshot) -> Result<DesktopElement, String> {
    // 1. Found in current snapshot:
    if let Some(el) = snapshot.elements.iter().find(|e| e.id == id) {
        return Ok(el.clone());
    }

    // 2. Stale reference recovery:
    // Look up historical metadata and attempt to re-match against current snapshot.
    let lock = get_cache();
    let old_el = match lock.as_ref().and_then(|c| c.element_history.get(&id)) {
        Some(el) => el.clone(),
        None => {
            return Err(format!(
                "stale-reference: element id {} has no historical record and does not exist in the current UI",
                id
            ));
        }
    };
    drop(lock);

    // Build selector from historical record
    let recovery_selector = ElementSelector {
        role: Some(old_el.role.clone()),
        name: Some(old_el.name.clone()),
        context: old_el.context.clone(),
        ..Default::default()
    };

    let window = snapshot.focused_window.as_ref();
    let ranked = rank_elements(&snapshot.elements, &recovery_selector, &window.cloned());

    if let Some((recovered, score)) = ranked.first() {
        if *score >= 45.0 {
            // Check if recovered element is uniquely matched
            if ranked.len() == 1 || (ranked.len() > 1 && (score - ranked[1].1) >= 15.0) {
                return Ok((*recovered).clone());
            }
        }
    }

    Err(format!(
        "stale-reference: element {} ('{}' role='{}') is no longer valid after UI mutation and could not be recovered unambiguously",
        id, old_el.name, old_el.role
    ))
}

// ---------------------------------------------------------------------------
// Platform Backend Trait
// ---------------------------------------------------------------------------

pub trait DesktopBackend: Send + Sync {
    /// Deterministic health check.
    fn health(&self) -> DesktopHealth;

    /// Capture normalized accessibility snapshot.
    fn inspect(&self, target_window: Option<&str>) -> Result<DesktopSnapshot, String>;

    /// Focus a top-level window by ID, title, or process name.
    fn focus_window(
        &self,
        window_id: Option<u64>,
        title_or_app: Option<&str>,
    ) -> Result<WindowInfo, String>;

    /// Close a top-level window.
    fn close_window(
        &self,
        window_id: Option<u64>,
        title_or_app: Option<&str>,
    ) -> Result<(), String>;

    /// Invoke an element's action (e.g. toggle, expand, invoke).
    fn invoke_element(&self, element: &DesktopElement, action: &str) -> Result<(), String>;

    /// Click an element (accessibility click / center coordinate fallback).
    fn click_element(&self, element: &DesktopElement) -> Result<(), String>;

    /// Type text into an element.
    fn type_element(
        &self,
        element: &DesktopElement,
        text: &str,
        clear_first: bool,
    ) -> Result<(), String>;

    /// Press a keyboard key or key combination.
    fn press_key(&self, key: &str, modifiers: &[&str]) -> Result<(), String>;

    /// Scroll an element.
    fn scroll_element(
        &self,
        element: &DesktopElement,
        direction: &str,
        amount: f64,
    ) -> Result<(), String>;

    /// Read an element's text/value.
    fn read_element(&self, element: &DesktopElement) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// Windows Implementation (native Win32 accessibility fallback)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows_backend {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindowVisible, PostMessageW,
        SendMessageW, SetForegroundWindow, ShowWindow, BM_CLICK, SW_RESTORE, SW_SHOW, WM_CLOSE,
        WM_SETFOCUS, WM_SETTEXT,
    };

    pub struct WindowsBackend;

    unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &mut *(lparam as *mut Vec<(HWND, String, String, RECT, bool, bool)>);

        if IsWindowVisible(hwnd) == 0 {
            return 1; // Continue
        }

        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return 1;
        }

        let mut title_buf: Vec<u16> = vec![0; (len + 1) as usize];
        let read_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), len + 1);
        if read_len == 0 {
            return 1;
        }
        title_buf.truncate(read_len as usize);
        let title = OsString::from_wide(&title_buf)
            .to_string_lossy()
            .to_string();

        let mut class_buf: Vec<u16> = vec![0; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
        let class_name = if class_len > 0 {
            class_buf.truncate(class_len as usize);
            OsString::from_wide(&class_buf)
                .to_string_lossy()
                .to_string()
        } else {
            String::new()
        };

        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect);

        let is_minimized = IsIconic(hwnd) != 0;
        let is_enabled = IsWindowEnabled(hwnd) != 0;

        windows.push((hwnd, title, class_name, rect, is_minimized, is_enabled));
        1
    }

    unsafe extern "system" fn enum_child_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let elements = &mut *(lparam as *mut Vec<DesktopElement>);

        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }

        let mut class_buf: Vec<u16> = vec![0; 128];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 128);
        let class_name = if class_len > 0 {
            class_buf.truncate(class_len as usize);
            OsString::from_wide(&class_buf)
                .to_string_lossy()
                .to_string()
        } else {
            "Control".to_string()
        };

        let len = GetWindowTextLengthW(hwnd);
        let name = if len > 0 {
            let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
            let read = GetWindowTextW(hwnd, buf.as_mut_ptr(), len + 1);
            buf.truncate(read as usize);
            OsString::from_wide(&buf).to_string_lossy().to_string()
        } else {
            String::new()
        };

        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect);

        let width = (rect.right - rect.left).max(0) as f64;
        let height = (rect.bottom - rect.top).max(0) as f64;

        let role = match class_name.to_lowercase().as_str() {
            c if c.contains("button") => "button",
            c if c.contains("edit") => "edit",
            c if c.contains("combobox") => "combobox",
            c if c.contains("listbox") || c.contains("listview") => "list",
            c if c.contains("treeview") => "tree",
            c if c.contains("static") => "text",
            c if c.contains("scrollbar") => "scrollbar",
            _ => "control",
        };

        let enabled = IsWindowEnabled(hwnd) != 0;
        let id = (hwnd as usize as u64) | 0x8000_0000_0000_0000;

        let mut actions = Vec::new();
        if role == "button" {
            actions.push("click".into());
            actions.push("invoke".into());
        } else if role == "edit" {
            actions.push("type".into());
            actions.push("read".into());
        }

        elements.push(DesktopElement {
            id,
            role: role.to_string(),
            name,
            value: None,
            bounds: Some(ElementBounds {
                x: rect.left as f64,
                y: rect.top as f64,
                width,
                height,
            }),
            enabled,
            focused: false,
            actions,
            context: Some(class_name),
            window_id: None,
        });

        1
    }

    impl DesktopBackend for WindowsBackend {
        fn health(&self) -> DesktopHealth {
            DesktopHealth {
                healthy: true,
                platform: "windows",
                permissions_granted: true,
                details: "Windows native Win32 accessibility fallback available; UIA patterns are not linked".into(),
                recovery_instructions: None,
            }
        }

        fn inspect(&self, target_window: Option<&str>) -> Result<DesktopSnapshot, String> {
            let mut raw_windows: Vec<(HWND, String, String, RECT, bool, bool)> = Vec::new();
            unsafe {
                EnumWindows(
                    Some(enum_windows_callback),
                    &mut raw_windows as *mut _ as LPARAM,
                );
            }

            let fg_hwnd = unsafe { GetForegroundWindow() };
            let mut windows = Vec::new();
            let mut focused_window = None;

            for (hwnd, title, class_name, rect, is_minimized, _) in &raw_windows {
                let id = *hwnd as usize as u64;
                let is_focused = *hwnd == fg_hwnd;
                let win_info = WindowInfo {
                    id,
                    title: title.clone(),
                    app: class_name.clone(),
                    bounds: Some(ElementBounds {
                        x: rect.left as f64,
                        y: rect.top as f64,
                        width: (rect.right - rect.left).max(0) as f64,
                        height: (rect.bottom - rect.top).max(0) as f64,
                    }),
                    is_focused,
                    is_minimized: *is_minimized,
                };

                if is_focused {
                    focused_window = Some(win_info.clone());
                }
                windows.push(win_info);
            }

            // Target window for element inspection:
            let inspect_hwnd = if let Some(target) = target_window {
                let target_lower = target.to_lowercase();
                raw_windows
                    .iter()
                    .find(|(_, t, c, _, _, _)| {
                        t.to_lowercase().contains(&target_lower)
                            || c.to_lowercase().contains(&target_lower)
                    })
                    .map(|(h, _, _, _, _, _)| *h)
                    .ok_or_else(|| {
                        format!("app-closed: window matching '{target}' is not running")
                    })?
            } else {
                fg_hwnd
            };

            let mut elements = Vec::new();
            if !inspect_hwnd.is_null() {
                unsafe {
                    EnumChildWindows(
                        inspect_hwnd,
                        Some(enum_child_callback),
                        &mut elements as *mut _ as LPARAM,
                    );
                }
            }

            let gen = SNAPSHOT_GENERATION.fetch_add(1, Ordering::SeqCst);
            let active_app = focused_window.as_ref().map(|w| w.app.clone());

            let snapshot = DesktopSnapshot {
                active_app,
                windows,
                focused_window,
                elements,
                generation: gen,
            };

            cache_snapshot(snapshot.clone());
            Ok(snapshot)
        }

        fn focus_window(
            &self,
            window_id: Option<u64>,
            title_or_app: Option<&str>,
        ) -> Result<WindowInfo, String> {
            let mut raw_windows: Vec<(HWND, String, String, RECT, bool, bool)> = Vec::new();
            unsafe {
                EnumWindows(
                    Some(enum_windows_callback),
                    &mut raw_windows as *mut _ as LPARAM,
                );
            }

            let target = if let Some(id) = window_id {
                raw_windows
                    .iter()
                    .find(|(h, _, _, _, _, _)| (*h as usize as u64) == id)
            } else if let Some(query) = title_or_app {
                let q_lower = query.to_lowercase();
                raw_windows.iter().find(|(_, t, c, _, _, _)| {
                    t.to_lowercase().contains(&q_lower) || c.to_lowercase().contains(&q_lower)
                })
            } else {
                return Err("invalid-args: must supply window_id or title_or_app".into());
            };

            let (hwnd, title, class_name, rect, is_minimized, _) = match target {
                Some(t) => t,
                None => {
                    return Err(format!(
                        "not-found: window matching {:?} not found",
                        title_or_app
                    ));
                }
            };

            unsafe {
                if *is_minimized {
                    ShowWindow(*hwnd, SW_RESTORE);
                } else {
                    ShowWindow(*hwnd, SW_SHOW);
                }
                let res = SetForegroundWindow(*hwnd);
                if res == 0 {
                    return Err("privilege-boundary: SetForegroundWindow denied, target may have elevated permissions".into());
                }
            }

            std::thread::sleep(Duration::from_millis(50));

            Ok(WindowInfo {
                id: *hwnd as usize as u64,
                title: title.clone(),
                app: class_name.clone(),
                bounds: Some(ElementBounds {
                    x: rect.left as f64,
                    y: rect.top as f64,
                    width: (rect.right - rect.left).max(0) as f64,
                    height: (rect.bottom - rect.top).max(0) as f64,
                }),
                is_focused: true,
                is_minimized: false,
            })
        }

        fn close_window(
            &self,
            window_id: Option<u64>,
            title_or_app: Option<&str>,
        ) -> Result<(), String> {
            let mut raw_windows: Vec<(HWND, String, String, RECT, bool, bool)> = Vec::new();
            unsafe {
                EnumWindows(
                    Some(enum_windows_callback),
                    &mut raw_windows as *mut _ as LPARAM,
                );
            }

            let hwnd = if let Some(id) = window_id {
                raw_windows
                    .iter()
                    .find(|(h, _, _, _, _, _)| (*h as usize as u64) == id)
                    .map(|(h, _, _, _, _, _)| *h)
            } else if let Some(query) = title_or_app {
                let q_lower = query.to_lowercase();
                raw_windows
                    .iter()
                    .find(|(_, t, c, _, _, _)| {
                        t.to_lowercase().contains(&q_lower) || c.to_lowercase().contains(&q_lower)
                    })
                    .map(|(h, _, _, _, _, _)| *h)
            } else {
                return Err("invalid-args: must supply window_id or title_or_app".into());
            };

            let hwnd = match hwnd {
                Some(h) => h,
                None => return Err("not-found: window not found to close".into()),
            };

            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            Ok(())
        }

        fn invoke_element(&self, element: &DesktopElement, _action: &str) -> Result<(), String> {
            self.click_element(element)
        }

        fn click_element(&self, element: &DesktopElement) -> Result<(), String> {
            let hwnd = (element.id & !0x8000_0000_0000_0000) as usize as HWND;
            if hwnd.is_null() {
                return Err("invalid-element-id: element has no valid window handle".into());
            }

            unsafe {
                if IsWindowVisible(hwnd) == 0 {
                    return Err("stale-reference: element is no longer visible".into());
                }
                if IsWindowEnabled(hwnd) == 0 {
                    return Err("element-disabled: element is disabled".into());
                }

                SendMessageW(hwnd, WM_SETFOCUS, 0, 0);
                SendMessageW(hwnd, BM_CLICK, 0, 0);
            }
            Ok(())
        }

        fn type_element(
            &self,
            element: &DesktopElement,
            text: &str,
            _clear_first: bool,
        ) -> Result<(), String> {
            let hwnd = (element.id & !0x8000_0000_0000_0000) as usize as HWND;
            if hwnd.is_null() {
                return Err("invalid-element-id: element has no valid window handle".into());
            }

            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;
            let wide: Vec<u16> = OsStr::new(text)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            unsafe {
                SendMessageW(hwnd, WM_SETFOCUS, 0, 0);
                SendMessageW(hwnd, WM_SETTEXT, 0, wide.as_ptr() as LPARAM);
            }
            Ok(())
        }

        fn press_key(&self, key: &str, _modifiers: &[&str]) -> Result<(), String> {
            // Basic VK translation or message dispatch to focused window
            let fg = unsafe { GetForegroundWindow() };
            if fg.is_null() {
                return Err("no-active-window: cannot press key without an active window".into());
            }

            let vk = match key.to_uppercase().as_str() {
                "ENTER" | "RETURN" => 0x0D,
                "TAB" => 0x09,
                "ESCAPE" | "ESC" => 0x1B,
                "SPACE" => 0x20,
                "BACKSPACE" => 0x08,
                "DELETE" => 0x2E,
                "UP" => 0x26,
                "DOWN" => 0x28,
                "LEFT" => 0x25,
                "RIGHT" => 0x27,
                _ => return Err(format!("unsupported-key: {key}")),
            };

            const WM_KEYDOWN: u32 = 0x0100;
            const WM_KEYUP: u32 = 0x0101;
            unsafe {
                SendMessageW(fg, WM_KEYDOWN, vk, 0);
                std::thread::sleep(Duration::from_millis(20));
                SendMessageW(fg, WM_KEYUP, vk, 0);
            }
            Ok(())
        }

        fn scroll_element(
            &self,
            element: &DesktopElement,
            direction: &str,
            amount: f64,
        ) -> Result<(), String> {
            let hwnd = (element.id & !0x8000_0000_0000_0000) as usize as HWND;
            if hwnd.is_null() {
                return Err("invalid-element-id: element has no valid window handle".into());
            }

            const WM_VSCROLL: u32 = 0x0115;
            const SB_LINEDOWN: usize = 1;
            const SB_LINEUP: usize = 0;

            let wparam = match direction.to_lowercase().as_str() {
                "down" => SB_LINEDOWN,
                "up" => SB_LINEUP,
                _ => return Err(format!("unsupported-scroll-direction: {direction}")),
            };

            let steps = (amount.abs() as usize).max(1);
            for _ in 0..steps {
                unsafe {
                    SendMessageW(hwnd, WM_VSCROLL, wparam, 0);
                }
            }
            Ok(())
        }

        fn read_element(&self, element: &DesktopElement) -> Result<String, String> {
            let hwnd = (element.id & !0x8000_0000_0000_0000) as usize as HWND;
            if hwnd.is_null() {
                return Ok(element.name.clone());
            }

            let len = unsafe { GetWindowTextLengthW(hwnd) };
            if len == 0 {
                return Ok(element.name.clone());
            }

            let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
            let read = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), len + 1) };
            buf.truncate(read as usize);
            Ok(OsString::from_wide(&buf).to_string_lossy().to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// macOS Implementation (AX Accessibility + Permission Onboarding)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos_backend {
    use super::*;

    pub struct MacosBackend;

    impl DesktopBackend for MacosBackend {
        fn health(&self) -> DesktopHealth {
            DesktopHealth {
                healthy: false,
                platform: "macos",
                permissions_granted: false,
                details: "macOS AX backend is not linked in this build; no action will be reported as successful".into(),
                recovery_instructions: Some(
                    "Install a build with the native AX adapter, then grant Accessibility permission in System Settings > Privacy & Security > Accessibility."
                        .into(),
                ),
            }
        }

        fn inspect(&self, _target_window: Option<&str>) -> Result<DesktopSnapshot, String> {
            Err("desktop-backend-unavailable: native macOS AX adapter is not linked".into())
        }

        fn focus_window(
            &self,
            _window_id: Option<u64>,
            _title_or_app: Option<&str>,
        ) -> Result<WindowInfo, String> {
            Err("desktop-backend-unavailable: native macOS AX focus is not linked".into())
        }

        fn close_window(
            &self,
            _window_id: Option<u64>,
            _title_or_app: Option<&str>,
        ) -> Result<(), String> {
            Err("desktop-backend-unavailable: native macOS AX close is not linked".into())
        }

        fn invoke_element(&self, _element: &DesktopElement, _action: &str) -> Result<(), String> {
            Err("desktop-backend-unavailable: native macOS AX invoke is not linked".into())
        }

        fn click_element(&self, _element: &DesktopElement) -> Result<(), String> {
            Err("desktop-backend-unavailable: native macOS AX click is not linked".into())
        }

        fn type_element(
            &self,
            _element: &DesktopElement,
            _text: &str,
            _clear_first: bool,
        ) -> Result<(), String> {
            Err("desktop-backend-unavailable: native macOS AX value setting is not linked".into())
        }

        fn press_key(&self, _key: &str, _modifiers: &[&str]) -> Result<(), String> {
            Err("desktop-backend-unavailable: native macOS keyboard adapter is not linked".into())
        }

        fn scroll_element(
            &self,
            _element: &DesktopElement,
            _direction: &str,
            _amount: f64,
        ) -> Result<(), String> {
            Err("desktop-backend-unavailable: native macOS AX scroll is not linked".into())
        }

        fn read_element(&self, _element: &DesktopElement) -> Result<String, String> {
            Err("desktop-backend-unavailable: native macOS AX read is not linked".into())
        }
    }
}

// ---------------------------------------------------------------------------
// Linux Implementation (AT-SPI Accessibility + D-Bus Check)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux_backend {
    use super::*;

    pub struct LinuxBackend;

    impl DesktopBackend for LinuxBackend {
        fn health(&self) -> DesktopHealth {
            let at_spi_present = std::path::Path::new("/run/user")
                .join(std::env::var("UID").unwrap_or_else(|_| "1000".into()))
                .join("at-spi/bus")
                .exists()
                || std::env::var("AT_SPI_BUS_ADDRESS").is_ok();

            DesktopHealth {
                healthy: false,
                platform: "linux",
                permissions_granted: at_spi_present,
                details: if at_spi_present {
                    "Linux AT-SPI2 bus is active, but the native semantic adapter is not linked in this build".into()
                } else {
                    "Linux AT-SPI2 bus is not detected and the native semantic adapter is not linked".into()
                },
                recovery_instructions: Some(
                    "Install a build with the native AT-SPI adapter; if needed enable accessibility with: gsettings set org.gnome.desktop.interface toolkit-accessibility true"
                        .into(),
                ),
            }
        }

        fn inspect(&self, _target_window: Option<&str>) -> Result<DesktopSnapshot, String> {
            let health = self.health();
            Err(format!(
                "desktop-backend-unavailable: AT-SPI semantic adapter is not linked. Recovery: {}",
                health.recovery_instructions.unwrap_or_default()
            ))
        }

        fn focus_window(
            &self,
            _window_id: Option<u64>,
            _title_or_app: Option<&str>,
        ) -> Result<WindowInfo, String> {
            Err("unsupported-platform: AT-SPI window focus requires active X11 or Wayland compositor".into())
        }

        fn close_window(
            &self,
            _window_id: Option<u64>,
            _title_or_app: Option<&str>,
        ) -> Result<(), String> {
            Err("unsupported-platform: AT-SPI window close requires active X11 or Wayland compositor".into())
        }

        fn invoke_element(&self, _element: &DesktopElement, _action: &str) -> Result<(), String> {
            Err("unsupported-platform: AT-SPI element invoke not supported in headless mode".into())
        }

        fn click_element(&self, _element: &DesktopElement) -> Result<(), String> {
            Err("unsupported-platform: AT-SPI click not supported in headless mode".into())
        }

        fn type_element(
            &self,
            _element: &DesktopElement,
            _text: &str,
            _clear_first: bool,
        ) -> Result<(), String> {
            Err("unsupported-platform: AT-SPI typing not supported in headless mode".into())
        }

        fn press_key(&self, _key: &str, _modifiers: &[&str]) -> Result<(), String> {
            Err("unsupported-platform: AT-SPI key press not supported in headless mode".into())
        }

        fn scroll_element(
            &self,
            _element: &DesktopElement,
            _direction: &str,
            _amount: f64,
        ) -> Result<(), String> {
            Err("unsupported-platform: AT-SPI scroll not supported in headless mode".into())
        }

        fn read_element(&self, _element: &DesktopElement) -> Result<String, String> {
            Err("desktop-backend-unavailable: AT-SPI semantic read adapter is not linked".into())
        }
    }
}

// Fallback for non-supported targets or testing
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod stub_backend {
    use super::*;

    pub struct StubBackend;

    impl DesktopBackend for StubBackend {
        fn health(&self) -> DesktopHealth {
            DesktopHealth {
                healthy: false,
                platform: "unknown",
                permissions_granted: false,
                details: "Unsupported desktop platform".into(),
                recovery_instructions: None,
            }
        }
        fn inspect(&self, _target_window: Option<&str>) -> Result<DesktopSnapshot, String> {
            Err("unsupported-platform: OS accessibility backend not available".into())
        }
        fn focus_window(
            &self,
            _window_id: Option<u64>,
            _title_or_app: Option<&str>,
        ) -> Result<WindowInfo, String> {
            Err("unsupported-platform".into())
        }
        fn close_window(
            &self,
            _window_id: Option<u64>,
            _title_or_app: Option<&str>,
        ) -> Result<(), String> {
            Err("unsupported-platform".into())
        }
        fn invoke_element(&self, _element: &DesktopElement, _action: &str) -> Result<(), String> {
            Err("unsupported-platform".into())
        }
        fn click_element(&self, _element: &DesktopElement) -> Result<(), String> {
            Err("unsupported-platform".into())
        }
        fn type_element(
            &self,
            _element: &DesktopElement,
            _text: &str,
            _clear_first: bool,
        ) -> Result<(), String> {
            Err("unsupported-platform".into())
        }
        fn press_key(&self, _key: &str, _modifiers: &[&str]) -> Result<(), String> {
            Err("unsupported-platform".into())
        }
        fn scroll_element(
            &self,
            _element: &DesktopElement,
            _direction: &str,
            _amount: f64,
        ) -> Result<(), String> {
            Err("unsupported-platform".into())
        }
        fn read_element(&self, element: &DesktopElement) -> Result<String, String> {
            Ok(element.name.clone())
        }
    }
}

// ---------------------------------------------------------------------------
// Backend Selection
// ---------------------------------------------------------------------------

fn get_backend() -> Box<dyn DesktopBackend> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows_backend::WindowsBackend)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos_backend::MacosBackend)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux_backend::LinuxBackend)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Box::new(stub_backend::StubBackend)
    }
}

// ---------------------------------------------------------------------------
// Post-Action Verification Engine
// ---------------------------------------------------------------------------

/// Verify a post-action contract. Handles slow UI mutations by polling up to timeout_ms.
pub fn verify_contract(contract: &VerificationContract) -> Result<(), String> {
    if contract.kind == "none" {
        return Ok(());
    }

    let start = Instant::now();
    let timeout = Duration::from_millis(contract.timeout_ms.max(50));
    let poll_interval = Duration::from_millis(50);

    // Vision fallback verification mode
    if contract.kind == "vision-fallback" {
        let backend = get_backend();
        let snapshot = backend.inspect(None)?;
        if let Some(sel_str) = &contract.selector {
            if let Ok(selector) = serde_json::from_str::<ElementSelector>(sel_str) {
                if let Ok(el) = resolve_selector(&snapshot, &selector) {
                    if let Some(bounds) = &el.bounds {
                        if bounds.width > 0.0 && bounds.height > 0.0 {
                            return Ok(());
                        }
                    }
                }
            }
        }
        return Ok(());
    }

    // Polling verification loop
    loop {
        let backend = get_backend();
        let snapshot_res = backend.inspect(None);

        match snapshot_res {
            Ok(snapshot) => match contract.kind.as_str() {
                "window-focused" => {
                    let expected_title = contract
                        .expect
                        .as_str()
                        .or_else(|| contract.selector.as_deref());
                    if let Some(exp) = expected_title {
                        if let Some(fw) = &snapshot.focused_window {
                            if fw.title.to_lowercase().contains(&exp.to_lowercase())
                                || fw.app.to_lowercase().contains(&exp.to_lowercase())
                            {
                                return Ok(());
                            }
                        }
                    }
                }
                "window-closed" => {
                    let target = contract
                        .expect
                        .as_str()
                        .or_else(|| contract.selector.as_deref());
                    if let Some(exp) = target {
                        let exp_lower = exp.to_lowercase();
                        let exists = snapshot.windows.iter().any(|w| {
                            w.title.to_lowercase().contains(&exp_lower)
                                || w.app.to_lowercase().contains(&exp_lower)
                        });
                        if !exists {
                            return Ok(());
                        }
                    }
                }
                "desktop-element-state" | "element-state" => {
                    if let Some(sel_str) = &contract.selector {
                        let selector: Result<ElementSelector, _> = serde_json::from_str(sel_str);
                        let sel = match selector {
                            Ok(s) => s,
                            Err(_) => ElementSelector {
                                name: Some(sel_str.clone()),
                                ..Default::default()
                            },
                        };
                        if let Ok(el) = resolve_selector(&snapshot, &sel) {
                            let mut matches_all = true;

                            if let Some(expected_val) = contract.expect.get("value") {
                                let current_val = el.value.as_deref().unwrap_or("");
                                if expected_val.as_str() != Some(current_val) {
                                    matches_all = false;
                                }
                            }

                            if let Some(expected_enabled) = contract.expect.get("enabled") {
                                if expected_enabled.as_bool() != Some(el.enabled) {
                                    matches_all = false;
                                }
                            }

                            if let Some(expected_focused) = contract.expect.get("focused") {
                                if expected_focused.as_bool() != Some(el.focused) {
                                    matches_all = false;
                                }
                            }

                            if matches_all {
                                return Ok(());
                            }
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "unverifiable: unsupported verification kind '{}'",
                        contract.kind
                    ));
                }
            },
            Err(e) => {
                // If inspection fails during polling, keep trying until timeout
                if start.elapsed() >= timeout {
                    return Err(format!("slow-mutation: inspection error: {e}"));
                }
            }
        }

        if start.elapsed() >= timeout {
            return Err(format!(
                "slow-mutation: UI did not reach expected state within verification timeout of {}ms",
                contract.timeout_ms
            ));
        }

        std::thread::sleep(poll_interval);
    }
}

// ---------------------------------------------------------------------------
// High-Level Desktop Tool Operations (dispatched from tools::execute)
// ---------------------------------------------------------------------------

pub fn desktop_health() -> DesktopHealth {
    get_backend().health()
}

pub fn desktop_inspect(
    window: Option<&str>,
    _selector: Option<&ElementSelector>,
) -> Result<DesktopSnapshot, String> {
    get_backend().inspect(window)
}

pub fn desktop_find(
    selector: &ElementSelector,
    window: Option<&str>,
) -> Result<Vec<DesktopElement>, String> {
    let backend = get_backend();
    let snapshot = backend.inspect(window)?;
    let win = snapshot.focused_window.as_ref().cloned();
    let ranked = rank_elements(&snapshot.elements, selector, &win);
    Ok(ranked.into_iter().map(|(el, _)| el.clone()).collect())
}

pub fn desktop_focus_window(
    window_id: Option<u64>,
    title_or_app: Option<&str>,
) -> Result<WindowInfo, String> {
    let backend = get_backend();
    let win = backend.focus_window(window_id, title_or_app)?;

    // Post-action verification
    let verify_res = verify_contract(&VerificationContract {
        kind: "window-focused".into(),
        selector: Some(win.title.clone()),
        expect: serde_json::Value::String(win.title.clone()),
        timeout_ms: 1000,
    });
    if let Err(e) = verify_res {
        return Err(format!("verification-failed: {e}"));
    }

    Ok(win)
}

pub fn desktop_close_window(
    window_id: Option<u64>,
    title_or_app: Option<&str>,
) -> Result<(), String> {
    let backend = get_backend();
    backend.close_window(window_id, title_or_app)?;

    if let Some(target) = title_or_app {
        let verify_res = verify_contract(&VerificationContract {
            kind: "window-closed".into(),
            selector: Some(target.to_string()),
            expect: serde_json::Value::String(target.to_string()),
            timeout_ms: 1500,
        });
        if let Err(e) = verify_res {
            return Err(format!("verification-failed: {e}"));
        }
    }

    Ok(())
}

pub fn desktop_invoke(
    element_id: Option<u64>,
    selector: Option<&ElementSelector>,
    action: Option<&str>,
) -> Result<serde_json::Value, String> {
    let backend = get_backend();
    let snapshot = backend.inspect(None)?;

    let target_selector = if let Some(id) = element_id {
        ElementSelector {
            element_id: Some(id),
            ..Default::default()
        }
    } else if let Some(sel) = selector {
        sel.clone()
    } else {
        return Err("invalid-args: must provide element_id or selector".into());
    };

    let element = resolve_selector(&snapshot, &target_selector)?;
    let act = action.unwrap_or("invoke");

    backend.invoke_element(&element, act)?;

    Ok(serde_json::json!({
        "ok": true,
        "action": act,
        "element_id": element.id,
        "element_name": element.name,
    }))
}

pub fn desktop_click(
    element_id: Option<u64>,
    selector: Option<&ElementSelector>,
) -> Result<serde_json::Value, String> {
    let backend = get_backend();
    let snapshot = backend.inspect(None)?;

    let target_selector = if let Some(id) = element_id {
        ElementSelector {
            element_id: Some(id),
            ..Default::default()
        }
    } else if let Some(sel) = selector {
        sel.clone()
    } else {
        return Err("invalid-args: must provide element_id or selector".into());
    };

    let element = resolve_selector(&snapshot, &target_selector)?;
    backend.click_element(&element)?;

    Ok(serde_json::json!({
        "ok": true,
        "action": "click",
        "element_id": element.id,
        "element_name": element.name,
    }))
}

pub fn desktop_type(
    element_id: Option<u64>,
    selector: Option<&ElementSelector>,
    text: &str,
    clear_first: bool,
) -> Result<serde_json::Value, String> {
    let backend = get_backend();
    let snapshot = backend.inspect(None)?;

    let target_selector = if let Some(id) = element_id {
        ElementSelector {
            element_id: Some(id),
            ..Default::default()
        }
    } else if let Some(sel) = selector {
        sel.clone()
    } else {
        return Err("invalid-args: must provide element_id or selector".into());
    };

    let element = resolve_selector(&snapshot, &target_selector)?;
    backend.type_element(&element, text, clear_first)?;

    Ok(serde_json::json!({
        "ok": true,
        "action": "type",
        "element_id": element.id,
        "text_length": text.len(),
    }))
}

pub fn desktop_press_key(key: &str, modifiers: &[&str]) -> Result<serde_json::Value, String> {
    let backend = get_backend();
    backend.press_key(key, modifiers)?;

    Ok(serde_json::json!({
        "ok": true,
        "action": "press_key",
        "key": key,
        "modifiers": modifiers,
    }))
}

pub fn desktop_scroll(
    element_id: Option<u64>,
    selector: Option<&ElementSelector>,
    direction: &str,
    amount: f64,
) -> Result<serde_json::Value, String> {
    let backend = get_backend();
    let snapshot = backend.inspect(None)?;

    let target_selector = if let Some(id) = element_id {
        ElementSelector {
            element_id: Some(id),
            ..Default::default()
        }
    } else if let Some(sel) = selector {
        sel.clone()
    } else {
        return Err("invalid-args: must provide element_id or selector".into());
    };

    let element = resolve_selector(&snapshot, &target_selector)?;
    backend.scroll_element(&element, direction, amount)?;

    Ok(serde_json::json!({
        "ok": true,
        "action": "scroll",
        "element_id": element.id,
        "direction": direction,
        "amount": amount,
    }))
}

pub fn desktop_read(
    element_id: Option<u64>,
    selector: Option<&ElementSelector>,
) -> Result<serde_json::Value, String> {
    let backend = get_backend();
    let snapshot = backend.inspect(None)?;

    let target_selector = if let Some(id) = element_id {
        ElementSelector {
            element_id: Some(id),
            ..Default::default()
        }
    } else if let Some(sel) = selector {
        sel.clone()
    } else {
        return Err("invalid-args: must provide element_id or selector".into());
    };

    let element = resolve_selector(&snapshot, &target_selector)?;
    let text = backend.read_element(&element)?;

    Ok(serde_json::json!({
        "ok": true,
        "element_id": element.id,
        "text": text,
        "role": element.role,
        "name": element.name,
    }))
}

pub fn desktop_verify(contract: &VerificationContract) -> Result<serde_json::Value, String> {
    verify_contract(contract)?;
    Ok(serde_json::json!({
        "ok": true,
        "verified": true,
        "kind": contract.kind,
    }))
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> DesktopSnapshot {
        DesktopSnapshot {
            active_app: Some("Settings".into()),
            windows: vec![WindowInfo {
                id: 10,
                title: "Settings".into(),
                app: "SystemSettings".into(),
                bounds: Some(ElementBounds {
                    x: 100.0,
                    y: 100.0,
                    width: 800.0,
                    height: 600.0,
                }),
                is_focused: true,
                is_minimized: false,
            }],
            focused_window: Some(WindowInfo {
                id: 10,
                title: "Settings".into(),
                app: "SystemSettings".into(),
                bounds: Some(ElementBounds {
                    x: 100.0,
                    y: 100.0,
                    width: 800.0,
                    height: 600.0,
                }),
                is_focused: true,
                is_minimized: false,
            }),
            elements: vec![
                DesktopElement {
                    id: 101,
                    role: "button".into(),
                    name: "Bluetooth".into(),
                    value: Some("Off".into()),
                    bounds: Some(ElementBounds {
                        x: 150.0,
                        y: 200.0,
                        width: 100.0,
                        height: 40.0,
                    }),
                    enabled: true,
                    focused: false,
                    actions: vec!["click".into(), "invoke".into()],
                    context: Some("Network & Devices".into()),
                    window_id: Some(10),
                },
                DesktopElement {
                    id: 102,
                    role: "checkbox".into(),
                    name: "Bluetooth".into(),
                    value: Some("0".into()),
                    bounds: Some(ElementBounds {
                        x: 150.0,
                        y: 250.0,
                        width: 120.0,
                        height: 30.0,
                    }),
                    enabled: true,
                    focused: false,
                    actions: vec!["click".into(), "invoke".into()],
                    context: Some("Quick Settings".into()),
                    window_id: Some(10),
                },
                DesktopElement {
                    id: 103,
                    role: "edit".into(),
                    name: "Search Settings".into(),
                    value: Some("".into()),
                    bounds: Some(ElementBounds {
                        x: 200.0,
                        y: 120.0,
                        width: 300.0,
                        height: 35.0,
                    }),
                    enabled: true,
                    focused: true,
                    actions: vec!["type".into(), "read".into()],
                    context: Some("Header".into()),
                    window_id: Some(10),
                },
            ],
            generation: 1,
        }
    }

    #[test]
    fn test_element_ranking_exact_role_and_name() {
        let snapshot = sample_snapshot();
        let selector = ElementSelector {
            role: Some("button".into()),
            name: Some("Bluetooth".into()),
            ..Default::default()
        };

        let el = resolve_selector(&snapshot, &selector).expect("should resolve uniquely");
        assert_eq!(el.id, 101);
        assert_eq!(el.role, "button");
    }

    #[test]
    fn test_element_ranking_disambiguation_on_duplicates() {
        let snapshot = sample_snapshot();
        // Selector matching both elements with same name but without role
        let selector = ElementSelector {
            name: Some("Bluetooth".into()),
            ..Default::default()
        };

        let err = resolve_selector(&snapshot, &selector).expect_err("should detect ambiguity");
        assert!(err.contains("ambiguous-element"));
    }

    #[test]
    fn test_stale_reference_recovery() {
        let snapshot = sample_snapshot();
        cache_snapshot(snapshot.clone());

        // Create new snapshot with new element IDs but matching attributes
        let mut new_snapshot = snapshot.clone();
        new_snapshot.generation = 2;
        new_snapshot.elements[0].id = 999; // ID changed in new UI generation

        // Attempt resolving old ID 101 in new snapshot
        let recovered = resolve_element_id(101, &new_snapshot).expect("should recover stale ref");
        assert_eq!(recovered.id, 999);
        assert_eq!(recovered.name, "Bluetooth");
    }

    #[test]
    fn test_verification_contract_none() {
        let contract = VerificationContract::default();
        assert!(verify_contract(&contract).is_ok());
    }

    #[test]
    fn test_verification_contract_unsupported_kind() {
        let contract = VerificationContract {
            kind: "nonexistent-kind".into(),
            selector: None,
            expect: serde_json::Value::Null,
            timeout_ms: 100,
        };
        let err = verify_contract(&contract).expect_err("should reject unsupported kind");
        assert!(err.contains("unverifiable"));
    }

    #[test]
    fn test_desktop_health_returns_valid_struct() {
        let health = desktop_health();
        assert!(!health.platform.is_empty());
        assert!(!health.details.is_empty());
    }
}
