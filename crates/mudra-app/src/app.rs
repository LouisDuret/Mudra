//! Application en zone de notification (§7.10, D-14) : une seule boucle
//! d'événements winit sur le thread principal pilote l'icône de notification,
//! le raccourci global, la fenêtre d'aperçu, l'overlay et le rechargement de
//! la configuration.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::bounded;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use mudra_actions::{ActionSender, Feedback};
use mudra_config::{Config, Control, InternalOp, Level};
use mudra_gestures::EngineState;
use mudra_gestures::custom::CustomPose;
use mudra_gestures::engine::Reason;
use notify::{RecursiveMode, Watcher};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId, WindowLevel};

use crate::halo::Halo;
use crate::pipeline::{
    self, Command, Options, Pipeline, PipelineEvent, PreviewFrame, RecordEvent, RecordKind,
    RecordRequest, Source, Status,
};
use crate::platform;
use crate::ui::gestures::GesturesState;
use crate::ui::overlay::{self, OverlayModel};
use crate::ui::preview::{self, LogEntry, LogKind, PreviewModel, RecordForm, Tab, UiAction};
use crate::ui::{EguiWindow, TrayKind, theme, tray_icon_rgba};

/// Délai avant de détruire la fenêtre d'overlay masquée : évite de la recréer
/// si le moteur est réarmé dans la foulée.
const OVERLAY_LINGER: Duration = Duration::from_secs(20);

pub struct RunOptions {
    pub preview: bool,
    pub paused: bool,
    pub source: Source,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            preview: false,
            paused: false,
            source: Source::Camera,
        }
    }
}

#[derive(Debug)]
enum UserEvent {
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    Hotkey(GlobalHotKeyEvent),
    Feedback(Feedback),
    /// Le pipeline a du nouveau (événements, état, image d'aperçu).
    Wake,
    ConfigChanged,
    ModelsChanged,
}

struct Tray {
    icon: TrayIcon,
    icons: HashMap<TrayKind, Icon>,
    status_item: MenuItem,
    pause_item: MenuItem,
    pause: MenuId,
    preview: MenuId,
    record: MenuId,
    reload: MenuId,
    config_dir: MenuId,
    logs: MenuId,
    quit: MenuId,
    shown: Option<(TrayKind, String)>,
}

