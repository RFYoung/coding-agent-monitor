use coding_agent_monitor::{
    AgentActivityStatus, DashboardAdvisorCredentialKind, DashboardAdvisorStatus, DashboardFilter,
    DashboardOptions, DashboardRow, DashboardRowKind, DashboardSeverity, DashboardSnapshot,
    ProjectStore, RunningAgent, agent_kind_label, detect_running_agents_from_system,
    judge_snapshot,
};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};

const TRAY_SHOW_ID: &str = "agent-monitor-show";
const TRAY_HIDE_ID: &str = "agent-monitor-hide";
const TRAY_TOGGLE_ID: &str = "agent-monitor-toggle";
const TRAY_QUIT_ID: &str = "agent-monitor-quit";

/// Shared UI color palette. Keeping these in one place keeps status colors
/// consistent across the toolbar, cards, table, and attention band.
mod palette {
    use eframe::egui::Color32;

    pub const HEALTHY: Color32 = Color32::from_rgb(32, 122, 74);
    pub const WARNING: Color32 = Color32::from_rgb(171, 112, 18);
    pub const CRITICAL: Color32 = Color32::from_rgb(184, 43, 43);
    pub const NEUTRAL: Color32 = Color32::from_rgb(100, 112, 126);

    pub const ACCENT: Color32 = Color32::from_rgb(32, 120, 210);
    pub const PANEL_BG: Color32 = Color32::from_rgb(246, 248, 251);
    pub const CARD_BG: Color32 = Color32::WHITE;
    pub const CARD_BG_SUBTLE: Color32 = Color32::from_rgb(248, 250, 253);
    pub const CARD_BORDER: Color32 = Color32::from_rgb(220, 228, 238);
    pub const SELECTED_BG: Color32 = Color32::from_rgb(232, 241, 252);
    pub const ATTENTION_BG: Color32 = Color32::from_rgb(255, 252, 245);
    pub const ATTENTION_BORDER: Color32 = Color32::from_rgb(230, 218, 198);
}

