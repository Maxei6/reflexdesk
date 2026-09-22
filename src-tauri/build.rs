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
        ])),
    )
    .unwrap();
}
