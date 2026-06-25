use crate::{Event, VerifierConfig, normalize_command_signature};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ValidationSurface {
    WebUi,
    MobileApp,
    NativeGui,
    SystemComponent,
    MlSystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ValidationEvidenceConfidence {
    TextHint,
    Observed,
    Structural,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidationSurfaceEvidence {
    pub(crate) surface: ValidationSurface,
    pub(crate) confidence: ValidationEvidenceConfidence,
    pub(crate) source: String,
}

impl ValidationSurfaceEvidence {
    pub(crate) fn can_require_runtime_validation(&self) -> bool {
        matches!(
            self.confidence,
            ValidationEvidenceConfidence::Observed
                | ValidationEvidenceConfidence::Structural
                | ValidationEvidenceConfidence::Explicit
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectValidationProfile {
    surface_evidence: Vec<ValidationSurfaceEvidence>,
}

impl ProjectValidationProfile {
    /// Discover project-level evidence that upgrades path/name matches from
    /// weak hints into trusted runtime-surface obligations.
    pub(crate) fn discover(workspace: &Path, verifiers: &[VerifierConfig]) -> Self {
        let mut profile = Self::default();
        profile.add_explicit_verifier_surfaces(verifiers);
        profile.add_structural_surfaces_from_workspace(workspace);
        profile
    }

    pub(crate) fn add_structural_surface(&mut self, surface: ValidationSurface, source: &str) {
        self.push_surface_evidence(ValidationSurfaceEvidence {
            surface,
            confidence: ValidationEvidenceConfidence::Structural,
            source: source.into(),
        });
    }

    pub(crate) fn validation_surface_evidence_for_path(
        &self,
        path: &str,
    ) -> Option<ValidationSurfaceEvidence> {
        let mut evidence = validation_surface_evidence_for_path(path)?;
        if let Some(stronger) = self.strongest_surface_evidence(evidence.surface) {
            evidence.confidence = stronger.confidence;
            evidence.source = stronger.source.clone();
        }
        Some(evidence)
    }

    fn add_explicit_verifier_surfaces(&mut self, verifiers: &[VerifierConfig]) {
        for verifier in verifiers {
            for pattern in &verifier.acceptance_patterns {
                if let Some(surface) = runtime_validation_surface_from_marker(pattern) {
                    self.push_surface_evidence(ValidationSurfaceEvidence {
                        surface,
                        confidence: ValidationEvidenceConfidence::Explicit,
                        source: format!("verifier `{}` acceptance pattern", verifier.id),
                    });
                }
            }
            for surface in validation_surfaces_for_command(&verifier.command) {
                self.push_surface_evidence(ValidationSurfaceEvidence {
                    surface,
                    confidence: ValidationEvidenceConfidence::Observed,
                    source: format!("verifier `{}` command", verifier.id),
                });
            }
        }
    }

    fn add_structural_surfaces_from_workspace(&mut self, workspace: &Path) {
        for (surface, candidates) in structural_surface_markers() {
            for candidate in candidates {
                if workspace.join(candidate).exists() {
                    self.add_structural_surface(surface, candidate);
                    break;
                }
            }
        }
    }

    fn strongest_surface_evidence(
        &self,
        surface: ValidationSurface,
    ) -> Option<&ValidationSurfaceEvidence> {
        self.surface_evidence
            .iter()
            .filter(|evidence| evidence.surface == surface)
            .max_by_key(|evidence| evidence.confidence)
    }

    fn push_surface_evidence(&mut self, evidence: ValidationSurfaceEvidence) {
        if self
            .strongest_surface_evidence(evidence.surface)
            .is_none_or(|existing| existing.confidence < evidence.confidence)
        {
            self.surface_evidence
                .retain(|existing| existing.surface != evidence.surface);
            self.surface_evidence.push(evidence);
        }
    }
}

impl ValidationSurface {
    pub(crate) fn change_label(self) -> &'static str {
        match self {
            ValidationSurface::WebUi => "web UI change",
            ValidationSurface::MobileApp => "mobile app change",
            ValidationSurface::NativeGui => "native GUI change",
            ValidationSurface::SystemComponent => "system component change",
            ValidationSurface::MlSystem => "ML system change",
        }
    }

    pub(crate) fn missing_evidence(self) -> &'static str {
        match self {
            ValidationSurface::WebUi => {
                "browser or Playwright validation after latest web UI change"
            }
            ValidationSurface::MobileApp => {
                "simulator/device validation after latest mobile app change"
            }
            ValidationSurface::NativeGui => {
                "native GUI smoke/e2e validation after latest GUI change"
            }
            ValidationSurface::SystemComponent => {
                "service or integration validation after latest system component change"
            }
            ValidationSurface::MlSystem => {
                "model evaluation or benchmark validation after latest ML system change"
            }
        }
    }

    pub(crate) fn packet_evidence_phrase(self) -> &'static str {
        match self {
            ValidationSurface::WebUi => {
                "browser/Playwright/Cypress-style evidence for the affected web UI"
            }
            ValidationSurface::MobileApp => "simulator/device evidence for the affected mobile app",
            ValidationSurface::NativeGui => {
                "native GUI smoke, e2e, or screenshot evidence for the affected desktop surface"
            }
            ValidationSurface::SystemComponent => {
                "service, integration, healthcheck, or daemon smoke evidence for the affected system component"
            }
            ValidationSurface::MlSystem => {
                "model evaluation, benchmark, golden-data, or inference smoke evidence for the affected ML system"
            }
        }
    }
}

pub(crate) fn is_ui_validation_relevant_file(path: &str) -> bool {
    validation_surface_for_path(path) == Some(ValidationSurface::WebUi)
}

pub(crate) fn validation_surface_for_path(path: &str) -> Option<ValidationSurface> {
    validation_surface_evidence_for_path(path).map(|evidence| evidence.surface)
}

pub(crate) fn validation_surface_evidence_for_path(
    path: &str,
) -> Option<ValidationSurfaceEvidence> {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if lower.starts_with("mobile/")
        || lower.starts_with("android/")
        || lower.starts_with("ios/")
        || lower.starts_with("react-native/")
        || lower.starts_with("flutter/")
        || lower.contains("/mobile/")
        || lower.contains("/android/")
        || lower.contains("/ios/")
        || lower.contains("/react-native/")
        || lower.contains("/flutter/")
        || [".swift", ".kt", ".kts", ".dart"]
            .iter()
            .any(|extension| file_name.ends_with(extension))
    {
        return Some(path_hint_evidence(ValidationSurface::MobileApp));
    }

    if lower.starts_with("ml/")
        || lower.starts_with("model/")
        || lower.starts_with("models/")
        || lower.starts_with("training/")
        || lower.starts_with("inference/")
        || lower.starts_with("evals/")
        || lower.contains("/ml/")
        || lower.contains("/model/")
        || lower.contains("/models/")
        || lower.contains("/training/")
        || lower.contains("/inference/")
        || lower.contains("/evals/")
        || file_name.ends_with(".ipynb")
    {
        return Some(path_hint_evidence(ValidationSurface::MlSystem));
    }

    if lower.starts_with("system/")
        || lower.starts_with("systemd/")
        || lower.starts_with("daemon/")
        || lower.starts_with("service/")
        || lower.starts_with("installer/")
        || lower.contains("/system/")
        || lower.contains("/systemd/")
        || lower.contains("/daemon/")
        || lower.contains("/installer/")
        || file_name.ends_with(".service")
    {
        return Some(path_hint_evidence(ValidationSurface::SystemComponent));
    }

    if lower.starts_with("desktop/")
        || lower.starts_with("gui/")
        || lower.starts_with("native-ui/")
        || lower.starts_with("tauri/")
        || lower.starts_with("src-tauri/")
        || lower.starts_with("wails/")
        || lower.contains("/desktop/")
        || lower.contains("/gui/")
        || lower.contains("/native-ui/")
        || lower.contains("/tauri/")
        || lower.contains("/src-tauri/")
        || lower.contains("/wails/")
    {
        return Some(path_hint_evidence(ValidationSurface::NativeGui));
    }

    if lower.starts_with("frontend/")
        || lower.starts_with("web/")
        || lower.starts_with("ui/")
        || lower.contains("/frontend/")
        || lower.contains("/web/")
        || lower.contains("/ui/")
        || lower.contains("/components/")
        || lower.contains("/pages/")
        || lower.contains("/views/")
        || [".vue", ".tsx", ".jsx", ".svelte"]
            .iter()
            .any(|extension| file_name.ends_with(extension))
    {
        return Some(path_hint_evidence(ValidationSurface::WebUi));
    }

    None
}

fn path_hint_evidence(surface: ValidationSurface) -> ValidationSurfaceEvidence {
    ValidationSurfaceEvidence {
        surface,
        confidence: ValidationEvidenceConfidence::TextHint,
        source: "path/name hint".into(),
    }
}

fn runtime_validation_surface_from_marker(pattern: &str) -> Option<ValidationSurface> {
    let marker = pattern.trim().to_ascii_lowercase();
    let kind = marker.strip_prefix("runtime_validation:")?;
    match kind {
        "web_ui" => Some(ValidationSurface::WebUi),
        "mobile_app" => Some(ValidationSurface::MobileApp),
        "native_gui" => Some(ValidationSurface::NativeGui),
        "system_component" => Some(ValidationSurface::SystemComponent),
        "ml_system" => Some(ValidationSurface::MlSystem),
        _ => None,
    }
}

fn structural_surface_markers() -> [(ValidationSurface, &'static [&'static str]); 5] {
    [
        (
            ValidationSurface::WebUi,
            &[
                "playwright.config.ts",
                "playwright.config.js",
                "playwright.config.mjs",
                "playwright.config.cjs",
                "cypress.config.ts",
                "cypress.config.js",
                "vite.config.ts",
                "vite.config.js",
                "next.config.js",
                "next.config.mjs",
            ],
        ),
        (
            ValidationSurface::MobileApp,
            &[
                "android/build.gradle",
                "android/app/build.gradle",
                "android/settings.gradle",
                "ios/Podfile",
                "ios/Runner.xcodeproj",
            ],
        ),
        (
            ValidationSurface::NativeGui,
            &["src-tauri/tauri.conf.json", "tauri.conf.json", "wails.json"],
        ),
        (
            ValidationSurface::SystemComponent,
            &[
                "docker-compose.yml",
                "docker-compose.yaml",
                "compose.yml",
                "compose.yaml",
                "systemd",
            ],
        ),
        (
            ValidationSurface::MlSystem,
            &["dvc.yaml", "mlflow", "MLproject"],
        ),
    ]
}

pub(crate) fn ordered_validation_surfaces() -> [ValidationSurface; 5] {
    [
        ValidationSurface::WebUi,
        ValidationSurface::MobileApp,
        ValidationSurface::NativeGui,
        ValidationSurface::SystemComponent,
        ValidationSurface::MlSystem,
    ]
}

pub(crate) fn push_validation_surface(
    surfaces: &mut Vec<ValidationSurface>,
    surface: ValidationSurface,
) {
    if !surfaces.contains(&surface) {
        surfaces.push(surface);
    }
}

pub(crate) fn validation_surfaces_for_event(event: &Event) -> Vec<ValidationSurface> {
    event
        .command
        .as_deref()
        .map(validation_surfaces_for_command)
        .unwrap_or_default()
}

pub(crate) fn validation_surfaces_for_command(command: &str) -> Vec<ValidationSurface> {
    let lower = normalize_command_signature(command).to_lowercase();
    let mut surfaces = Vec::new();
    for surface in ordered_validation_surfaces() {
        if validation_command_matches_surface(&lower, surface) {
            surfaces.push(surface);
        }
    }
    surfaces
}

fn validation_command_matches_surface(command: &str, surface: ValidationSurface) -> bool {
    let runtime_signal = has_runtime_validation_signal(command);
    match surface {
        ValidationSurface::WebUi => {
            is_browser_validation_command(command)
                || ["route check", "console check", "web validation"]
                    .iter()
                    .any(|signal| command.contains(signal))
        }
        ValidationSurface::MobileApp => [
            "maestro",
            "appium",
            "detox",
            "emulator",
            "simulator",
            "connectedandroidtest",
            "androidtest",
            "flutter drive",
            "xcodebuild test",
            "device test",
        ]
        .iter()
        .any(|signal| command.contains(signal)),
        ValidationSurface::NativeGui => {
            runtime_signal
                && ["gui", "desktop", "native", "tauri", "wails", "screenshot"]
                    .iter()
                    .any(|signal| command.contains(signal))
        }
        ValidationSurface::SystemComponent => {
            ["healthcheck", "health check", "systemd", "docker compose"]
                .iter()
                .any(|signal| command.contains(signal))
                || (runtime_signal
                    && ["service", "system", "container", "daemon"]
                        .iter()
                        .any(|signal| command.contains(signal)))
        }
        ValidationSurface::MlSystem => [
            "eval",
            "evaluation",
            "benchmark",
            "golden",
            "model eval",
            "mlflow",
            "inference smoke",
            "dataset check",
        ]
        .iter()
        .any(|signal| command.contains(signal)),
    }
}

fn has_runtime_validation_signal(command: &str) -> bool {
    ["smoke", "e2e", "end-to-end", "integration"]
        .iter()
        .any(|signal| command.contains(signal))
}

fn is_browser_validation_command(command: &str) -> bool {
    let lower = normalize_command_signature(command).to_lowercase();
    ["playwright", "cypress", "puppeteer", "browser", "webdriver"]
        .iter()
        .any(|signal| lower.contains(signal))
}

#[cfg(test)]
#[path = "validation_surface_tests.rs"]
mod tests;