fn main() -> eframe::Result {
    let options = parse_ui_options(std::env::args().skip(1));
    let native_options = eframe::NativeOptions {
        viewport: build_viewport(options.background),
        ..Default::default()
    };
    eframe::run_native(
        "Coding Agent Monitor",
        native_options,
        Box::new(move |cc| {
            configure_light_theme(&cc.egui_ctx);
            Ok(Box::new(MonitorDashboard::with_visibility(
                options.workspaces.clone(),
                !options.background,
            )))
        }),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UiOptions {
    workspaces: Vec<PathBuf>,
    background: bool,
}

#[derive(Debug, Clone)]
struct WorkspaceState {
    root: PathBuf,
    store_root: PathBuf,
    snapshot: DashboardSnapshot,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceStatus {
    Empty,
    Healthy,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FleetStatus {
    total: usize,
    empty: usize,
    healthy: usize,
    warning: usize,
    critical: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttentionLevel {
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttentionItem {
    level: AttentionLevel,
    workspace: PathBuf,
    message: String,
}

struct MonitorDashboard {
    workspaces: Vec<WorkspaceState>,
    active_workspace: usize,
    last_refresh: Instant,
    refresh_interval: Duration,
    workspace_input: String,
    display_filter: String,
    filter_error: Option<String>,
    selected_row: Option<usize>,
    tray: Option<TrayHandle>,
    window_visible: bool,
    quit_requested: bool,
    running_agents: Vec<RunningAgent>,
}

impl MonitorDashboard {
    fn with_visibility(workspace_roots: Vec<PathBuf>, window_visible: bool) -> Self {
        let workspaces = normalize_workspaces(workspace_roots)
            .into_iter()
            .map(WorkspaceState::new)
            .collect::<Vec<_>>();
        let mut dashboard = Self {
            workspaces,
            active_workspace: 0,
            last_refresh: Instant::now() - Duration::from_secs(60),
            refresh_interval: Duration::from_millis(750),
            workspace_input: String::new(),
            display_filter: String::new(),
            filter_error: None,
            selected_row: None,
            tray: build_tray_icon(),
            window_visible,
            quit_requested: false,
            running_agents: detect_running_agents_from_system(),
        };
        dashboard.refresh_all();
        dashboard
    }

    fn active_workspace(&self) -> &WorkspaceState {
        &self.workspaces[self.active_workspace]
    }

    fn active_workspace_mut(&mut self) -> &mut WorkspaceState {
        &mut self.workspaces[self.active_workspace]
    }

    fn refresh(&mut self) {
        let root = self.active_workspace().root.clone();
        let result = ProjectStore::open(&root).and_then(|store| {
            DashboardSnapshot::load_with_options(
                store.root(),
                DashboardOptions {
                    recent_limit: 500,
                    now: current_utc_timestamp(),
                    stale_after_secs: Some(120),
                },
            )
        });
        match result {
            Ok(snapshot) => {
                let workspace = self.active_workspace_mut();
                workspace.store_root = root.join(".agent-monitor");
                workspace.snapshot = snapshot;
                workspace.last_error = None;
            }
            Err(error) => {
                self.active_workspace_mut().last_error = Some(error.to_string());
            }
        }
        self.last_refresh = Instant::now();
    }

    fn refresh_all(&mut self) {
        for workspace in &mut self.workspaces {
            let root = workspace.root.clone();
            let result = ProjectStore::open(&root).and_then(|store| {
                DashboardSnapshot::load_with_options(
                    store.root(),
                    DashboardOptions {
                        recent_limit: 500,
                        now: current_utc_timestamp(),
                        stale_after_secs: Some(120),
                    },
                )
            });
            match result {
                Ok(snapshot) => {
                    workspace.store_root = root.join(".agent-monitor");
                    workspace.snapshot = snapshot;
                    workspace.last_error = None;
                }
                Err(error) => workspace.last_error = Some(error.to_string()),
            }
        }
        self.running_agents = detect_running_agents_from_system();
        self.last_refresh = Instant::now();
    }

    fn add_workspace(&mut self, root: PathBuf) {
        if root.as_os_str().is_empty() {
            return;
        }
        if let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.root == root)
        {
            self.active_workspace = index;
        } else {
            self.workspaces.push(WorkspaceState::new(root));
            self.active_workspace = self.workspaces.len() - 1;
        }
        self.selected_row = None;
        self.refresh();
    }

    fn set_window_visible(&mut self, ctx: &egui::Context, visible: bool) {
        self.window_visible = visible;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
        if visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }
}

impl WorkspaceState {
    fn new(root: PathBuf) -> Self {
        Self {
            store_root: root.join(".agent-monitor"),
            root,
            snapshot: empty_snapshot(),
            last_error: None,
        }
    }
}

fn workspace_status(workspace: &WorkspaceState) -> WorkspaceStatus {
    if workspace.last_error.is_some() || workspace.snapshot.severity == DashboardSeverity::Critical
    {
        return WorkspaceStatus::Critical;
    }
    if workspace.snapshot.event_count == 0
        && workspace.snapshot.intervention_count == 0
        && workspace.snapshot.design_count == 0
        && workspace.snapshot.trace_count == 0
    {
        return WorkspaceStatus::Empty;
    }
    if workspace.snapshot.severity == DashboardSeverity::Warning
        || workspace
            .snapshot
            .agent_sessions
            .iter()
            .any(|session| session.status != AgentActivityStatus::Active)
    {
        return WorkspaceStatus::Warning;
    }
    WorkspaceStatus::Healthy
}

fn workspace_status_label(status: WorkspaceStatus) -> (&'static str, egui::Color32) {
    match status {
        WorkspaceStatus::Empty => ("Empty", palette::NEUTRAL),
        WorkspaceStatus::Healthy => ("Healthy", palette::HEALTHY),
        WorkspaceStatus::Warning => ("Warning", palette::WARNING),
        WorkspaceStatus::Critical => ("Critical", palette::CRITICAL),
    }
}

fn fleet_status(workspaces: &[WorkspaceState]) -> FleetStatus {
    let mut status = FleetStatus {
        total: workspaces.len(),
        ..FleetStatus::default()
    };
    for workspace in workspaces {
        match workspace_status(workspace) {
            WorkspaceStatus::Empty => status.empty += 1,
            WorkspaceStatus::Healthy => status.healthy += 1,
            WorkspaceStatus::Warning => status.warning += 1,
            WorkspaceStatus::Critical => status.critical += 1,
        }
    }
    status
}

fn fleet_status_label(status: FleetStatus) -> (&'static str, egui::Color32) {
    if status.critical > 0 {
        ("Critical", palette::CRITICAL)
    } else if status.warning > 0 {
        ("Warning", palette::WARNING)
    } else if status.healthy > 0 {
        ("Healthy", palette::HEALTHY)
    } else {
        ("Empty", palette::NEUTRAL)
    }
}

fn fleet_summary_text(status: FleetStatus) -> String {
    if status.total == 0 {
        return "no workspaces configured".to_string();
    }
    let mut parts = Vec::new();
    if status.critical > 0 {
        parts.push(format!("{} critical", status.critical));
    }
    if status.warning > 0 {
        parts.push(format!("{} warning", status.warning));
    }
    if status.healthy > 0 {
        parts.push(format!("{} healthy", status.healthy));
    }
    if status.empty > 0 {
        parts.push(format!("{} empty", status.empty));
    }
    let noun = if status.total == 1 {
        "workspace"
    } else {
        "workspaces"
    };
    format!("{} {} · {}", status.total, noun, parts.join(", "))
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let prefix: String = trimmed.chars().take(keep).collect();
    format!("{}…", prefix.trim_end())
}

fn format_relative_age(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 2 {
        "just now".to_string()
    } else if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else {
        format!("{}h ago", seconds / 3_600)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CaptureSummary {
    total: usize,
    events: usize,
    interventions: usize,
    warning: usize,
    critical: usize,
}

fn capture_summary(rows: &[&DashboardRow]) -> CaptureSummary {
    let mut summary = CaptureSummary {
        total: rows.len(),
        ..CaptureSummary::default()
    };
    for row in rows {
        match row.kind {
            DashboardRowKind::Event => summary.events += 1,
            DashboardRowKind::Intervention => summary.interventions += 1,
            DashboardRowKind::VerifierRun => summary.events += 1,
            DashboardRowKind::ProbeRun => summary.events += 1,
            DashboardRowKind::RepoHunkFile => summary.events += 1,
            DashboardRowKind::RepoHunk => summary.events += 1,
            DashboardRowKind::Requirement => summary.events += 1,
            DashboardRowKind::DevHistory => summary.events += 1,
            DashboardRowKind::DecisionTrail => summary.events += 1,
            DashboardRowKind::WorktreeLock => summary.events += 1,
        }
        match row.severity {
            DashboardSeverity::Warning => summary.warning += 1,
            DashboardSeverity::Critical => summary.critical += 1,
            DashboardSeverity::Healthy => {}
        }
    }
    summary
}

fn capture_summary_text(summary: CaptureSummary) -> String {
    let noun = if summary.total == 1 { "row" } else { "rows" };
    format!(
        "{} {} · {} events · {} interventions · {} warning · {} critical",
        summary.total,
        noun,
        summary.events,
        summary.interventions,
        summary.warning,
        summary.critical
    )
}

fn review_summary_text(report: &coding_agent_monitor::AgentReviewReport) -> String {
    let status = match report.status {
        coding_agent_monitor::AgentReviewStatus::Ok => "OK",
        coding_agent_monitor::AgentReviewStatus::Watch => "Watch",
        coding_agent_monitor::AgentReviewStatus::Intervene => "Intervene",
    };
    let noun = if report.findings.len() == 1 {
        "finding"
    } else {
        "findings"
    };
    format!("{status} · {} {noun}", report.findings.len())
}

fn attention_items(workspaces: &[WorkspaceState]) -> Vec<AttentionItem> {
    let mut items = Vec::new();
    for workspace in workspaces {
        if let Some(error) = &workspace.last_error {
            items.push(AttentionItem {
                level: AttentionLevel::Critical,
                workspace: workspace.root.clone(),
                message: error.clone(),
            });
        }
        if workspace.snapshot.severity == DashboardSeverity::Critical
            && workspace.snapshot.advisor_status.severity != DashboardSeverity::Critical
        {
            items.push(AttentionItem {
                level: AttentionLevel::Critical,
                workspace: workspace.root.clone(),
                message: "workspace has critical monitor severity".into(),
            });
        }
        if let Some(item) = advisor_attention_item(workspace) {
            items.push(item);
        }
        for session in &workspace.snapshot.agent_sessions {
            match session.status {
                AgentActivityStatus::Degraded => items.push(AttentionItem {
                    level: AttentionLevel::Critical,
                    workspace: workspace.root.clone(),
                    message: format!(
                        "{} degraded: score {}, {} interventions",
                        session.agent, session.score, session.interventions
                    ),
                }),
                AgentActivityStatus::Stale => items.push(AttentionItem {
                    level: AttentionLevel::Warning,
                    workspace: workspace.root.clone(),
                    message: format!(
                        "{} stale since {}",
                        session.agent,
                        session.last_seen.as_deref().unwrap_or("unknown")
                    ),
                }),
                AgentActivityStatus::Active => {}
            }
        }
    }
    items.sort_by(|left, right| {
        attention_rank(left.level)
            .cmp(&attention_rank(right.level))
            .then_with(|| left.workspace.cmp(&right.workspace))
            .then_with(|| left.message.cmp(&right.message))
    });
    items
}

fn advisor_attention_item(workspace: &WorkspaceState) -> Option<AttentionItem> {
    let status = &workspace.snapshot.advisor_status;
    let level = match status.severity {
        DashboardSeverity::Healthy => return None,
        DashboardSeverity::Warning => AttentionLevel::Warning,
        DashboardSeverity::Critical => AttentionLevel::Critical,
    };
    Some(AttentionItem {
        level,
        workspace: workspace.root.clone(),
        message: format!("advisor: {}", status.message),
    })
}

fn attention_rank(level: AttentionLevel) -> u8 {
    match level {
        AttentionLevel::Critical => 0,
        AttentionLevel::Warning => 1,
    }
}

fn attention_level_label(level: AttentionLevel) -> (&'static str, egui::Color32) {
    match level {
        AttentionLevel::Critical => ("Critical", palette::CRITICAL),
        AttentionLevel::Warning => ("Warning", palette::WARNING),
    }
}

fn current_utc_timestamp() -> Option<String> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(format_utc_seconds(seconds))
}

fn format_utc_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

impl eframe::App for MonitorDashboard {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        handle_tray_events(ctx, self);
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // When a tray icon is available, closing the window minimizes to the tray
        // instead of quitting the supervisor process.
        if self.tray.is_some() && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.set_window_visible(ctx, false);
        }
        if self.last_refresh.elapsed() >= self.refresh_interval {
            self.refresh_all();
        }
        ctx.request_repaint_after(self.refresh_interval);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter()
            .rect_filled(ui.max_rect(), 0, palette::PANEL_BG);
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
        render_toolbar(ui, self);
        render_attention_band(ui, &self.workspaces);

        if let Some(error) = &self.active_workspace().last_error {
            ui.colored_label(palette::CRITICAL, error);
            ui.separator();
        }

        render_metrics(ui, &self.active_workspace().snapshot);

        ui.add_space(8.0);
        render_filter_bar(
            ui,
            &mut self.display_filter,
            &mut self.filter_error,
            &mut self.selected_row,
        );
        ui.separator();

        let snapshot = self.active_workspace().snapshot.clone();
        let review = judge_snapshot(
            self.active_workspace().root.clone(),
            &snapshot,
            &self.running_agents,
        );
        let rows = filtered_rows(&snapshot, &self.display_filter, &mut self.filter_error);
        ui.columns(2, |columns| {
            render_workspace_panel(&mut columns[0], self);
            columns[0].separator();
            render_review_panel(&mut columns[0], &review);
            columns[0].separator();
            render_advisor_panel(&mut columns[0], &snapshot.advisor_status);
            columns[0].separator();
            render_agent_panel(&mut columns[0], &snapshot, &self.running_agents);

            columns[1].horizontal(|ui| {
                ui.heading("Capture");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(capture_summary_text(capture_summary(&rows)));
                });
            });
            columns[1].separator();
            egui::ScrollArea::vertical()
                .id_salt("capture_rows")
                .max_height(360.0)
                .show(&mut columns[1], |ui| {
                    render_packet_table(ui, &rows, &mut self.selected_row);
                });
            columns[1].separator();
            render_details(&mut columns[1], &rows, self.selected_row);
            if let Some(error) = &self.filter_error {
                columns[1].separator();
                columns[1].colored_label(palette::CRITICAL, error);
            }
        });
    }
}

fn configure_light_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = palette::PANEL_BG;
    visuals.window_fill = egui::Color32::from_rgb(255, 255, 255);
    visuals.extreme_bg_color = egui::Color32::from_rgb(236, 241, 247);
    visuals.faint_bg_color = egui::Color32::from_rgb(242, 246, 250);
    visuals.selection.bg_fill = egui::Color32::from_rgb(206, 226, 250);
    visuals.hyperlink_color = palette::ACCENT;
    ctx.set_theme(egui::Theme::Light);
    ctx.set_visuals_of(egui::Theme::Light, visuals.clone());
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_global_style(style);
}

fn render_toolbar(ui: &mut egui::Ui, dashboard: &mut MonitorDashboard) {
    ui.horizontal(|ui| {
        ui.heading("Coding Agent Monitor");
        ui.separator();
        let fleet = fleet_status(&dashboard.workspaces);
        let (fleet_label, fleet_color) = fleet_status_label(fleet);
        status_pill(ui, fleet_label, fleet_color);
        ui.label(fleet_summary_text(fleet));
        ui.separator();
        ui.label("Workspace");
        ui.monospace(dashboard.active_workspace().root.display().to_string());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh All").clicked() {
                dashboard.refresh_all();
            }
            if ui.button("Refresh").clicked() {
                dashboard.refresh();
            }
            ui.weak(format!(
                "Updated {}",
                format_relative_age(dashboard.last_refresh.elapsed())
            ));
        });
    });
    ui.separator();
}

fn render_attention_band(ui: &mut egui::Ui, workspaces: &[WorkspaceState]) {
    let items = attention_items(workspaces);
    if items.is_empty() {
        return;
    }
    egui::Frame::new()
        .fill(palette::ATTENTION_BG)
        .stroke(egui::Stroke::new(1.0, palette::ATTENTION_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Attention");
                for item in items.iter().take(4) {
                    let (label, color) = attention_level_label(item.level);
                    status_pill(ui, label, color);
                    ui.label(format!(
                        "{}: {}",
                        item.workspace.display(),
                        truncate_summary(&item.message, 72)
                    ));
                    ui.separator();
                }
                if items.len() > 4 {
                    ui.weak(format!("{} more", items.len() - 4));
                }
            });
        });
    ui.add_space(6.0);
}

