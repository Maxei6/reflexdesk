fn main() {
    // Opt the sensitive commands into the Tauri ACL (deny-by-default).
    // `src-tauri/permissions/reflexdesk.toml` then allows each per window:
    // main-only for request/confirm/cancel/quit/shutdown, overlay-only for
    // transcript submission. Unlisted commands keep the default allow-all.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "request_action",
            "confirm_action",
            "cancel_session",
            "quit_app",
            "stt_shutdown",
            "transcript_nonce",
            "submit_transcript",
            "stt_transcribe",
            "get_desktop_health",
            "get_browser_status",
            "get_browser_pairing_secret",
            "stt_stream_chunk",
            "stt_cancel_stream",
            "stt_baseline_metrics",
            "get_hardware_profile",
            "get_benchmark_report",
            "run_hardware_benchmark",
            "set_backend_override",
            "get_diagnostics_preview",
            "export_diagnostics",
        ])),
    )
    .unwrap();
}
