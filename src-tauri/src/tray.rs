use crate::lifecycle::{Phase, RuntimeSnapshot};
use tauri::{
    menu::{Menu, MenuId, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Manager, Wry,
};

pub const TRAY_ID: &str = "reflexdesk-tray";

pub struct TrayUi {
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
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

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("ReflexDesk");

    if let Some(icon) = app.default_window_icon() {
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

    app.manage(TrayUi { status, toggle });
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

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tooltip = format!("ReflexDesk — {}", status_text.trim_start_matches("● "));
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

pub fn is_action(id: &MenuId, action: &str) -> bool {
    id == action
}