fn render_workspace_panel(ui: &mut egui::Ui, dashboard: &mut MonitorDashboard) {
    ui.heading("Workspaces");
    ui.separator();
    let mut selected = None;
    for (index, workspace) in dashboard.workspaces.iter().enumerate() {
        let label = workspace.root.display().to_string();
        let (status, color) = workspace_status_label(workspace_status(workspace));
        let is_active = index == dashboard.active_workspace;
        egui::Frame::new()
            .fill(if is_active {
                palette::SELECTED_BG
            } else {
                palette::CARD_BG
            })
            .stroke(egui::Stroke::new(
                1.0,
                if is_active {
                    palette::ACCENT
                } else {
                    palette::CARD_BORDER
                },
            ))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                if ui.selectable_label(is_active, label).clicked() {
                    selected = Some(index);
                }
                ui.horizontal(|ui| {
                    status_pill(ui, status, color);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small(format!("score {}", worst_agent_score(&workspace.snapshot)));
                    });
                });
                ui.small(format!(
                    "{} agents · {} events · {} interventions",
                    workspace.snapshot.agent_sessions.len(),
                    workspace.snapshot.event_count,
                    workspace.snapshot.intervention_count
                ));
                if let Some(error) = &workspace.last_error {
                    ui.small(
                        egui::RichText::new(truncate_summary(error, 60)).color(palette::CRITICAL),
                    );
                }
            });
        ui.add_space(6.0);
    }
    if let Some(index) = selected {
        dashboard.active_workspace = index;
        dashboard.selected_row = None;
        dashboard.refresh();
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut dashboard.workspace_input)
                .hint_text("Path to a project folder…")
                .desired_width(f32::INFINITY),
        );
        let submitted =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (ui.button("Add").clicked() || submitted) && !dashboard.workspace_input.trim().is_empty()
        {
            let input = dashboard.workspace_input.trim().to_string();
            dashboard.add_workspace(PathBuf::from(input));
            dashboard.workspace_input.clear();
        }
        if ui.button("Browse…").clicked()
            && let Some(folder) = rfd::FileDialog::new()
                .set_title("Open project workspace")
                .pick_folder()
        {
            dashboard.add_workspace(folder);
            dashboard.workspace_input.clear();
        }
    });
    ui.small(
        egui::RichText::new("Pick a folder to monitor its .agent-monitor logs.")
            .color(palette::NEUTRAL),
    );
}

fn worst_agent_score(snapshot: &DashboardSnapshot) -> i32 {
    snapshot
        .agent_health
        .iter()
        .map(|health| health.score)
        .min()
        .unwrap_or_default()
}

fn build_viewport(background: bool) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("Coding Agent Monitor")
        .with_visible(!background)
        .with_taskbar(!background)
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([820.0, 520.0])
}

fn render_metrics(ui: &mut egui::Ui, snapshot: &DashboardSnapshot) {
    ui.horizontal_wrapped(|ui| {
        status_badge(ui, snapshot.severity);
        for (label, value) in dashboard_metric_items(snapshot) {
            metric(ui, label, value);
        }
    });
}

fn dashboard_metric_items(snapshot: &DashboardSnapshot) -> Vec<(&'static str, usize)> {
    vec![
        ("Events", snapshot.event_count),
        ("Interventions", snapshot.intervention_count),
        ("Design", snapshot.design_count),
        ("Trace", snapshot.trace_count),
        (
            "Replay",
            snapshot.advice_count
                + snapshot.packet_count
                + snapshot.dispatch_count
                + snapshot.outcome_count,
        ),
        ("Locks", snapshot.lock_event_count),
        ("Agents", snapshot.active_agents.len()),
    ]
}

fn status_pill(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.linear_multiply(0.16))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.colored_label(color, label);
        });
}

fn status_badge(ui: &mut egui::Ui, severity: DashboardSeverity) {
    let (label, color) = severity_label(severity);
    egui::Frame::new()
        .fill(color.linear_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Status");
                ui.colored_label(color, label);
            });
        });
}

fn render_agent_panel(
    ui: &mut egui::Ui,
    snapshot: &DashboardSnapshot,
    running_agents: &[RunningAgent],
) {
    ui.heading("Agents");
    ui.separator();
    if snapshot.agent_sessions.is_empty() && running_agents.is_empty() {
        ui.label("No agent activity yet.");
        return;
    }

    if !running_agents.is_empty() {
        ui.strong("Detected processes");
        ui.add_space(4.0);
        for agent in running_agents {
            egui::Frame::new()
                .fill(palette::CARD_BG_SUBTLE)
                .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        status_pill(ui, "Running", palette::HEALTHY);
                        ui.monospace(agent_kind_label(agent.agent));
                    });
                    ui.small(running_agent_summary(agent));
                });
            ui.add_space(6.0);
        }
        if !snapshot.agent_sessions.is_empty() {
            ui.add_space(4.0);
            ui.strong("Logged sessions");
            ui.add_space(4.0);
        }
    }

    for session in &snapshot.agent_sessions {
        let (label, color) = agent_status_label(session.status);
        egui::Frame::new()
            .fill(palette::CARD_BG)
            .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    status_pill(ui, label, color);
                    ui.monospace(&session.agent);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("score {}", session.score));
                    });
                });
                ui.small(format!(
                    "{} events, {} interventions",
                    session.events, session.interventions
                ));
                if let Some(last_seen) = &session.last_seen {
                    ui.small(format!("last seen {last_seen}"));
                }
            });
        ui.add_space(6.0);
    }
}

fn render_advisor_panel(ui: &mut egui::Ui, status: &DashboardAdvisorStatus) {
    ui.heading("Advisor");
    ui.separator();
    let (label, color) = advisor_status_label(status);
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                status_pill(ui, label, color);
                ui.label(advisor_summary_text(status));
            });
            ui.small(&status.message);
            if let Some(profile) = &status.credential_file {
                ui.small(format!("profile {profile}"));
            }
        });
}

fn advisor_status_label(status: &DashboardAdvisorStatus) -> (&'static str, egui::Color32) {
    if !status.enabled {
        return ("Disabled", palette::NEUTRAL);
    }
    match status.severity {
        DashboardSeverity::Healthy => ("Ready", palette::HEALTHY),
        DashboardSeverity::Warning => ("Check", palette::WARNING),
        DashboardSeverity::Critical => ("Blocked", palette::CRITICAL),
    }
}

fn advisor_summary_text(status: &DashboardAdvisorStatus) -> String {
    let source = advisor_credential_source_label(status.credential_source);
    let kind = advisor_credential_kind_label(status.credential_kind);
    let host = status
        .endpoint_host
        .as_deref()
        .filter(|host| !host.trim().is_empty())
        .unwrap_or("endpoint unset");
    let model = if status.model.trim().is_empty() {
        "model unset"
    } else {
        status.model.as_str()
    };
    format!("{source} · {kind} · {host} · {model}")
}

fn advisor_credential_source_label(
    source: coding_agent_monitor::AdvisorCredentialSource,
) -> &'static str {
    match source {
        coding_agent_monitor::AdvisorCredentialSource::Env => "env",
        coding_agent_monitor::AdvisorCredentialSource::CodingPlan => "coding_plan",
        coding_agent_monitor::AdvisorCredentialSource::ClaudePlan => "claude_plan",
    }
}

fn advisor_credential_kind_label(kind: DashboardAdvisorCredentialKind) -> &'static str {
    match kind {
        DashboardAdvisorCredentialKind::None => "none",
        DashboardAdvisorCredentialKind::Env => "env",
        DashboardAdvisorCredentialKind::ApiKey => "api_key",
        DashboardAdvisorCredentialKind::JwtBearer => "jwt_bearer",
        DashboardAdvisorCredentialKind::MissingProfile => "missing_profile",
        DashboardAdvisorCredentialKind::InvalidProfile => "invalid_profile",
        DashboardAdvisorCredentialKind::UnsupportedSource => "unsupported_source",
    }
}

fn render_review_panel(ui: &mut egui::Ui, report: &coding_agent_monitor::AgentReviewReport) {
    ui.heading("Judge");
    ui.separator();
    let (label, color) = match report.status {
        coding_agent_monitor::AgentReviewStatus::Ok => ("OK", palette::HEALTHY),
        coding_agent_monitor::AgentReviewStatus::Watch => ("Watch", palette::WARNING),
        coding_agent_monitor::AgentReviewStatus::Intervene => ("Intervene", palette::CRITICAL),
    };
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                status_pill(ui, label, color);
                ui.label(review_summary_text(report));
            });
            if report.findings.is_empty() {
                ui.small("No control action recommended from current evidence.");
            } else {
                for finding in report.findings.iter().take(4) {
                    ui.add_space(4.0);
                    let (_, severity_color) = severity_label(finding.severity);
                    ui.horizontal(|ui| {
                        ui.colored_label(severity_color, &finding.category);
                        ui.monospace(finding.agent.as_deref().unwrap_or("-"));
                    });
                    ui.small(format!(
                        "{:?}: {}",
                        finding.recommended_action, finding.evidence
                    ));
                }
                if report.findings.len() > 4 {
                    ui.small(format!("{} more findings", report.findings.len() - 4));
                }
            }
        });
}

