//! Mudra — piloter son PC par des gestes de la main, via la webcam.
//!
//! Binaire unique : application en zone de notification (`run`, par défaut) et
//! sous-commandes de diagnostic (EF-73). En release, le binaire n'ouvre pas de
//! console ; les sous-commandes s'attachent à la console parente (EF-75).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cli;
mod halo;
mod metrics;
mod pipeline;
mod platform;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mudra_config::Config;

use crate::pipeline::{RecordKind, RecordRequest, Source};

#[derive(Parser)]
#[command(
    name = "mudra",
    version,
    about = "Piloter son PC par des gestes de la main, via la webcam"
)]
struct Cli {
    /// Fichier de configuration (défaut : %APPDATA%\Mudra\config.toml)
    #[arg(long, global = true, value_name = "FICHIER")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Lance l'application en zone de notification (commande par défaut)
    Run {
        /// Ouvre la fenêtre d'aperçu au démarrage
        #[arg(long)]
        preview: bool,
        /// Démarre en pause, caméra libérée
        #[arg(long)]
        paused: bool,
        /// Rejoue une image JPEG fixe à la place de la caméra (démonstration, tests)
        #[arg(long, value_name = "JPEG")]
        source: Option<PathBuf>,
    },
    /// Liste les caméras et leurs formats natifs
    Devices,
    /// Enregistre des points-clés pour un geste (dataset d'entraînement)
    Record {
        /// Identifiant du geste, en snake_case (ex. thumbs_up, swipe_left, none)
        #[arg(long)]
        label: String,
        /// Pose statique : capture continue pendant --duration
        #[arg(long = "static", conflicts_with = "dynamic")]
        static_: bool,
        /// Geste dynamique : --reps répétitions guidées de 1 s
        #[arg(long)]
        dynamic: bool,
        /// Durée de la capture statique
        #[arg(long, default_value = "20s", value_parser = cli::parse_duration)]
        duration: Duration,
        /// Nombre de répétitions en mode dynamique
        #[arg(long, default_value_t = 15)]
        reps: u32,
        /// Dossier racine du dataset
        #[arg(long, value_name = "DOSSIER")]
        dataset: Option<PathBuf>,
        #[arg(long, value_name = "JPEG", hide = true)]
        source: Option<PathBuf>,
    },
    /// Rejoue une session enregistrée dans le moteur de gestes, sans caméra
    Replay {
        session: PathBuf,
        /// Affiche aussi les mises à jour des contrôles continus
        #[arg(long)]
        verbose: bool,
    },
    /// Mesure les performances de bout en bout (B-03) ou par fournisseur (B-01)
    Bench {
        #[arg(long, default_value = "120s", value_parser = cli::parse_duration)]
        duration: Duration,
        /// Compare les fournisseurs d'exécution (CPU, DirectML) au lieu du bout en bout
        #[arg(long)]
        providers: bool,
        /// Mesure le décodage MJPEG seul, sur de vraies trames gardées en mémoire (B-02)
        #[arg(long, conflicts_with = "providers")]
        decode: bool,
        /// Rejoue une image JPEG fixe à la place de la caméra
        #[arg(long, value_name = "JPEG")]
        source: Option<PathBuf>,
    },
    /// Valide le fichier de configuration
    CheckConfig,
    /// Diagnostic : passe une photo dans la vision et le classifieur de poses
    Image {
        path: PathBuf,
        #[arg(long, default_value_t = 6)]
        frames: usize,
        /// Traite la photo comme une vue selfie (retournée)
        #[arg(long)]
        mirror: bool,
        /// Affiche les mesures géométriques du classifieur intégré
        #[arg(long)]
        verbose: bool,
    },
}

fn config_path(cli: &Cli) -> Result<PathBuf> {
    match &cli.config {
        Some(path) => Ok(path.clone()),
        None => Ok(mudra_config::config_path()?),
    }
}

/// Charge la configuration pour une sous-commande ; le fichier par défaut est
/// créé s'il n'existe pas encore (§7.9).
fn load_config(cli: &Cli) -> Result<(PathBuf, Config)> {
    let path = config_path(cli)?;
    if cli.config.is_none() && mudra_config::ensure_default(&path)? {
        println!("Configuration par défaut créée : {}", path.display());
    }
    let config = mudra_config::load(&path)?;
    Ok((path, config))
}

fn source_of(path: Option<PathBuf>) -> Source {
    path.map_or(Source::Camera, Source::Still)
}

fn main() {
    platform::attach_parent_console();
    let cli = Cli::parse();
    let foreground = !matches!(cli.command, None | Some(Cmd::Run { .. }));
    let code = match dispatch(cli) {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!("{e:#}");
            if foreground {
                eprintln!("Erreur : {e:#}");
            } else {
                platform::error_box(&format!("{e:#}"));
            }
            1
        }
    };
    std::process::exit(code);
}

fn dispatch(cli: Cli) -> Result<()> {
    match &cli.command {
        None => {
            let path = config_path(&cli)?;
            app::run(path, cli.config.is_some(), app::RunOptions::default())
        }
        Some(Cmd::Run {
            preview,
            paused,
            source,
        }) => {
            let options = app::RunOptions {
                preview: *preview,
                paused: *paused,
                source: source_of(source.clone()),
            };
            app::run(config_path(&cli)?, cli.config.is_some(), options)
        }
        Some(Cmd::Devices) => {
            let _log = platform::init_logging("warn", true);
            cli::devices()
        }
        Some(Cmd::CheckConfig) => {
            let _log = platform::init_logging("warn", true);
            let path = config_path(&cli)?;
            if cli.config.is_none() {
                mudra_config::ensure_default(&path)?;
            }
            cli::check_config(&path)
        }
        Some(Cmd::Image {
            path,
            frames,
            mirror,
            verbose,
        }) => {
            let (_, config) = load_config(&cli)?;
            let _log = platform::init_logging("warn", true);
            cli::image(&config, path, *frames, *mirror, *verbose)
        }
        Some(Cmd::Bench {
            duration,
            providers,
            decode,
            source,
        }) => {
            let (_, config) = load_config(&cli)?;
            let _log = platform::init_logging("warn", true);
            if *providers {
                cli::bench_providers(&config)
            } else if *decode {
                cli::bench_decode(&config, &source_of(source.clone()))
            } else {
                cli::bench(Arc::new(config), source_of(source.clone()), *duration)
            }
        }
        Some(Cmd::Replay { session, verbose }) => {
            let (_, config) = load_config(&cli)?;
            let _log = platform::init_logging("warn", true);
            cli::replay(&config, session, *verbose)
        }
        Some(Cmd::Record {
            label,
            static_: _,
            dynamic,
            duration,
            reps,
            dataset,
            source,
        }) => {
            let (_, config) = load_config(&cli)?;
            let _log = platform::init_logging(&config.general.log_level, true);
            let kind = if *dynamic {
                RecordKind::Dynamic {
                    reps: (*reps).clamp(1, 500),
                }
            } else {
                RecordKind::Static {
                    seconds: duration.as_secs_f32(),
                }
            };
            let dataset_root = dataset.clone().unwrap_or_else(cli::default_dataset_root);
            std::fs::create_dir_all(&dataset_root)
                .with_context(|| format!("création de {}", dataset_root.display()))?;
            cli::record(
                Arc::new(config),
                source_of(source.clone()),
                RecordRequest {
                    label: label.clone(),
                    kind,
                    dataset_root,
                },
            )
        }
    }
}
