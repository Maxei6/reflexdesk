use crate::lifecycle::{Phase, RuntimeSnapshot};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{Menu, MenuId, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Manager, Wry,
};

pub const TRAY_ID: &str = "reflexdesk-tray";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayState {
    Idle,
    Ready,
    Listening,
    Working,
    Attention,
}

impl TrayState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Ready => "ready",
            Self::Listening => "listening",
            Self::Working => "working",
            Self::Attention => "attention",
        }
    }
}

pub fn phase_to_tray_state(phase: Phase, listening: bool) -> TrayState {
    match phase {
        Phase::Booting | Phase::ShuttingDown => TrayState::Idle,
        Phase::SetupRequired
        | Phase::ConfirmationRequired
        | Phase::Degraded
        | Phase::Error => TrayState::Attention,
        Phase::Preparing
        | Phase::Transcribing
        | Phase::Routing
        | Phase::Executing
        | Phase::Updating => TrayState::Working,
        Phase::Listening => TrayState::Listening,
        Phase::Ready => {
            if listening {
                TrayState::Listening
            } else {
                TrayState::Ready
            }
        }
    }
}

pub fn get_tray_icon(state: TrayState) -> Result<Image<'static>, tauri::Error> {
    #[cfg(target_os = "macos")]
    let bytes: &'static [u8] = match state {
        TrayState::Idle => include_bytes!("../../assets/tray/idle-template-24.png"),
        TrayState::Ready => include_bytes!("../../assets/tray/ready-template-24.png"),
        TrayState::Listening => include_bytes!("../../assets/tray/listening-template-24.png"),
        TrayState::Working => include_bytes!("../../assets/tray/working-template-24.png"),
        TrayState::Attention => include_bytes!("../../assets/tray/attention-template-24.png"),
    };

    #[cfg(not(target_os = "macos"))]
    let bytes: &'static [u8] = match state {
        TrayState::Idle => include_bytes!("../../assets/tray/idle-24.png"),
        TrayState::Ready => include_bytes!("../../assets/tray/ready-24.png"),
        TrayState::Listening => include_bytes!("../../assets/tray/listening-24.png"),
        TrayState::Working => include_bytes!("../../assets/tray/working-24.png"),
        TrayState::Attention => include_bytes!("../../assets/tray/attention-24.png"),
    };

    Image::from_bytes(bytes)
}

pub struct TrayUi {
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    current_state: Mutex<Option<TrayState>>,
}

pub fn setup(app: &App) -> Result<(), String> {
    let status = MenuItem::with_id(
        app,
        "tray_status",
        "● Starting",
        false,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;

    let toggle = MenuItem::with_id(
        app,
        "tray_toggle",
        "Toggle listening",
        false,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;

    let settings = MenuItem::with_id(
        app,
        "tray_settings",
        "Open ReflexDesk",
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;

    let quit = MenuItem::with_id(
        app,
        "tray_quit",
        "Quit ReflexDesk",
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;

    let separator_one = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let separator_two = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &toggle,
            &separator_one,
            &settings,
            &separator_two,
            &quit,
        ],
    )
    .map_err(|e| e.to_string())?;

    let initial_state = TrayState::Idle;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("ReflexDesk");

    if let Ok(icon) = get_tray_icon(initial_state) {
        builder = builder.icon(icon);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    builder
        .on_menu_event(|app, event| {
            crate::handle_tray_action(app, event.id());
        })
        .build(app)
        .map_err(|e| e.to_string())?;

    app.manage(TrayUi {
        status,
        toggle,
        current_state: Mutex::new(Some(initial_state)),
    });
    Ok(())
}

pub fn update(app: &AppHandle, snapshot: &RuntimeSnapshot) {
    let ui = app.state::<TrayUi>();

    let status_text = match snapshot.phase {
        Phase::Booting => "● Starting",
        Phase::SetupRequired => "● Setup required",
        Phase::Preparing => "● Preparing local AI",
        Phase::Ready => "● Ready",
        Phase::Listening => "● Listening",
        Phase::Transcribing => "● Transcribing",
        Phase::Routing => "● Understanding",
        Phase::Executing => "● Working",
        Phase::ConfirmationRequired => "● Confirmation required",
        Phase::Degraded => "● Recovering",
        Phase::Error => "● Attention required",
        Phase::Updating => "● Updating",
        Phase::ShuttingDown => "● Shutting down",
    };

    let toggle_text = if snapshot.listening {
        "Stop listening"
    } else {
        "Start listening"
    };

    let toggle_enabled = snapshot.ready || snapshot.listening;

    let _ = ui.status.set_text(status_text);
    let _ = ui.toggle.set_text(toggle_text);
    let _ = ui.toggle.set_enabled(toggle_enabled);

    let next_state = phase_to_tray_state(snapshot.phase, snapshot.listening);
    let state_changed = {
        let mut guard = ui.current_state.lock().unwrap_or_else(|e| e.into_inner());
        if *guard != Some(next_state) {
            *guard = Some(next_state);
            true
        } else {
            false
        }
    };

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if state_changed {
            if let Ok(icon) = get_tray_icon(next_state) {
                let _ = tray.set_icon(Some(icon));
                #[cfg(target_os = "macos")]
                {
                    let _ = tray.set_icon_as_template(true);
                }
            }
        }
        let tooltip = format!("ReflexDesk — {}", status_text.trim_start_matches("● "));
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

pub fn is_action(id: &MenuId, action: &str) -> bool {
    id == action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_to_tray_state_mapping() {
        assert_eq!(phase_to_tray_state(Phase::Booting, false), TrayState::Idle);
        assert_eq!(phase_to_tray_state(Phase::ShuttingDown, false), TrayState::Idle);
        assert_eq!(phase_to_tray_state(Phase::Ready, false), TrayState::Ready);
        assert_eq!(phase_to_tray_state(Phase::Ready, true), TrayState::Listening);
        assert_eq!(phase_to_tray_state(Phase::Listening, true), TrayState::Listening);
        assert_eq!(phase_to_tray_state(Phase::Listening, false), TrayState::Listening);
        assert_eq!(phase_to_tray_state(Phase::Preparing, false), TrayState::Working);
        assert_eq!(phase_to_tray_state(Phase::Transcribing, false), TrayState::Working);
        assert_eq!(phase_to_tray_state(Phase::Routing, false), TrayState::Working);
        assert_eq!(phase_to_tray_state(Phase::Executing, false), TrayState::Working);
        assert_eq!(phase_to_tray_state(Phase::Updating, false), TrayState::Working);
        assert_eq!(phase_to_tray_state(Phase::SetupRequired, false), TrayState::Attention);
        assert_eq!(
            phase_to_tray_state(Phase::ConfirmationRequired, false),
            TrayState::Attention
        );
        assert_eq!(phase_to_tray_state(Phase::Degraded, false), TrayState::Attention);
        assert_eq!(phase_to_tray_state(Phase::Error, false), TrayState::Attention);
    }

    #[test]
    fn test_tray_icon_bytes_valid() {
        for state in [
            TrayState::Idle,
            TrayState::Ready,
            TrayState::Listening,
            TrayState::Working,
            TrayState::Attention,
        ] {
            let res = get_tray_icon(state);
            assert!(
                res.is_ok(),
                "Tray icon for {:?} should load successfully",
                state
            );
        }
    }
}