fn running_agent_summary(agent: &RunningAgent) -> String {
    let cwd = agent
        .cwd
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "cwd unavailable".to_string());
    format!("pid {} · {} · {}", agent.pid, agent.process_name, cwd)
}

fn render_filter_bar(
    ui: &mut egui::Ui,
    display_filter: &mut String,
    filter_error: &mut Option<String>,
    selected_row: &mut Option<usize>,
) {
    ui.horizontal(|ui| {
        ui.label("Display filter");
        let response = ui.text_edit_singleline(display_filter);
        if response.changed() {
            *selected_row = None;
            *filter_error = DashboardFilter::parse(display_filter)
                .err()
                .map(|error| error.to_string());
        }
        if ui.button("Clear").clicked() {
            display_filter.clear();
            *filter_error = None;
            *selected_row = None;
        }
    });
    ui.small("Examples: kind:intervention agent:codex text:memory severity:critical");
}

fn filtered_rows<'a>(
    snapshot: &'a DashboardSnapshot,
    display_filter: &str,
    filter_error: &mut Option<String>,
) -> Vec<&'a DashboardRow> {
    if display_filter.trim().is_empty() {
        *filter_error = None;
        return snapshot.rows.iter().collect();
    }
    match DashboardFilter::parse(display_filter) {
        Ok(filter) => {
            *filter_error = None;
            snapshot.filtered_rows(&filter)
        }
        Err(error) => {
            *filter_error = Some(error.to_string());
            snapshot.rows.iter().collect()
        }
    }
}

fn render_packet_table(
    ui: &mut egui::Ui,
    rows: &[&DashboardRow],
    selected_row: &mut Option<usize>,
) {
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto())
        .column(Column::initial(96.0).at_least(78.0))
        .column(Column::initial(112.0).at_least(88.0))
        .column(Column::initial(130.0).at_least(90.0))
        .column(Column::initial(140.0).at_least(100.0))
        .column(Column::remainder())
        .header(24.0, |mut header| {
            header.col(|ui| {
                ui.strong("No.");
            });
            header.col(|ui| {
                ui.strong("Severity");
            });
            header.col(|ui| {
                ui.strong("Kind");
            });
            header.col(|ui| {
                ui.strong("Agent");
            });
            header.col(|ui| {
                ui.strong("Protocol");
            });
            header.col(|ui| {
                ui.strong("Summary");
            });
        })
        .body(|mut body| {
            for row in rows {
                body.row(26.0, |mut table_row| {
                    let selected = *selected_row == Some(row.number);
                    table_row.col(|ui| {
                        if ui
                            .selectable_label(selected, row.number.to_string())
                            .clicked()
                        {
                            *selected_row = Some(row.number);
                        }
                    });
                    table_row.col(|ui| {
                        let (label, color) = severity_label(row.severity);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.colored_label(color, "●");
                            ui.colored_label(color, label);
                        });
                    });
                    table_row.col(|ui| {
                        ui.label(format!("{:?}", row.kind));
                    });
                    table_row.col(|ui| {
                        ui.monospace(row.agent.as_deref().unwrap_or("-"));
                    });
                    table_row.col(|ui| {
                        ui.monospace(&row.protocol);
                    });
                    table_row.col(|ui| {
                        ui.add(egui::Label::new(truncate_summary(&row.summary, 120)).truncate())
                            .on_hover_text(&row.summary);
                    });
                });
            }
        });
    if rows.is_empty() {
        ui.label("No captured rows match the display filter.");
    }
}

fn render_details(ui: &mut egui::Ui, rows: &[&DashboardRow], selected_row: Option<usize>) {
    ui.heading("Details");
    let row = selected_row.and_then(|number| rows.iter().find(|row| row.number == number));
    if let Some(row) = row {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("No. {}", row.number));
            ui.label(format!("{:?}", row.kind));
            let (label, color) = severity_label(row.severity);
            ui.colored_label(color, label);
            if let Some(agent) = &row.agent {
                ui.monospace(agent);
            }
        });
        if let Some(trail) = requirement_proof_trail(row, 8) {
            render_requirement_proof_trail(ui, &trail);
            ui.add_space(6.0);
        }
        if let Some(detail) = repo_hunk_file_detail(row) {
            render_repo_hunk_file_detail(ui, &detail);
            ui.add_space(6.0);
        }
        if let Some(detail) = dev_history_detail(row) {
            render_dev_history_detail(ui, &detail);
            ui.add_space(6.0);
        }
        if let Some(detail) = probe_run_detail(row) {
            render_probe_run_detail(ui, &detail);
            ui.add_space(6.0);
        }
        if let Some(detail) = decision_trail_detail(row) {
            render_decision_trail_detail(ui, &detail);
            ui.add_space(6.0);
        }
        egui::Frame::new()
            .fill(palette::CARD_BG_SUBTLE)
            .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.monospace(&row.detail);
            });
    } else {
        ui.label("Select a capture row to inspect the normalized event payload.");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequirementProofTrail {
    requirement_id: String,
    text: String,
    steps: Vec<RequirementProofTrailStep>,
    hidden_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequirementProofTrailStep {
    summary: String,
    evidence_summary: String,
    gap_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoHunkFileDetail {
    path: String,
    total: u64,
    traced: u64,
    missing_rationale: u64,
    untraced: u64,
    matching_traces: u64,
    worst_status: String,
    latest_status: String,
    latest_history_id: String,
    latest_observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DevHistoryDetail {
    kind: String,
    severity: String,
    generated_at: String,
    workspace: String,
    summary: String,
    source_summary: String,
    evidence_summary: String,
    monitor_response_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeRunDetail {
    probe_run_id: String,
    advice_id: String,
    probe_kind: String,
    target: String,
    status: String,
    evidence_count: usize,
    summary: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecisionTrailDetail {
    advice_id: String,
    action: String,
    target_agent: String,
    packet_id: String,
    urgency: String,
    dispatch_status: String,
    outcome_count: usize,
    failed_outcome_count: usize,
    rationale: String,
}

fn requirement_proof_trail(row: &DashboardRow, max_steps: usize) -> Option<RequirementProofTrail> {
    if row.kind != DashboardRowKind::Requirement || max_steps == 0 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&row.detail).ok()?;
    let requirement_id = json_string(&value, "/requirement/requirement_id")?;
    let text = json_string(&value, "/requirement/text").unwrap_or_default();
    let proofs = value.get("proofs")?.as_array()?;
    if proofs.is_empty() {
        return None;
    }
    let steps = proofs
        .iter()
        .take(max_steps)
        .map(requirement_proof_trail_step)
        .collect::<Vec<_>>();
    Some(RequirementProofTrail {
        requirement_id,
        text,
        hidden_count: proofs.len().saturating_sub(steps.len()),
        steps,
    })
}

fn requirement_proof_trail_step(value: &serde_json::Value) -> RequirementProofTrailStep {
    let case_file = json_string(value, "/case_file_id").unwrap_or_else(|| "case unknown".into());
    let status = json_string(value, "/status").unwrap_or_else(|| "status unknown".into());
    let score = value
        .pointer("/proof_strength/score")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let latest_status = json_string(value, "/latest_status");
    let built_at = json_string(value, "/built_at");
    let latest_verifier = json_string(value, "/latest_verification_evidence_id");
    let mut summary = format!("{case_file} · {status} · proof {score}");
    if let Some(latest_status) = latest_status {
        summary.push_str(&format!(" · verifier {latest_status}"));
    }
    if let Some(built_at) = built_at {
        summary.push_str(&format!(" · {built_at}"));
    }
    if let Some(latest_verifier) = latest_verifier {
        summary.push_str(&format!(" · evidence {latest_verifier}"));
    }

    let evidence_summary = format!(
        "{} trace · {} repo hunk · {} control · {} outcome",
        json_array_len(value, "trace_refs"),
        json_array_len(value, "repo_hunks"),
        json_array_len(value, "control_refs"),
        json_array_len(value, "outcome_refs")
    );
    let gaps = json_string_array(value, "/proof_strength/gaps");
    let gap_summary = if gaps.is_empty() {
        "no proof gaps".into()
    } else {
        format!(
            "gaps: {}",
            gaps.into_iter().take(4).collect::<Vec<_>>().join(", ")
        )
    };

    RequirementProofTrailStep {
        summary,
        evidence_summary,
        gap_summary,
    }
}

fn render_requirement_proof_trail(ui: &mut egui::Ui, trail: &RequirementProofTrail) {
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Requirement Proof Trail");
                ui.monospace(&trail.requirement_id);
                if trail.hidden_count > 0 {
                    ui.weak(format!("{} older hidden", trail.hidden_count));
                }
            });
            if !trail.text.trim().is_empty() {
                ui.small(truncate_summary(&trail.text, 140));
            }
            ui.add_space(4.0);
            for step in &trail.steps {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&step.summary);
                });
                ui.small(&step.evidence_summary);
                ui.small(&step.gap_summary);
                ui.add_space(4.0);
            }
        });
}