impl Tray {
    fn new() -> Result<Self> {
        let status_item = MenuItem::new("Mudra", false, None);
        let pause_item = MenuItem::new("Mettre en pause", true, None);
        let preview = MenuItem::new("Ouvrir l'aperçu", true, None);
        let record = MenuItem::new("Gestes et actions…", true, None);
        let reload = MenuItem::new("Recharger la configuration", true, None);
        let config_dir = MenuItem::new("Ouvrir le dossier de configuration", true, None);
        let logs = MenuItem::new("Ouvrir les journaux", true, None);
        let quit = MenuItem::new("Quitter", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status_item,
            &PredefinedMenuItem::separator(),
            &pause_item,
            &preview,
            &record,
            &PredefinedMenuItem::separator(),
            &reload,
            &config_dir,
            &logs,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .context("construction du menu")?;
        let mut icons = HashMap::new();
        for kind in TrayKind::ALL {
            icons.insert(
                kind,
                Icon::from_rgba(tray_icon_rgba(kind), 32, 32).context("icône de notification")?,
            );
        }
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Mudra")
            .with_icon(icons[&TrayKind::Locked].clone())
            .build()
            .context("création de l'icône de notification")?;
        Ok(Self {
            icon,
            icons,
            pause: pause_item.id().clone(),
            preview: preview.id().clone(),
            record: record.id().clone(),
            reload: reload.id().clone(),
            config_dir: config_dir.id().clone(),
            logs: logs.id().clone(),
            quit: quit.id().clone(),
            status_item,
            pause_item,
            shown: None,
        })
    }

    /// Met à jour l'icône, l'infobulle et la ligne d'état du menu (EF-36, EF-70).
    fn show(&mut self, kind: TrayKind, text: &str) {
        if self
            .shown
            .as_ref()
            .is_some_and(|(k, t)| *k == kind && t == text)
        {
            return;
        }
        let _ = self.icon.set_icon(self.icons.get(&kind).cloned());
        let _ = self.icon.set_tooltip(Some(format!("Mudra — {text}")));
        self.status_item.set_text(format!("Mudra — {text}"));
        self.pause_item.set_text(if kind == TrayKind::Paused {
            "Reprendre"
        } else {
            "Mettre en pause"
        });
        self.shown = Some((kind, text.to_string()));
    }
}

struct PreviewWindow {
    window: EguiWindow,
    texture: Option<egui::TextureHandle>,
    frame: Option<PreviewFrame>,
}

struct OverlayWindow {
    window: EguiWindow,
    visible: bool,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    config_path: PathBuf,
    config: Arc<Config>,
    options: RunOptions,
    actions: ActionSender,
    pipeline: Option<Pipeline>,
    pipeline_error: Option<String>,
    status: Status,
    tray: Option<Tray>,
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    watcher: Option<notify::RecommendedWatcher>,
    preview: Option<PreviewWindow>,
    overlay: Option<OverlayWindow>,
    overlay_title: String,
    overlay_detail: String,
    overlay_color: egui::Color32,
    overlay_hide_at: Option<Instant>,
    /// L'overlay doit être à l'écran ; la fenêtre est créée à la demande.
    overlay_wanted: bool,
    started: Instant,
    /// Destruction différée de la fenêtre d'overlay une fois masquée.
    overlay_destroy_at: Option<Instant>,
    /// Halo du curseur, créé au premier contrôle de la souris.
    halo: Option<Halo>,
    /// Un contrôle de la souris qui demande le halo est en cours.
    halo_wanted: bool,
    log: VecDeque<LogEntry>,
    record: RecordForm,
    /// Poses personnalisées chargées, pour l'onglet « Gestes ».
    custom: Vec<CustomPose>,
    /// Gestes des classifieurs chargés, pour l'onglet « Gestes ».
    known: mudra_config::KnownGestures,
    tab: Tab,
    gestures: GesturesState,
    /// Contenu du fichier avant la dernière modification faite dans l'interface.
    undo_text: Option<String>,
    reload_at: Option<Instant>,
    restart_at: Option<Instant>,
    initialized: bool,
    startup_notice: Option<String>,
}

/// Charge et valide la configuration contre les gestes des classifieurs
/// qu'elle désigne (D-10). Renvoie aussi les avertissements.
fn load_checked(path: &Path) -> Result<(Config, Vec<String>), String> {
    let config = mudra_config::load(path).map_err(|e| e.to_string())?;
    let (known, _) = crate::cli::known_gestures(&config);
    let issues = mudra_config::validate(&config, &known);
    if issues.iter().any(|i| i.level == Level::Error) {
        let lines: Vec<String> = issues
            .iter()
            .filter(|i| i.level == Level::Error)
            .map(|i| i.to_string())
            .collect();
        return Err(format!("{} : {}", path.display(), lines.join(" ; ")));
    }
    Ok((config, issues.iter().map(|i| i.to_string()).collect()))
}

pub fn run(config_path: PathBuf, explicit: bool, options: RunOptions) -> Result<()> {
    let Some(_instance) = platform::SingleInstance::acquire() else {
        platform::error_box("Mudra est déjà lancé : regarde dans la zone de notification.");
        return Ok(());
    };
    if !explicit {
        mudra_config::ensure_default(&config_path)?;
    }
    // Au démarrage, il n'y a pas de « dernière configuration valide » : si le
    // fichier est invalide, on part de la configuration par défaut et on le dit.
    let (config, startup_notice) = match load_checked(&config_path) {
        Ok((config, _)) => (config, None),
        Err(e) => {
            let fallback =
                mudra_config::parse(mudra_config::DEFAULT_CONFIG_TOML, Path::new("default.toml"))?;
            (
                fallback,
                Some(format!(
                    "Configuration invalide, valeurs par défaut utilisées — {e}"
                )),
            )
        }
    };
    let _log = platform::init_logging(&config.general.log_level, cfg!(debug_assertions));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), config = %config_path.display(), "démarrage de Mudra");

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("boucle d'événements")?;
    let proxy = event_loop.create_proxy();
    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Menu(e));
    }));
    let p = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Tray(e));
    }));
    let p = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Hotkey(e));
    }));

    let (feedback_tx, feedback_rx) = bounded(64);
    let actions = mudra_actions::spawn(config.general.allow_commands, feedback_tx)
        .context("thread des actions")?;
    let p = proxy.clone();
    std::thread::Builder::new()
        .name("mudra-feedback".into())
        .spawn(move || {
            for feedback in feedback_rx {
                if p.send_event(UserEvent::Feedback(feedback)).is_err() {
                    break;
                }
            }
        })?;

    let (known, _) = crate::cli::known_gestures(&config);
    let mut app = App {
        proxy,
        config_path,
        config: Arc::new(config),
        options,
        actions,
        pipeline: None,
        pipeline_error: None,
        status: Status::default(),
        tray: None,
        hotkeys: None,
        hotkey: None,
        watcher: None,
        preview: None,
        overlay: None,
        overlay_title: String::new(),
        overlay_detail: String::new(),
        overlay_color: theme::MUTED,
        overlay_hide_at: None,
        overlay_wanted: false,
        started: Instant::now(),
        overlay_destroy_at: None,
        halo: None,
        halo_wanted: false,
        log: VecDeque::with_capacity(128),
        record: RecordForm::default(),
        custom: Vec::new(),
        known,
        tab: Tab::default(),
        gestures: GesturesState::default(),
        undo_text: None,
        reload_at: None,
        restart_at: None,
        initialized: false,
        startup_notice,
    };
    event_loop
        .run_app(&mut app)
        .context("boucle d'événements")?;
    Ok(())
}

