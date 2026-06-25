//! Tests for validation-surface inference, included into `validation_surface.rs`
//! via `#[path]` so they can reach the module's private helpers.

use super::*;

#[test]
fn generic_runtime_terms_do_not_imply_a_validation_surface() {
    for command in [
        "npm run e2e",
        "cargo test integration",
        "just smoke",
        "pnpm test end-to-end",
    ] {
        assert_eq!(
            validation_surfaces_for_command(command),
            Vec::<ValidationSurface>::new(),
            "{command} should require a surface qualifier or mapped verifier"
        );
    }
}

#[test]
fn surface_qualified_runtime_commands_map_to_specific_surfaces() {
    assert_eq!(
        validation_surfaces_for_command("cargo test desktop gui smoke"),
        vec![ValidationSurface::NativeGui]
    );
    assert_eq!(
        validation_surfaces_for_command("docker compose run service healthcheck smoke"),
        vec![ValidationSurface::SystemComponent]
    );
    assert_eq!(
        validation_surfaces_for_command("npx playwright test"),
        vec![ValidationSurface::WebUi]
    );
}