fn repo_hunk_file_detail(row: &DashboardRow) -> Option<RepoHunkFileDetail> {
    if row.kind != DashboardRowKind::RepoHunkFile {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&row.detail).ok()?;
    Some(RepoHunkFileDetail {
        path: json_string(&value, "/path")?,
        total: json_u64(&value, "/entry_count"),
        traced: json_u64(&value, "/traced_count"),
        missing_rationale: json_u64(&value, "/missing_rationale_count"),
        untraced: json_u64(&value, "/untraced_count"),
        matching_traces: json_u64(&value, "/matching_trace_count"),
        worst_status: json_string(&value, "/worst_trace_status")
            .unwrap_or_else(|| "unknown".into()),
        latest_status: json_string(&value, "/latest_trace_status")
            .unwrap_or_else(|| "unknown".into()),
        latest_history_id: json_string(&value, "/latest_history_id")
            .unwrap_or_else(|| "unknown".into()),
        latest_observed_at: json_string(&value, "/latest_observed_at")
            .unwrap_or_else(|| "unknown".into()),
    })
}

fn render_repo_hunk_file_detail(ui: &mut egui::Ui, detail: &RepoHunkFileDetail) {
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Repo Hunk File");
                ui.monospace(&detail.path);
                ui.weak(format!("latest {}", detail.latest_observed_at));
            });
            ui.small(format!(
                "{} hunk(s) · {} traced · {} missing rationale · {} untraced",
                detail.total, detail.traced, detail.missing_rationale, detail.untraced
            ));
            ui.small(format!(
                "worst {} · latest {} · {} matching trace(s)",
                detail.worst_status, detail.latest_status, detail.matching_traces
            ));
            ui.small(format!("latest history {}", detail.latest_history_id));
        });
}

fn dev_history_detail(row: &DashboardRow) -> Option<DevHistoryDetail> {
    if row.kind != DashboardRowKind::DevHistory {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&row.detail).ok()?;
    Some(DevHistoryDetail {
        kind: json_string(&value, "/finding/kind")?,
        severity: json_string(&value, "/finding/severity").unwrap_or_else(|| "info".into()),
        generated_at: json_string(&value, "/generated_at").unwrap_or_else(|| "unknown".into()),
        workspace: json_string(&value, "/workspace").unwrap_or_else(|| "workspace unknown".into()),
        summary: json_string(&value, "/finding/summary").unwrap_or_default(),
        source_summary: dev_history_sources_summary(&value),
        evidence_summary: compact_json_string_list(
            &json_string_array(&value, "/finding/evidence"),
            "no aggregate evidence details",
        ),
        monitor_response_summary: compact_json_string_list(
            &json_string_array(&value, "/finding/monitor_response"),
            "no monitor response recorded",
        ),
    })
}

fn dev_history_sources_summary(value: &serde_json::Value) -> String {
    let sources = value
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .filter_map(|source| {
                    let name = json_string(source, "/source")?;
                    let files = json_u64(source, "/files");
                    let sessions = json_u64(source, "/sessions");
                    Some(format!("{name} {files} file(s)/{sessions} session(s)"))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    compact_json_string_list(&sources, "no sources")
}

fn compact_json_string_list(values: &[String], empty: &str) -> String {
    if values.is_empty() {
        return empty.into();
    }
    let shown = values.iter().take(4).cloned().collect::<Vec<_>>();
    let mut summary = shown.join("; ");
    if values.len() > shown.len() {
        summary.push_str(&format!("; {} more", values.len() - shown.len()));
    }
    summary
}

fn render_dev_history_detail(ui: &mut egui::Ui, detail: &DevHistoryDetail) {
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Dev History Finding");
                ui.monospace(&detail.kind);
                ui.weak(&detail.severity);
                ui.weak(format!("generated {}", detail.generated_at));
            });
            ui.small(truncate_summary(&detail.workspace, 140));
            if !detail.summary.trim().is_empty() {
                ui.small(truncate_summary(&detail.summary, 180));
            }
            ui.small(format!("sources: {}", detail.source_summary));
            ui.small(format!("evidence: {}", detail.evidence_summary));
            ui.small(format!(
                "monitor response: {}",
                detail.monitor_response_summary
            ));
        });
}

fn probe_run_detail(row: &DashboardRow) -> Option<ProbeRunDetail> {
    if row.kind != DashboardRowKind::ProbeRun {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&row.detail).ok()?;
    let probe_kind = json_string(&value, "/probe/kind")?;
    let target = json_string(&value, "/probe/target")
        .or_else(|| json_string(&value, "/probe/command"))
        .unwrap_or_else(|| "no target".into());
    Some(ProbeRunDetail {
        probe_run_id: json_string(&value, "/probe_run_id")?,
        advice_id: json_string(&value, "/advice_id")?,
        probe_kind,
        target,
        status: json_string(&value, "/status").unwrap_or_else(|| "unknown".into()),
        evidence_count: value
            .get("evidence_ids")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        summary: json_string(&value, "/summary").unwrap_or_default(),
        note: json_string(&value, "/note"),
    })
}

fn render_probe_run_detail(ui: &mut egui::Ui, detail: &ProbeRunDetail) {
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Probe Run");
                ui.monospace(&detail.probe_run_id);
                ui.weak(&detail.status);
            });
            ui.small(format!(
                "{} · target {} · advice {}",
                detail.probe_kind, detail.target, detail.advice_id
            ));
            ui.small(format!("{} evidence ref(s)", detail.evidence_count));
            if !detail.summary.trim().is_empty() {
                ui.small(truncate_summary(&detail.summary, 180));
            }
            if let Some(note) = &detail.note {
                ui.small(truncate_summary(note, 180));
            }
        });
}

fn decision_trail_detail(row: &DashboardRow) -> Option<DecisionTrailDetail> {
    if row.kind != DashboardRowKind::DecisionTrail {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&row.detail).ok()?;
    let outcomes = value
        .get("outcomes")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let failed_outcome_count = outcomes
        .iter()
        .filter(|outcome| json_string(outcome, "/status").as_deref() == Some("failed"))
        .count();
    Some(DecisionTrailDetail {
        advice_id: json_string(&value, "/advice/advice_id")?,
        action: json_string(&value, "/advice/control_rationale/selected_action")
            .or_else(|| json_string(&value, "/advice/final_action/type"))
            .unwrap_or_else(|| "unknown_action".into()),
        target_agent: json_string(&value, "/packet/target_agent")
            .or_else(|| json_string(&value, "/dispatch_result/target_agent"))
            .unwrap_or_else(|| "unknown_agent".into()),
        packet_id: json_string(&value, "/packet/packet_id")
            .or_else(|| json_string(&value, "/dispatch_result/packet_id"))
            .unwrap_or_else(|| "unknown_packet".into()),
        urgency: json_string(&value, "/packet/urgency").unwrap_or_else(|| "unknown".into()),
        dispatch_status: json_string(&value, "/dispatch_result/status")
            .unwrap_or_else(|| "unknown".into()),
        outcome_count: outcomes.len(),
        failed_outcome_count,
        rationale: json_string(&value, "/advice/control_rationale/reason").unwrap_or_default(),
    })
}

fn render_decision_trail_detail(ui: &mut egui::Ui, detail: &DecisionTrailDetail) {
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Decision Trail");
                ui.monospace(&detail.advice_id);
                ui.weak(&detail.action);
            });
            ui.small(format!(
                "target {} · packet {} · {} · dispatch {}",
                detail.target_agent, detail.packet_id, detail.urgency, detail.dispatch_status
            ));
            ui.small(format!(
                "{} outcome(s), {} failed",
                detail.outcome_count, detail.failed_outcome_count
            ));
            if !detail.rationale.trim().is_empty() {
                ui.small(truncate_summary(&detail.rationale, 180));
            }
        });
}