impl App {
    fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        if self.log.len() >= 120 {
            self.log.pop_front();
        }
        self.log.push_back(LogEntry {
            time: platform::clock(),
            kind,
            text: text.into(),
        });
    }

    /// Notification : journal de l'aperçu et overlay temporaire.
    fn notice(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            tracing::warn!("{text}");
        }
        self.log(
            if error { LogKind::Error } else { LogKind::Info },
            text.clone(),
        );
        self.show_overlay(
            if error { "Attention" } else { "Mudra" }.to_string(),
            text,
            if error { theme::RED } else { theme::MUTED },
            Some(Duration::from_secs(if error { 5 } else { 3 })),
        );
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) {
        match Tray::new() {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => tracing::error!("{e:#}"),
        }
        match GlobalHotKeyManager::new() {
            Ok(manager) => self.hotkeys = Some(manager),
            Err(e) => tracing::error!(erreur = %e, "raccourci global indisponible"),
        }
        self.register_hotkey();
        self.watch_files();
        if let Err(e) = platform::set_autostart(self.config.general.autostart) {
            tracing::warn!(erreur = %e, "lancement à l'ouverture de session");
        }
        self.load_custom();
        self.start_pipeline();
        if let Some(message) = self.startup_notice.take() {
            self.notice(message, true);
        }
        if self.options.preview {
            self.open_preview(event_loop);
        }
        self.refresh_tray();
    }

    fn register_hotkey(&mut self) {
        let Some(manager) = &self.hotkeys else { return };
        if let Some(old) = self.hotkey.take() {
            let _ = manager.unregister(old);
        }
        match HotKey::from_str(&self.config.activation.pause_hotkey) {
            Ok(hotkey) => match manager.register(hotkey) {
                Ok(()) => self.hotkey = Some(hotkey),
                Err(e) => {
                    let text = format!(
                        "Raccourci {} indisponible : {e}",
                        self.config.activation.pause_hotkey
                    );
                    self.notice(text, true);
                }
            },
            Err(e) => {
                let text = format!(
                    "Raccourci {} invalide : {e}",
                    self.config.activation.pause_hotkey
                );
                self.notice(text, true);
            }
        }
    }

    /// Surveille le fichier de configuration (D-15) et les classifieurs ONNX (EF-65).
    fn watch_files(&mut self) {
        let proxy = self.proxy.clone();
        let config_file = self.config_path.file_name().map(|n| n.to_os_string());
        let handler = move |result: notify::Result<notify::Event>| {
            let Ok(event) = result else { return };
            if !(event.kind.is_modify() || event.kind.is_create()) {
                return;
            }
            for path in &event.paths {
                if path.file_name().map(|n| n.to_os_string()) == config_file {
                    let _ = proxy.send_event(UserEvent::ConfigChanged);
                } else if path.extension().is_some_and(|e| e == "onnx") {
                    let _ = proxy.send_event(UserEvent::ModelsChanged);
                }
            }
        };
        let mut watcher = match notify::recommended_watcher(handler) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(erreur = %e, "surveillance de la configuration indisponible");
                return;
            }
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(dir) = self.config_path.parent() {
            dirs.push(dir.to_path_buf());
        }
        for raw in [&self.config.models.static_, &self.config.models.dynamic] {
            if let Some(dir) = mudra_config::resolve_model_path(raw).parent()
                && dir.is_dir()
                && !dirs.iter().any(|d| d == dir)
            {
                dirs.push(dir.to_path_buf());
            }
        }
        for dir in dirs {
            if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
                tracing::warn!(dossier = %dir.display(), erreur = %e, "surveillance impossible");
            }
        }
        self.watcher = Some(watcher);
    }

    fn start_pipeline(&mut self) {
        self.halo_wanted = false;
        self.refresh_halo();
        // L'ancien pipeline est arrêté d'abord : la caméra doit être libre.
        let was_paused = self
            .pipeline
            .take()
            .map(|p| p.status().state == EngineState::Paused);
        let proxy = self.proxy.clone();
        let notify: pipeline::Notify = Arc::new(move || {
            let _ = proxy.send_event(UserEvent::Wake);
        });
        let result = pipeline::start(Options {
            config: self.config.clone(),
            source: self.options.source.clone(),
            actions: Some(self.actions.clone()),
            notify: Some(notify),
            start_paused: was_paused.unwrap_or(self.options.paused),
        });
        match result {
            Ok(pipeline) => {
                pipeline.send(Command::SetPreview(self.preview.is_some()));
                if self.gestures.dry_run {
                    pipeline.send(Command::SetDryRun(true));
                }
                self.status = pipeline.status();
                self.pipeline = Some(pipeline);
                self.pipeline_error = None;
            }
            Err(e) => {
                let message = format!("{e:#}");
                tracing::error!("{message}");
                self.pipeline_error = Some(message.clone());
                self.notice(message, true);
            }
        }
    }

    fn refresh_tray(&mut self) {
        let (kind, text) = if self.pipeline_error.is_some() {
            (TrayKind::Error, "erreur (voir l'aperçu)".to_string())
        } else {
            (
                TrayKind::of(self.status.state),
                self.status.state.label_fr().to_string(),
            )
        };
        if let Some(tray) = &mut self.tray {
            tray.show(kind, &text);
        }
    }

    /// Le halo entoure le curseur tant que l'index le pilote réellement : en
    /// mode essai, la souris ne bouge pas.
    fn refresh_halo(&mut self) {
        let visible = self.halo_wanted && !self.gestures.dry_run;
        if visible && self.halo.is_none() {
            self.halo = Halo::spawn();
        }
        if let Some(halo) = &mut self.halo {
            halo.set_visible(visible);
        }
    }

    // ----- Overlay -----

    fn create_overlay(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = Window::default_attributes()
            .with_title("Mudra")
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size(LogicalSize::new(overlay::WIDTH, overlay::HEIGHT))
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_active(false)
            .with_visible(false)
            .with_skip_taskbar(true);
        match EguiWindow::new(event_loop, attributes) {
            Ok(window) => {
                // En bas au centre de l'écran principal.
                if let Some(monitor) = event_loop
                    .primary_monitor()
                    .or_else(|| event_loop.available_monitors().next())
                {
                    let size = window.window().outer_size();
                    let area = monitor.size();
                    let origin = monitor.position();
                    let margin = (90.0 * monitor.scale_factor()) as i32;
                    window.window().set_outer_position(PhysicalPosition::new(
                        origin.x + (area.width as i32 - size.width as i32) / 2,
                        origin.y + area.height as i32 - size.height as i32 - margin,
                    ));
                }
                let _ = window.window().set_cursor_hittest(false);
                platform::make_overlay(window.window());
                self.overlay = Some(OverlayWindow {
                    window,
                    visible: false,
                });
            }
            Err(e) => tracing::warn!("overlay indisponible : {e:#}"),
        }
    }

    /// Demande l'affichage de l'overlay. Sa fenêtre (et son contexte graphique)
    /// n'existe que lorsqu'elle sert : elle est créée au prochain tour de la
    /// boucle d'événements et détruite peu après avoir été masquée.
    fn show_overlay(
        &mut self,
        title: String,
        detail: String,
        color: egui::Color32,
        hide_after: Option<Duration>,
    ) {
        if !self.config.general.overlay {
            return;
        }
        self.overlay_title = title;
        self.overlay_detail = detail;
        self.overlay_color = color;
        self.overlay_hide_at = hide_after.map(|d| Instant::now() + d);
        self.overlay_wanted = true;
        self.overlay_destroy_at = None;
        if let Some(overlay) = &mut self.overlay {
            if !overlay.visible {
                overlay.visible = true;
                platform::show_no_activate(overlay.window.window(), true);
            }
            overlay.window.request_redraw();
        }
    }

    fn hide_overlay(&mut self) {
        self.overlay_hide_at = None;
        self.overlay_wanted = false;
        if let Some(overlay) = &mut self.overlay
            && overlay.visible
        {
            overlay.visible = false;
            platform::show_no_activate(overlay.window.window(), false);
            self.overlay_destroy_at = Some(Instant::now() + OVERLAY_LINGER);
        }
    }

    /// L'overlay reste affiché tant que le moteur est armé ou en contrôle continu.
    fn overlay_for_state(&mut self, detail: Option<String>) {
        let state = self.status.state;
        let detail = detail.unwrap_or_default();
        let color = theme::state_color(state);
        match state {
            EngineState::Armed | EngineState::Continuous => {
                self.show_overlay(state.label_fr().to_string(), detail, color, None);
            }
            EngineState::Locked | EngineState::Paused => {
                self.show_overlay(
                    state.label_fr().to_string(),
                    detail,
                    color,
                    Some(Duration::from_millis(1800)),
                );
            }
        }
    }

    // ----- Aperçu -----

    fn open_preview(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(preview) = &self.preview {
            preview.window.window().set_minimized(false);
            preview.window.window().focus_window();
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Mudra — aperçu")
            .with_inner_size(LogicalSize::new(1180.0, 720.0))
            .with_min_inner_size(LogicalSize::new(820.0, 520.0));
        match EguiWindow::new(event_loop, attributes) {
            Ok(window) => {
                self.preview = Some(PreviewWindow {
                    window,
                    texture: None,
                    frame: None,
                });
                if let Some(pipeline) = &self.pipeline {
                    pipeline.send(Command::SetPreview(true));
                }
            }
            Err(e) => self.notice(format!("Aperçu indisponible : {e:#}"), true),
        }
    }

    /// Détruit la fenêtre : plus aucun rendu ni copie d'image (EF-72).
    fn close_preview(&mut self) {
        self.preview = None;
        if let Some(pipeline) = &self.pipeline {
            pipeline.send(Command::SetPreview(false));
        }
    }

    fn paint_preview(&mut self) {
        let Some(preview) = &mut self.preview else {
            return;
        };
        if let Some(pipeline) = &self.pipeline {
            self.status = pipeline.status();
            let fresh = pipeline
                .shared
                .preview
                .lock()
                .ok()
                .and_then(|mut slot| slot.take());
            if let Some(frame) = fresh {
                let image = egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb);
                match &mut preview.texture {
                    Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        preview.texture = Some(preview.window.ctx().load_texture(
                            "camera",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                preview.frame = Some(frame);
            }
        }
        if self.status.state == EngineState::Paused || self.pipeline.is_none() {
            preview.frame = None;
        }
        let mut model = PreviewModel {
            status: &self.status,
            config: &self.config,
            log: &self.log,
            frame: preview.frame.as_ref(),
            texture: preview.texture.as_ref(),
            pipeline_error: self.pipeline_error.as_deref(),
            record: &mut self.record,
            known: &self.known,
            custom: &self.custom,
            tab: &mut self.tab,
            gestures: &mut self.gestures,
        };
        let mut actions = Vec::new();
        preview
            .window
            .paint(theme::PANEL, |ui| actions = preview::draw(ui, &mut model));
        for action in actions {
            self.ui_action(action);
        }
    }

    fn paint_overlay(&mut self) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        let model = OverlayModel {
            color: self.overlay_color,
            title: &self.overlay_title,
            detail: &self.overlay_detail,
        };
        overlay
            .window
            .paint(theme::PANEL, |ui| overlay::draw(ui, &model));
    }

    // ----- Poses personnalisées -----

    /// Relit les poses perso du disque et donne leurs noms à l'interface.
    fn load_custom(&mut self) {
        let (poses, notices) = mudra_gestures::custom::load_dir(&crate::custom::dir());
        for notice in notices {
            self.log(LogKind::Error, notice);
        }
        theme::set_custom_names(poses.iter().map(|p| (p.label.as_str(), p.name.as_str())));
        self.custom = poses;
    }

    /// Après une modification des poses perso : les classifieurs et la liste
    /// des gestes connus suivent, sans redémarrer la caméra.
    fn reload_custom(&mut self) {
        self.load_custom();
        self.known = crate::cli::known_gestures(&self.config).0;
        self.gestures.sensitivity = None;
        if let Some(pipeline) = &self.pipeline {
            pipeline.send(Command::ReloadClassifiers);
        }
    }

    fn train_take(&mut self, label: String) {
        let Some(pipeline) = &self.pipeline else {
            if let Some(trainer) = &mut self.gestures.trainer {
                trainer.error = Some("Mudra n'est pas démarré.".into());
            }
            return;
        };
        self.record.active = true;
        self.record.progress = None;
        self.record.status = Some(("Prépare-toi…".into(), false));
        pipeline.send(Command::Record(RecordRequest {
            label,
            kind: RecordKind::Static {
                seconds: crate::custom::TAKE_SECONDS,
            },
            dataset_root: crate::custom::dataset_root(),
        }));
    }

    fn train_finish(&mut self, label: String, name: Option<String>) {
        let created = name.is_some();
        let trained =
            crate::custom::train_in(&crate::custom::dir(), &label, name.as_deref(), &self.config);
        match trained {
            Ok(pose) => {
                self.gestures.trainer = None;
                self.reload_custom();
                self.log(
                    LogKind::Info,
                    format!(
                        "Pose « {} » apprise sur {} exemples",
                        pose.name,
                        pose.examples.len()
                    ),
                );
                if created {
                    // Une pose sans action ne sert à rien : le formulaire s'ouvre.
                    self.gestures.open_new_binding(&label, true, false);
                }
            }
            Err(e) => match &mut self.gestures.trainer {
                Some(trainer) => trainer.error = Some(e),
                None => self.notice(e, true),
            },
        }
    }

    fn edit_pose(&mut self, label: &str, edit: impl FnOnce(&mut CustomPose)) {
        match crate::custom::update_in(&crate::custom::dir(), label, edit) {
            Ok(()) => self.reload_custom(),
            Err(e) => self.gestures.error = Some(e),
        }
    }

    /// Supprime une pose perso, ses prises et les actions qui l'utilisent.
    fn delete_pose(&mut self, label: &str) {
        let activation = &self.config.activation;
        let reserved = activation.wake_gesture == label
            || activation.lock_gesture == label
            || self.config.continuous.iter().any(|c| {
                c.enter_gesture == label
                    || matches!(&c.control, Control::Mouse { click: Some(click), .. } if click == label)
            });
        if reserved {
            self.gestures.error = Some(
                "Cette pose sert à l'activation ou à un contrôle continu : retire-la d'abord \
                 de config.toml."
                    .into(),
            );
            return;
        }
        self.gestures.error = None;
        // Du dernier au premier : les indices restants ne bougent pas.
        let indices: Vec<usize> = (0..self.config.bindings.len())
            .rev()
            .filter(|i| self.config.bindings[*i].gesture == label)
            .collect();
        for index in indices {
            self.edit_binding(mudra_config::edit::BindingEdit::Remove(index));
            if self.gestures.error.is_some() {
                return;
            }
        }
        // Rétablir ces actions sans la pose donnerait une configuration invalide.
        self.undo_text = None;
        self.gestures.can_undo = false;
        match crate::custom::delete_in(&crate::custom::dir(), label) {
            Ok(()) => {
                self.reload_custom();
                self.log(LogKind::Info, format!("Pose « {label} » supprimée"));
            }
            Err(e) => self.gestures.error = Some(e),
        }
    }

    /// Applique une modification de binding faite dans l'onglet « Gestes » :
    /// écrit le fichier, puis recharge la configuration.
    fn edit_binding(&mut self, edit: mudra_config::edit::BindingEdit) {
        match self.write_binding_edit(&edit) {
            Ok(previous) => {
                self.undo_text = Some(previous);
                self.gestures.can_undo = true;
                self.gestures.error = None;
                self.gestures.editor = None;
                self.reload_config(false);
            }
            Err(e) => self.gestures.error = Some(e),
        }
    }

    /// Renvoie le contenu du fichier avant la modification.
    fn write_binding_edit(&self, edit: &mudra_config::edit::BindingEdit) -> Result<String, String> {
        let path = &self.config_path;
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("lecture de {} : {e}", path.display()))?;
        // Les indices de l'interface sont ceux de la configuration chargée.
        let on_disk = mudra_config::parse(&text, path).map_err(|e| e.to_string())?;
        if on_disk.bindings != self.config.bindings {
            return Err("Le fichier de configuration a changé entre-temps : réessaie.".into());
        }
        let updated = mudra_config::edit::apply_binding_edit(&text, edit)?;
        let config = mudra_config::parse(&updated, path).map_err(|e| e.to_string())?;
        let errors: Vec<String> = mudra_config::validate(&config, &self.known)
            .iter()
            .filter(|i| i.level == Level::Error)
            .map(|i| i.to_string())
            .collect();
        if !errors.is_empty() {
            return Err(errors.join(" ; "));
        }
        mudra_config::edit::write_atomic(path, &updated)
            .map_err(|e| format!("écriture de {} : {e}", path.display()))?;
        Ok(text)
    }

    fn undo_edit(&mut self) {
        let Some(text) = self.undo_text.take() else {
            return;
        };
        self.gestures.can_undo = false;
        match mudra_config::edit::write_atomic(&self.config_path, &text) {
            Ok(()) => {
                self.gestures.error = None;
                self.reload_config(false);
            }
            Err(e) => self.gestures.error = Some(format!("annulation impossible : {e}")),
        }
    }

    fn ui_action(&mut self, action: UiAction) {
        match action {
            UiAction::EditBinding(edit) => self.edit_binding(edit),
            UiAction::UndoEdit => self.undo_edit(),
            UiAction::TestAction(action) => match &self.pipeline {
                Some(pipeline) => pipeline.send(Command::TestAction(action)),
                None => self.notice("Mudra n'est pas démarré : action non testée", true),
            },
            UiAction::SetDryRun(on) => {
                if let Some(pipeline) = &self.pipeline {
                    pipeline.send(Command::SetDryRun(on));
                }
                self.refresh_halo();
                self.log(
                    LogKind::Info,
                    if on {
                        "Mode essai : les gestes n'exécutent plus rien"
                    } else {
                        "Mode essai terminé"
                    }
                    .to_string(),
                );
            }
            UiAction::TogglePause => self.toggle_pause(),
            UiAction::Lock => {
                if let Some(pipeline) = &self.pipeline {
                    pipeline.send(Command::Lock);
                }
            }
            UiAction::TrainTake { label } => self.train_take(label),
            UiAction::TrainFinish { label, name } => self.train_finish(label, name),
            UiAction::DiscardTakes(files) => crate::custom::discard_takes(&files),
            UiAction::SetPoseSensitivity { label, value } => {
                self.edit_pose(&label, |pose| pose.sensitivity = value);
            }
            UiAction::RenamePose { label, name } => self.edit_pose(&label, |pose| pose.name = name),
            UiAction::DeletePose(label) => self.delete_pose(&label),
            UiAction::CancelRecording => {
                if let Some(pipeline) = &self.pipeline {
                    pipeline.send(Command::CancelRecording);
                }
            }
            UiAction::ReloadConfig => self.reload_config(true),
            UiAction::OpenConfigFolder => {
                if let Some(dir) = self.config_path.parent() {
                    platform::open_path(dir);
                }
            }
            UiAction::OpenLogs => {
                if let Ok(dir) = mudra_config::logs_dir() {
                    platform::open_path(&dir);
                }
            }
        }
    }

    fn toggle_pause(&mut self) {
        match &self.pipeline {
            Some(pipeline) => pipeline.send(Command::TogglePause),
            // Après une erreur de démarrage, « Reprendre » retente le lancement.
            None => self.start_pipeline(),
        }
    }

    // ----- Configuration -----

    /// Recharge la configuration (CA-08) : une configuration invalide est
    /// refusée avec un message précis et la précédente reste active.
    fn reload_config(&mut self, manual: bool) {
        let (config, warnings) = match load_checked(&self.config_path) {
            Ok(v) => v,
            Err(e) => {
                self.notice(
                    format!("Configuration refusée, l'ancienne reste active — {e}"),
                    true,
                );
                return;
            }
        };
        if config == *self.config && self.pipeline.is_some() {
            if manual {
                self.notice("Configuration inchangée", false);
            }
            return;
        }
        let old = std::mem::replace(&mut self.config, Arc::new(config));
        self.known = crate::cli::known_gestures(&self.config).0;
        if old.bindings != self.config.bindings {
            // Les indices du formulaire ouvert ne correspondent plus.
            self.gestures.editor = None;
        }
        for warning in warnings {
            self.log(LogKind::Info, warning);
        }
        if old.activation.pause_hotkey != self.config.activation.pause_hotkey {
            self.register_hotkey();
        }
        if old.general.autostart != self.config.general.autostart
            && let Err(e) = platform::set_autostart(self.config.general.autostart)
        {
            self.notice(format!("Lancement à l'ouverture de session : {e}"), true);
        }
        if !self.config.general.overlay {
            self.hide_overlay();
        }
        let needs_restart = old.camera != self.config.camera
            || old.inference != self.config.inference
            || old.models != self.config.models
            || self.pipeline.is_none();
        if needs_restart {
            self.start_pipeline();
            self.watch_files();
        } else if let Some(pipeline) = &self.pipeline {
            pipeline.send(Command::Reload(self.config.clone()));
        }
        tracing::info!(redemarrage = needs_restart, "configuration rechargée");
        if self.pipeline_error.is_none() {
            self.notice("Configuration rechargée", false);
        }
        self.refresh_tray();
    }

    // ----- Événements du pipeline et des actions -----

    fn drain_pipeline(&mut self) {
        let Some(pipeline) = &self.pipeline else {
            return;
        };
        let events: Vec<PipelineEvent> = pipeline.events.try_iter().collect();
        let previous = self.status.state;
        self.status = pipeline.status();
        for event in events {
            self.on_pipeline_event(event);
        }
        if previous != self.status.state {
            self.refresh_tray();
        }
        if let Some(preview) = &self.preview {
            preview.window.request_redraw();
        }
    }

    fn on_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::State { to, reason, .. } => {
                self.status.state = to;
                if to != EngineState::Continuous {
                    self.halo_wanted = false;
                    self.refresh_halo();
                }
                let why = match reason {
                    Reason::WakeGesture => "geste de réveil",
                    Reason::LockGesture => "geste de verrouillage",
                    Reason::ArmedTimeout => "délai écoulé",
                    Reason::Pause => "pause",
                    Reason::Resume => "reprise",
                    Reason::LockCommand => "verrouillage demandé",
                    Reason::ContinuousEnter => "contrôle continu",
                    Reason::PoseReleased => "pose relâchée",
                    Reason::HandLost => "main perdue",
                    Reason::ConfigReload => "configuration rechargée",
                };
                let text = match reason {
                    Reason::Pause | Reason::Resume => to.label_fr().to_string(),
                    _ => format!("{} ({why})", to.label_fr()),
                };
                self.log(LogKind::State, text);
                self.refresh_tray();
                // Au lancement, l'icône de notification suffit : pas d'overlay.
                let starting = matches!(reason, Reason::Resume | Reason::Pause)
                    && self.started.elapsed() < Duration::from_secs(5);
                if self.config.general.overlay && !self.record.active && !starting {
                    let detail = match to {
                        EngineState::Armed => "prêt pour un geste",
                        EngineState::Paused => "caméra libérée",
                        _ => why,
                    };
                    self.overlay_for_state(Some(detail.to_string()));
                }
            }
            PipelineEvent::Gesture {
                binding,
                gesture,
                action,
                confidence,
                right,
                repeat,
                latency_ms,
            } => {
                self.gestures.flash = Some((binding, Instant::now()));
                let name = theme::gesture_name(&gesture).to_string();
                if !repeat {
                    let side = if right { "main droite" } else { "main gauche" };
                    self.log(
                        LogKind::Gesture,
                        format!(
                            "{name} ({:.0} %, {side}) : {action} · {latency_ms:.1} ms",
                            confidence * 100.0
                        ),
                    );
                }
                if self.config.general.overlay {
                    self.overlay_for_state(Some(format!("{name} · {action}")));
                }
            }
            PipelineEvent::Continuous { name, active } => {
                self.halo_wanted = active
                    && self.config.continuous.iter().any(|c| {
                        c.name == name && matches!(c.control, Control::Mouse { halo: true, .. })
                    });
                self.refresh_halo();
                self.log(
                    LogKind::State,
                    format!(
                        "Contrôle « {name} » {}",
                        if active { "actif" } else { "terminé" }
                    ),
                );
                if self.config.general.overlay {
                    let detail = if active {
                        format!("contrôle « {name} »")
                    } else {
                        "prêt pour un geste".into()
                    };
                    self.overlay_for_state(Some(detail));
                }
            }
            PipelineEvent::Notice { error, message } => {
                // Les informations de démarrage (classifieurs utilisés…) vont au
                // journal ; l'overlay est réservé à ce qui survient ensuite.
                if !error && self.started.elapsed() < Duration::from_secs(5) {
                    self.log(LogKind::Info, message);
                } else {
                    self.notice(message, error);
                }
            }
            PipelineEvent::Recording(event) => self.on_record_event(event),
        }
    }

    fn on_record_event(&mut self, event: RecordEvent) {
        let form = &mut self.record;
        match event {
            RecordEvent::Countdown { seconds_left, .. } => {
                form.banner = Some((seconds_left.to_string(), Instant::now()));
                form.progress = Some(0.0);
                form.status = Some((format!("Début dans {seconds_left}…"), false));
            }
            RecordEvent::Go { .. } => {
                form.banner = Some(("⏺".into(), Instant::now()));
                form.status = Some(("Tiens la pose…".into(), false));
            }
            RecordEvent::Progress { fraction, .. } => {
                form.progress = Some(fraction);
            }
            RecordEvent::Done { path, frames } => {
                form.active = false;
                form.banner = None;
                form.progress = None;
                form.status = None;
                let Some(trainer) = &mut self.gestures.trainer else {
                    return;
                };
                if (frames as usize) < mudra_gestures::custom::MIN_TAKE_FRAMES {
                    crate::custom::discard_takes(&[path]);
                    trainer.error = Some(
                        "La main n'a presque pas été vue pendant cette prise : elle n'est pas \
                         gardée. Place ta main dans le cadre et recommence."
                            .into(),
                    );
                } else {
                    trainer.takes.push(path);
                }
            }
            RecordEvent::Failed(message) => {
                form.active = false;
                form.banner = None;
                form.progress = None;
                form.status = None;
                // Une prise arrêtée à la main n'est pas une erreur à afficher.
                if !message.contains("(annulé)")
                    && let Some(trainer) = &mut self.gestures.trainer
                {
                    trainer.error = Some(message.clone());
                }
                self.log(LogKind::Error, message);
            }
        }
    }

    fn on_feedback(&mut self, feedback: Feedback) {
        match feedback {
            Feedback::Done {
                summary,
                detail: Some(detail),
                ..
            } => {
                self.log(LogKind::Info, format!("{summary} : {detail}"));
                if self.config.general.overlay && self.overlay.as_ref().is_some_and(|o| o.visible) {
                    self.overlay_detail = format!("{summary} : {detail}");
                    if let Some(overlay) = &self.overlay {
                        overlay.window.request_redraw();
                    }
                }
            }
            Feedback::Done { .. } => {}
            Feedback::Failed { summary, error, .. } => {
                self.notice(format!("{summary} : {error}"), true)
            }
            Feedback::Internal { op, message } => match op {
                InternalOp::TogglePause => self.toggle_pause(),
                InternalOp::Lock => {
                    if let Some(pipeline) = &self.pipeline {
                        pipeline.send(Command::Lock);
                    }
                }
                InternalOp::Notify => self.notice(message.unwrap_or_default(), false),
            },
        }
        if let Some(preview) = &self.preview {
            preview.window.request_redraw();
        }
    }

    fn on_menu(&mut self, id: &MenuId, event_loop: &ActiveEventLoop) {
        let Some(tray) = &self.tray else { return };
        if *id == tray.pause {
            self.toggle_pause();
        } else if *id == tray.preview || *id == tray.record {
            if *id == tray.record {
                self.tab = Tab::Gestures;
            }
            self.open_preview(event_loop);
        } else if *id == tray.reload {
            self.reload_config(true);
        } else if *id == tray.config_dir {
            self.ui_action(UiAction::OpenConfigFolder);
        } else if *id == tray.logs {
            self.ui_action(UiAction::OpenLogs);
        } else if *id == tray.quit {
            // Arrêter le pipeline libère la caméra avant de quitter (ENF-24).
            self.pipeline = None;
            self.preview = None;
            self.overlay = None;
            self.tray = None;
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.initialized {
            self.initialized = true;
            self.init(event_loop);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(event) => self.on_menu(&event.id, event_loop),
            UserEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.open_preview(event_loop),
            UserEvent::Tray(_) => {}
            UserEvent::Hotkey(event) => {
                // EF-35 : pause / reprise, en toutes circonstances.
                if event.state == HotKeyState::Pressed
                    && self.hotkey.is_some_and(|h| h.id() == event.id)
                {
                    self.toggle_pause();
                }
            }
            UserEvent::Feedback(feedback) => self.on_feedback(feedback),
            UserEvent::Wake => self.drain_pipeline(),
            // Les éditeurs écrivent en plusieurs fois : on attend que le fichier se stabilise.
            UserEvent::ConfigChanged => {
                self.reload_at = Some(Instant::now() + Duration::from_millis(250))
            }
            UserEvent::ModelsChanged => {
                self.restart_at = Some(Instant::now() + Duration::from_millis(800))
            }
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.preview.as_ref().is_some_and(|p| p.window.id() == id) {
            match event {
                WindowEvent::CloseRequested => self.close_preview(),
                WindowEvent::RedrawRequested => self.paint_preview(),
                event => {
                    if let Some(preview) = &mut self.preview
                        && preview.window.on_event(&event)
                    {
                        preview.window.request_redraw();
                    }
                }
            }
        } else if self.overlay.as_ref().is_some_and(|o| o.window.id() == id) {
            match event {
                WindowEvent::RedrawRequested => self.paint_overlay(),
                event => {
                    if let Some(overlay) = &mut self.overlay {
                        overlay.window.on_event(&event);
                    }
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.reload_at.is_some_and(|t| now >= t) {
            self.reload_at = None;
            self.reload_config(false);
        }
        if self.restart_at.is_some_and(|t| now >= t) {
            self.restart_at = None;
            // EF-65 : un classifieur .onnx a été remplacé.
            self.notice("Modèles de classification mis à jour : rechargement", false);
            self.start_pipeline();
            self.refresh_tray();
        }
        if self.overlay_hide_at.is_some_and(|t| now >= t) {
            self.hide_overlay();
        }
        if self.overlay_wanted && self.overlay.is_none() {
            self.create_overlay(event_loop);
            if let Some(overlay) = &mut self.overlay {
                overlay.visible = true;
                platform::show_no_activate(overlay.window.window(), true);
                overlay.window.request_redraw();
            } else {
                // Création impossible : inutile de réessayer à chaque tour.
                self.overlay_wanted = false;
            }
        }
        if !self.overlay_wanted && self.overlay_destroy_at.is_some_and(|t| now >= t) {
            self.overlay_destroy_at = None;
            self.overlay = None;
        }
        let next = [
            self.reload_at,
            self.restart_at,
            self.overlay_hide_at,
            self.overlay_destroy_at,
        ]
        .into_iter()
        .flatten()
        .min();
        event_loop.set_control_flow(match next {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }
}
