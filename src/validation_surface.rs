use crate::{Event, normalize_command_signature};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ValidationSurface {
    WebUi,
    MobileApp,
    NativeGui,
    SystemComponent,
    MlSystem,
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
        return Some(ValidationSurface::MobileApp);
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
        return Some(ValidationSurface::MlSystem);
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
        return Some(ValidationSurface::SystemComponent);
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
        return Some(ValidationSurface::NativeGui);
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
        return Some(ValidationSurface::WebUi);
    }

    None
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
mod tests {
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
}