fn json_string(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn json_u64(value: &serde_json::Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
}

fn json_array_len(value: &serde_json::Value, field: &str) -> usize {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

fn json_string_array(value: &serde_json::Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn severity_label(severity: DashboardSeverity) -> (&'static str, egui::Color32) {
    match severity {
        DashboardSeverity::Healthy => ("Healthy", palette::HEALTHY),
        DashboardSeverity::Warning => ("Warning", palette::WARNING),
        DashboardSeverity::Critical => ("Critical", palette::CRITICAL),
    }
}

fn agent_status_label(status: AgentActivityStatus) -> (&'static str, egui::Color32) {
    match status {
        AgentActivityStatus::Active => ("Active", palette::HEALTHY),
        AgentActivityStatus::Stale => ("Stale", palette::WARNING),
        AgentActivityStatus::Degraded => ("Degraded", palette::CRITICAL),
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: usize) {
    egui::Frame::new()
        .fill(palette::CARD_BG)
        .stroke(egui::Stroke::new(1.0, palette::CARD_BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(96.0);
            ui.vertical_centered(|ui| {
                ui.heading(egui::RichText::new(value.to_string()).color(palette::ACCENT));
                ui.small(egui::RichText::new(label).color(palette::NEUTRAL));
            });
        });
}

fn empty_snapshot() -> DashboardSnapshot {
    DashboardSnapshot {
        severity: DashboardSeverity::Healthy,
        advisor_status: DashboardAdvisorStatus::default(),
        event_count: 0,
        intervention_count: 0,
        design_count: 0,
        trace_count: 0,
        verifier_run_count: 0,
        probe_run_count: 0,
        repo_hunk_history_count: 0,
        repo_hunk_file_count: 0,
        requirement_count: 0,
        dev_history_count: 0,
        decision_trail_count: 0,
        advice_count: 0,
        packet_count: 0,
        dispatch_count: 0,
        outcome_count: 0,
        lock_event_count: 0,
        active_agents: Vec::new(),
        agent_health: Vec::new(),
        agent_sessions: Vec::new(),
        rows: Vec::new(),
        recent_events: Vec::new(),
        recent_interventions: Vec::new(),
        recent_verifier_runs: Vec::new(),
        recent_probe_runs: Vec::new(),
        recent_repo_hunks: Vec::new(),
        recent_repo_hunk_files: Vec::new(),
        recent_requirements: Vec::new(),
        recent_requirement_proofs: Vec::new(),
        recent_dev_history: Vec::new(),
        recent_decision_trails: Vec::new(),
        recent_worktree_lock_events: Vec::new(),
    }
}

fn parse_ui_options(args: impl IntoIterator<Item = String>) -> UiOptions {
    let mut workspaces = Vec::new();
    let mut options = UiOptions {
        workspaces: Vec::new(),
        background: false,
    };

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspaces.push(PathBuf::from(value));
        } else if arg == "--background" {
            options.background = true;
        }
    }

    options.workspaces = normalize_workspaces(workspaces);
    options
}

fn normalize_workspaces(workspaces: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut normalized = Vec::new();
    for workspace in workspaces {
        if !normalized.contains(&workspace) {
            normalized.push(workspace);
        }
    }
    if normalized.is_empty() {
        normalized.push(PathBuf::from("."));
    }
    normalized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayCommand {
    Show,
    Hide,
    Toggle,
    Quit,
}

struct TrayHandle {
    _icon: TrayIcon,
}

fn tray_command_from_id(id: &MenuId) -> Option<TrayCommand> {
    match id.as_ref() {
        TRAY_SHOW_ID => Some(TrayCommand::Show),
        TRAY_HIDE_ID => Some(TrayCommand::Hide),
        TRAY_TOGGLE_ID => Some(TrayCommand::Toggle),
        TRAY_QUIT_ID => Some(TrayCommand::Quit),
        _ => None,
    }
}

fn handle_tray_events(ctx: &egui::Context, dashboard: &mut MonitorDashboard) {
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        match tray_command_from_id(event.id()) {
            Some(TrayCommand::Show) => dashboard.set_window_visible(ctx, true),
            Some(TrayCommand::Hide) => dashboard.set_window_visible(ctx, false),
            Some(TrayCommand::Toggle) => {
                let visible = !dashboard.window_visible;
                dashboard.set_window_visible(ctx, visible);
            }
            Some(TrayCommand::Quit) => {
                dashboard.quit_requested = true;
            }
            None => {}
        }
    }
}

fn build_tray_icon() -> Option<TrayHandle> {
    let menu = Menu::new();
    let title = MenuItem::new("Coding Agent Monitor", false, None);
    let toggle = MenuItem::with_id(TRAY_TOGGLE_ID, "Toggle Dashboard", true, None);
    let show = MenuItem::with_id(TRAY_SHOW_ID, "Show Dashboard", true, None);
    let hide = MenuItem::with_id(TRAY_HIDE_ID, "Hide Dashboard", true, None);
    let quit = MenuItem::with_id(TRAY_QUIT_ID, "Quit Monitor", true, None);
    let separator = PredefinedMenuItem::separator();
    menu.append_items(&[&title, &separator, &toggle, &show, &hide, &quit])
        .ok()?;

    let icon = monitor_icon()?;
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_menu_on_right_click(true)
        .with_tooltip("Coding Agent Monitor")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(TrayHandle { _icon: icon })
}

fn monitor_icon() -> Option<Icon> {
    let size = 16;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let inside = (3..=12).contains(&x) && (3..=12).contains(&y);
            let accent = (x == y || x + y == 15) && inside;
            let pixel = if accent {
                [245, 248, 250, 255]
            } else if inside {
                [32, 120, 210, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    Icon::from_rgba(rgba, size as u32, size as u32).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workspace_argument() {
        assert_eq!(
            parse_ui_options(["--workspace=F:/repo".to_string()]).workspaces,
            vec![PathBuf::from("F:/repo")]
        );
    }

    #[test]
    fn parses_multiple_workspace_arguments() {
        assert_eq!(
            parse_ui_options([
                "--workspace=F:/repo-a".to_string(),
                "--workspace=F:/repo-b".to_string(),
            ])
            .workspaces,
            vec![PathBuf::from("F:/repo-a"), PathBuf::from("F:/repo-b")]
        );
    }

    #[test]
    fn workspace_arguments_are_deduplicated() {
        assert_eq!(
            parse_ui_options([
                "--workspace=F:/repo-a".to_string(),
                "--workspace=F:/repo-a".to_string(),
            ])
            .workspaces,
            vec![PathBuf::from("F:/repo-a")]
        );
    }

    #[test]
    fn ui_options_default_to_foreground_current_workspace() {
        assert_eq!(
            parse_ui_options(["--other=value".to_string()]),
            UiOptions {
                workspaces: vec![PathBuf::from(".")],
                background: false,
            }
        );
    }

    #[test]
    fn parses_background_flag() {
        assert!(parse_ui_options(["--background".to_string()]).background);
    }

    #[test]
    fn tray_menu_ids_map_to_commands() {
        assert_eq!(
            tray_command_from_id(&MenuId::new(TRAY_SHOW_ID)),
            Some(TrayCommand::Show)
        );
        assert_eq!(
            tray_command_from_id(&MenuId::new(TRAY_HIDE_ID)),
            Some(TrayCommand::Hide)
        );
        assert_eq!(
            tray_command_from_id(&MenuId::new(TRAY_QUIT_ID)),
            Some(TrayCommand::Quit)
        );
    }

    #[test]
    fn background_viewport_starts_hidden_and_off_taskbar() {
        let viewport = build_viewport(true);

        assert_eq!(viewport.visible, Some(false));
        assert_eq!(viewport.taskbar, Some(false));
    }

    #[test]
    fn foreground_viewport_starts_visible_and_on_taskbar() {
        let viewport = build_viewport(false);

        assert_eq!(viewport.visible, Some(true));
        assert_eq!(viewport.taskbar, Some(true));
    }

    #[test]
    fn light_theme_configuration_forces_active_light_theme() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);

        configure_light_theme(&ctx);

        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(
            ctx.global_style().visuals.panel_fill,
            egui::Color32::from_rgb(246, 248, 251)
        );
    }

    #[test]
    fn formats_unix_epoch_as_utc_timestamp() {
        assert_eq!(format_utc_seconds(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn formats_known_utc_timestamp() {
        assert_eq!(format_utc_seconds(1_782_130_800), "2026-06-22T12:20:00Z");
    }

    #[test]
    fn workspace_status_is_empty_without_activity() {
        let workspace = WorkspaceState::new(PathBuf::from("F:/repo"));

        assert_eq!(workspace_status(&workspace), WorkspaceStatus::Empty);
    }

    #[test]
    fn workspace_status_follows_snapshot_severity() {
        let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
        workspace.snapshot.event_count = 3;
        workspace.snapshot.severity = DashboardSeverity::Critical;

        assert_eq!(workspace_status(&workspace), WorkspaceStatus::Critical);
    }

    #[test]
    fn workspace_status_reports_warning_for_stale_agents() {
        let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
        workspace.snapshot.event_count = 3;
        workspace
            .snapshot
            .agent_sessions
            .push(coding_agent_monitor::AgentSession {
                agent: "codex".into(),
                status: AgentActivityStatus::Stale,
                score: 0,
                events: 3,
                interventions: 0,
                last_seen: Some("2026-06-22T12:00:00Z".into()),
            });

        assert_eq!(workspace_status(&workspace), WorkspaceStatus::Warning);
    }

    #[test]
    fn fleet_status_counts_workspace_states() {
        let empty = WorkspaceState::new(PathBuf::from("F:/empty"));
        let mut healthy = WorkspaceState::new(PathBuf::from("F:/healthy"));
        healthy.snapshot.event_count = 1;
        let mut critical = WorkspaceState::new(PathBuf::from("F:/critical"));
        critical.snapshot.event_count = 1;
        critical.snapshot.severity = DashboardSeverity::Critical;

        let status = fleet_status(&[empty, healthy, critical]);

        assert_eq!(
            status,
            FleetStatus {
                total: 3,
                empty: 1,
                healthy: 1,
                warning: 0,
                critical: 1,
            }
        );
    }

    #[test]
    fn fleet_status_label_prioritizes_critical_over_warning() {
        let (label, _) = fleet_status_label(FleetStatus {
            total: 2,
            empty: 0,
            healthy: 0,
            warning: 1,
            critical: 1,
        });

        assert_eq!(label, "Critical");
    }

    #[test]
    fn fleet_summary_reports_no_workspaces_when_empty() {
        assert_eq!(
            fleet_summary_text(FleetStatus::default()),
            "no workspaces configured"
        );
    }

    #[test]
    fn fleet_summary_omits_zero_categories_and_orders_by_severity() {
        let status = FleetStatus {
            total: 4,
            empty: 1,
            healthy: 1,
            warning: 0,
            critical: 2,
        };

        assert_eq!(
            fleet_summary_text(status),
            "4 workspaces · 2 critical, 1 healthy, 1 empty"
        );
    }

    #[test]
    fn fleet_summary_uses_singular_workspace_noun() {
        let status = FleetStatus {
            total: 1,
            empty: 0,
            healthy: 1,
            warning: 0,
            critical: 0,
        };

        assert_eq!(fleet_summary_text(status), "1 workspace · 1 healthy");
    }

    #[test]
    fn relative_age_uses_just_now_for_fresh_refresh() {
        assert_eq!(format_relative_age(Duration::from_millis(400)), "just now");
    }

    #[test]
    fn relative_age_scales_units() {
        assert_eq!(format_relative_age(Duration::from_secs(5)), "5s ago");
        assert_eq!(format_relative_age(Duration::from_secs(150)), "2m ago");
        assert_eq!(format_relative_age(Duration::from_secs(7_400)), "2h ago");
    }

    #[test]
    fn capture_summary_counts_kinds_and_severities() {
        let rows = [
            DashboardRow {
                number: 1,
                kind: DashboardRowKind::Event,
                severity: DashboardSeverity::Healthy,
                agent: Some("codex".into()),
                protocol: "ModelMessage".into(),
                summary: "ok".into(),
                detail: String::new(),
            },
            DashboardRow {
                number: 2,
                kind: DashboardRowKind::Intervention,
                severity: DashboardSeverity::Warning,
                agent: Some("codex".into()),
                protocol: "ServiceFailure".into(),
                summary: "retry".into(),
                detail: String::new(),
            },
            DashboardRow {
                number: 3,
                kind: DashboardRowKind::RepoHunkFile,
                severity: DashboardSeverity::Warning,
                agent: None,
                protocol: "repo-hunk-file".into(),
                summary: "src/lib.rs".into(),
                detail: String::new(),
            },
            DashboardRow {
                number: 4,
                kind: DashboardRowKind::Intervention,
                severity: DashboardSeverity::Critical,
                agent: Some("pi".into()),
                protocol: "AgentDegraded".into(),
                summary: "spawn".into(),
                detail: String::new(),
            },
        ];
        let refs = rows.iter().collect::<Vec<_>>();

        assert_eq!(
            capture_summary(&refs),
            CaptureSummary {
                total: 4,
                events: 2,
                interventions: 2,
                warning: 2,
                critical: 1,
            }
        );
    }

    #[test]
    fn capture_summary_is_empty_for_no_rows() {
        assert_eq!(capture_summary(&[]), CaptureSummary::default());
        assert_eq!(
            capture_summary_text(CaptureSummary::default()),
            "0 rows · 0 events · 0 interventions · 0 warning · 0 critical"
        );
    }

    #[test]
    fn dashboard_metric_items_show_replay_side_logs_and_locks_separately() {
        let mut snapshot = empty_snapshot();
        snapshot.event_count = 3;
        snapshot.intervention_count = 1;
        snapshot.design_count = 2;
        snapshot.trace_count = 4;
        snapshot.advice_count = 5;
        snapshot.packet_count = 6;
        snapshot.dispatch_count = 7;
        snapshot.outcome_count = 8;
        snapshot.lock_event_count = 9;

        let metrics = dashboard_metric_items(&snapshot);

        assert_eq!(
            metrics,
            vec![
                ("Events", 3),
                ("Interventions", 1),
                ("Design", 2),
                ("Trace", 4),
                ("Replay", 26),
                ("Locks", 9),
                ("Agents", 0),
            ]
        );
    }

    #[test]
    fn truncate_summary_keeps_short_text_intact() {
        assert_eq!(truncate_summary("  hello  ", 20), "hello");
    }

    #[test]
    fn truncate_summary_adds_ellipsis_when_too_long() {
        assert_eq!(truncate_summary("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn truncate_summary_counts_characters_not_bytes() {
        assert_eq!(truncate_summary("ßßßßß", 3), "ßß…");
    }

    #[test]
    fn attention_items_include_workspace_errors() {
        let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
        workspace.last_error = Some("decode failed".into());

        let items = attention_items(&[workspace]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].level, AttentionLevel::Critical);
        assert_eq!(items[0].message, "decode failed");
    }

    #[test]
    fn attention_items_include_stale_and_degraded_agents() {
        let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
        workspace
            .snapshot
            .agent_sessions
            .push(coding_agent_monitor::AgentSession {
                agent: "codex".into(),
                status: AgentActivityStatus::Stale,
                score: 0,
                events: 4,
                interventions: 0,
                last_seen: Some("2026-06-22T12:00:00Z".into()),
            });
        workspace
            .snapshot
            .agent_sessions
            .push(coding_agent_monitor::AgentSession {
                agent: "claude-code".into(),
                status: AgentActivityStatus::Degraded,
                score: -3,
                events: 2,
                interventions: 1,
                last_seen: Some("2026-06-22T12:02:00Z".into()),
            });

        let items = attention_items(&[workspace]);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].level, AttentionLevel::Critical);
        assert!(items[0].message.contains("claude-code"));
        assert_eq!(items[1].level, AttentionLevel::Warning);
        assert!(items[1].message.contains("codex"));
    }

    #[test]
    fn attention_items_include_critical_advisor_status() {
        let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
        workspace.snapshot.advisor_status = DashboardAdvisorStatus {
            enabled: true,
            credential_source: coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
            credential_kind: DashboardAdvisorCredentialKind::JwtBearer,
            uses_dedicated_profile: true,
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            endpoint_host: Some("api.openai.com".into()),
            model: "gpt-5.5".into(),
            credential_file: Some("credentials/coding-plan/auth.json".into()),
            severity: DashboardSeverity::Critical,
            message: "JWT/OAuth-style coding-plan credential is incompatible with api.openai.com"
                .into(),
        };

        let items = attention_items(&[workspace]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].level, AttentionLevel::Critical);
        assert!(items[0].message.contains("advisor"));
        assert!(items[0].message.contains("api.openai.com"));
    }

    #[test]
    fn advisor_summary_names_source_without_exposing_token_material() {
        let status = DashboardAdvisorStatus {
            enabled: true,
            credential_source: coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
            credential_kind: DashboardAdvisorCredentialKind::JwtBearer,
            uses_dedicated_profile: true,
            endpoint: "https://coding-plan.example.test/v1/chat/completions".into(),
            endpoint_host: Some("coding-plan.example.test".into()),
            model: "coding-plan-advisor".into(),
            credential_file: Some("credentials/coding-plan/auth.json".into()),
            severity: DashboardSeverity::Healthy,
            message: "dedicated coding-plan advisor endpoint configured".into(),
        };

        let text = advisor_summary_text(&status);

        assert!(text.contains("coding_plan"));
        assert!(text.contains("jwt_bearer"));
        assert!(text.contains("coding-plan.example.test"));
        assert!(!text.contains("eyJ"));
        assert!(!text.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn requirement_proof_trail_summarizes_bounded_history() {
        let row = DashboardRow {
            number: 7,
            kind: DashboardRowKind::Requirement,
            severity: DashboardSeverity::Warning,
            agent: None,
            protocol: "requirement".into(),
            summary: "covered proof 85: Advisor decisions must cite evidence.".into(),
            detail: r#"{
              "requirement": {
                "requirement_id": "req-advisor-proof",
                "text": "Advisor decisions must cite evidence."
              },
              "proofs": [
                {
                  "case_file_id": "case-new",
                  "built_at": "2026-06-24T09:00:00Z",
                  "status": "covered",
                  "latest_status": "passed",
                  "latest_verification_evidence_id": "evt-new-verifier",
                  "proof_strength": {
                    "score": 85,
                    "signals": ["trace_refs", "outcome_refs"],
                    "gaps": []
                  },
                  "trace_refs": [{"file": "src/lib.rs", "necessity": "necessary"}],
                  "repo_hunks": [{"history_id": "hunk-1"}],
                  "control_refs": [{"advice_id": "adv-1"}],
                  "outcome_refs": [{"outcome_id": "out-1"}]
                },
                {
                  "case_file_id": "case-old",
                  "built_at": "2026-06-23T09:00:00Z",
                  "status": "stale",
                  "latest_status": "stale",
                  "proof_strength": {
                    "score": 35,
                    "signals": ["verification_ref"],
                    "gaps": ["no_trace_refs", "no_outcome_refs"]
                  },
                  "trace_refs": [],
                  "repo_hunks": [],
                  "control_refs": [],
                  "outcome_refs": []
                },
                {
                  "case_file_id": "case-older",
                  "built_at": "2026-06-22T09:00:00Z",
                  "status": "unverified",
                  "proof_strength": { "score": 10, "signals": [], "gaps": ["no_verifier"] }
                }
              ]
            }"#
            .into(),
        };

        let trail = requirement_proof_trail(&row, 2).expect("proof trail");

        assert_eq!(trail.requirement_id, "req-advisor-proof");
        assert_eq!(trail.text, "Advisor decisions must cite evidence.");
        assert_eq!(trail.steps.len(), 2);
        assert_eq!(trail.hidden_count, 1);
        assert!(trail.steps[0].summary.contains("case-new"));
        assert!(trail.steps[0].summary.contains("covered"));
        assert!(trail.steps[0].summary.contains("proof 85"));
        assert!(trail.steps[0].summary.contains("passed"));
        assert!(trail.steps[0].evidence_summary.contains("1 trace"));
        assert!(trail.steps[0].evidence_summary.contains("1 outcome"));
        assert!(trail.steps[1].gap_summary.contains("no_trace_refs"));
        assert!(trail.steps[1].gap_summary.contains("no_outcome_refs"));
    }

    #[test]
    fn requirement_proof_trail_ignores_non_requirement_rows() {
        let row = DashboardRow {
            number: 1,
            kind: DashboardRowKind::Event,
            severity: DashboardSeverity::Healthy,
            agent: Some("codex".into()),
            protocol: "model".into(),
            summary: "not a requirement".into(),
            detail: r#"{"proofs":[{"case_file_id":"case"}]}"#.into(),
        };

        assert_eq!(requirement_proof_trail(&row, 4), None);
    }

    #[test]
    fn requirement_proof_trail_ignores_invalid_requirement_detail_json() {
        let row = DashboardRow {
            number: 1,
            kind: DashboardRowKind::Requirement,
            severity: DashboardSeverity::Warning,
            agent: None,
            protocol: "requirement".into(),
            summary: "broken detail".into(),
            detail: "{not-json".into(),
        };

        assert_eq!(requirement_proof_trail(&row, 4), None);
    }

    #[test]
    fn repo_hunk_file_detail_summarizes_counts() {
        let row = DashboardRow {
            number: 8,
            kind: DashboardRowKind::RepoHunkFile,
            severity: DashboardSeverity::Warning,
            agent: None,
            protocol: "repo-hunk-file".into(),
            summary: "src/lib.rs".into(),
            detail: r#"{
              "path": "src/lib.rs",
              "entry_count": 3,
              "traced_count": 1,
              "missing_rationale_count": 1,
              "untraced_count": 1,
              "matching_trace_count": 2,
              "worst_trace_status": "untraced",
              "latest_trace_status": "missing_rationale",
              "latest_history_id": "hist-new",
              "latest_observed_at": "2026-06-24T09:00:00Z"
            }"#
            .into(),
        };

        let detail = repo_hunk_file_detail(&row).expect("repo hunk file detail");

        assert_eq!(detail.path, "src/lib.rs");
        assert_eq!(detail.total, 3);
        assert_eq!(detail.traced, 1);
        assert_eq!(detail.missing_rationale, 1);
        assert_eq!(detail.untraced, 1);
        assert_eq!(detail.matching_traces, 2);
        assert_eq!(detail.worst_status, "untraced");
        assert_eq!(detail.latest_status, "missing_rationale");
        assert_eq!(detail.latest_history_id, "hist-new");
        assert_eq!(detail.latest_observed_at, "2026-06-24T09:00:00Z");
    }

    #[test]
    fn repo_hunk_file_detail_ignores_non_file_rows() {
        let row = DashboardRow {
            number: 1,
            kind: DashboardRowKind::RepoHunk,
            severity: DashboardSeverity::Warning,
            agent: None,
            protocol: "repo-hunk".into(),
            summary: "raw hunk".into(),
            detail: r#"{"path":"src/lib.rs","entry_count":3}"#.into(),
        };

        assert_eq!(repo_hunk_file_detail(&row), None);
    }

    #[test]
    fn dev_history_detail_summarizes_sources_and_finding() {
        let row = DashboardRow {
            number: 9,
            kind: DashboardRowKind::DevHistory,
            severity: DashboardSeverity::Critical,
            agent: None,
            protocol: "dev-history".into(),
            summary: "critical verification_entropy: stale verification risk".into(),
            detail: r#"{
              "generated_at": "2026-06-24T02:17:50Z",
              "workspace": "F:/rag_sys",
              "sources": [
                {
                  "source": "codex",
                  "files": 36,
                  "sessions": 36,
                  "lines": 21319,
                  "bytes": 82563658,
                  "history_root": "C:/Users/yys/.codex/sessions"
                },
                {
                  "source": "claude-code",
                  "files": 432,
                  "sessions": 4,
                  "lines": 77842,
                  "bytes": 369301667,
                  "history_root": "C:/Users/yys/.claude/projects/F--rag-sys"
                }
              ],
              "finding": {
                "kind": "verification_entropy",
                "severity": "critical",
                "summary": "History shows stale verification risk.",
                "evidence": ["14136 verification or unverified-stop signals"],
                "monitor_response": ["Force verification before continue."]
              }
            }"#
            .into(),
        };

        let detail = dev_history_detail(&row).expect("dev-history detail");

        assert_eq!(detail.kind, "verification_entropy");
        assert_eq!(detail.severity, "critical");
        assert_eq!(detail.generated_at, "2026-06-24T02:17:50Z");
        assert_eq!(detail.workspace, "F:/rag_sys");
        assert_eq!(
            detail.source_summary,
            "codex 36 file(s)/36 session(s); claude-code 432 file(s)/4 session(s)"
        );
        assert_eq!(
            detail.evidence_summary,
            "14136 verification or unverified-stop signals"
        );
        assert_eq!(
            detail.monitor_response_summary,
            "Force verification before continue."
        );
    }

    #[test]
    fn dev_history_detail_ignores_non_dev_history_rows() {
        let row = DashboardRow {
            number: 1,
            kind: DashboardRowKind::Event,
            severity: DashboardSeverity::Healthy,
            agent: Some("codex".into()),
            protocol: "model".into(),
            summary: "not dev history".into(),
            detail: r#"{"finding":{"kind":"verification_entropy"}}"#.into(),
        };

        assert_eq!(dev_history_detail(&row), None);
    }

    #[test]
    fn probe_run_detail_summarizes_probe_payload() {
        let row = DashboardRow {
            number: 10,
            kind: DashboardRowKind::ProbeRun,
            severity: DashboardSeverity::Healthy,
            agent: None,
            protocol: "probe".into(),
            summary: "local_evidence: succeeded".into(),
            detail: r#"{
              "probe_run_id": "probe-run-local",
              "advice_id": "advice-probe",
              "probe": {
                "kind": "local_evidence",
                "target": "routine_next_step"
              },
              "status": "succeeded",
              "summary": "local evidence probe observed recent events",
              "evidence_ids": ["evt-user", "repo-audit-src-lib-rs"]
            }"#
            .into(),
        };

        let detail = probe_run_detail(&row).expect("probe detail");

        assert_eq!(detail.probe_run_id, "probe-run-local");
        assert_eq!(detail.advice_id, "advice-probe");
        assert_eq!(detail.probe_kind, "local_evidence");
        assert_eq!(detail.target, "routine_next_step");
        assert_eq!(detail.status, "succeeded");
        assert_eq!(detail.evidence_count, 2);
        assert!(detail.summary.contains("recent events"));
    }

    #[test]
    fn decision_trail_detail_summarizes_control_chain() {
        let row = DashboardRow {
            number: 11,
            kind: DashboardRowKind::DecisionTrail,
            severity: DashboardSeverity::Warning,
            agent: Some("codex".into()),
            protocol: "decision-trail".into(),
            summary: "force_verification -> codex".into(),
            detail: r#"{
              "advice": {
                "advice_id": "advice-force",
                "final_action": {
                  "type": "force_verification",
                  "suite": "targeted",
                  "blocking": true
                },
                "control_rationale": {
                  "selected_action": "force_verification",
                  "reason": "verification is stale"
                }
              },
              "packet": {
                "packet_id": "packet-force",
                "target_agent": "codex",
                "urgency": "blocking"
              },
              "dispatch_result": {
                "packet_id": "packet-force",
                "target_agent": "codex",
                "status": "outbox_written"
              },
              "outcomes": [
                { "outcome_id": "out-pass", "status": "succeeded" },
                { "outcome_id": "out-fail", "status": "failed" }
              ]
            }"#
            .into(),
        };

        let detail = decision_trail_detail(&row).expect("decision detail");

        assert_eq!(detail.advice_id, "advice-force");
        assert_eq!(detail.action, "force_verification");
        assert_eq!(detail.target_agent, "codex");
        assert_eq!(detail.packet_id, "packet-force");
        assert_eq!(detail.urgency, "blocking");
        assert_eq!(detail.dispatch_status, "outbox_written");
        assert_eq!(detail.outcome_count, 2);
        assert_eq!(detail.failed_outcome_count, 1);
        assert_eq!(detail.rationale, "verification is stale");
    }

    #[test]
    fn tray_toggle_id_maps_to_toggle_command() {
        assert_eq!(
            tray_command_from_id(&MenuId::new(TRAY_TOGGLE_ID)),
            Some(TrayCommand::Toggle)
        );
    }

    #[test]
    fn running_agent_summary_includes_process_and_workspace() {
        let agent = RunningAgent::new(42, coding_agent_monitor::AgentKind::Codex, "codex.exe")
            .with_cwd(Some(PathBuf::from("F:/repo")));

        assert_eq!(
            running_agent_summary(&agent),
            "pid 42 · codex.exe · F:/repo"
        );
    }

    #[test]
    fn running_agent_summary_reports_missing_workspace() {
        let agent = RunningAgent::new(42, coding_agent_monitor::AgentKind::ClaudeCode, "node.exe");

        assert_eq!(
            running_agent_summary(&agent),
            "pid 42 · node.exe · cwd unavailable"
        );
    }

    #[test]
    fn review_summary_reports_intervention_count() {
        let report = coding_agent_monitor::AgentReviewReport {
            workspace: "F:/repo".into(),
            status: coding_agent_monitor::AgentReviewStatus::Intervene,
            findings: vec![coding_agent_monitor::AgentReviewFinding {
                severity: DashboardSeverity::Critical,
                category: "unverified_completion".into(),
                agent: Some("codex".into()),
                evidence: "done without tests".into(),
                recommended_action: coding_agent_monitor::AgentReviewAction::ForceVerification,
            }],
        };

        assert_eq!(review_summary_text(&report), "Intervene · 1 finding");
    }
}
