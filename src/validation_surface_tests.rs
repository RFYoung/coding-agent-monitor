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

#[test]
fn path_only_surface_matches_are_weak_hints() {
    let evidence = validation_surface_evidence_for_path("frontend/components/App.tsx")
        .expect("web UI path hint");

    assert_eq!(evidence.surface, ValidationSurface::WebUi);
    assert_eq!(evidence.confidence, ValidationEvidenceConfidence::TextHint);
    assert!(!evidence.can_require_runtime_validation());
}

#[test]
fn project_profile_promotes_surface_matches_to_structural_evidence() {
    let mut profile = ProjectValidationProfile::default();
    profile.add_structural_surface(ValidationSurface::WebUi, "playwright.config.ts");

    let evidence = profile
        .validation_surface_evidence_for_path("frontend/components/App.tsx")
        .expect("web UI evidence");

    assert_eq!(evidence.surface, ValidationSurface::WebUi);
    assert_eq!(
        evidence.confidence,
        ValidationEvidenceConfidence::Structural
    );
    assert!(evidence.can_require_runtime_validation());
}
