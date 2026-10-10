//! Schéma TOML, chargement et validation de la configuration de Mudra (§7.9, D-15).
//!
//! Le fichier vit dans `%APPDATA%\Mudra\config.toml`. `config/default.toml` y est
//! copié au premier lancement. La surveillance du fichier (rechargement à chaud)
//! est faite par l'application ; ce crate reste sans entrées/sorties autres que
//! la lecture du fichier.

pub mod keys;
mod schema;
mod validate;

use std::path::{Path, PathBuf};

pub use schema::*;
pub use validate::{Issue, KnownGestures, Level, validate};

/// Contenu de `config/default.toml`, embarqué dans le binaire.
pub mod edit;

pub const DEFAULT_CONFIG_TOML: &str = include_str!("../../../config/default.toml");

pub const APP_NAME: &str = "Mudra";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: lecture impossible ({source})")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}:{line}:{column}: {message}")]
    Parse {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("{path}: configuration invalide\n{}", format_issues(.issues))]
    Invalid { path: PathBuf, issues: Vec<Issue> },
    #[error("variable d'environnement {0} absente")]
    MissingEnv(&'static str),
}

fn format_issues(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(|i| format!("  - {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `%APPDATA%\Mudra`.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    std::env::var_os("APPDATA")
        .map(|p| PathBuf::from(p).join(APP_NAME))
        .ok_or(ConfigError::MissingEnv("APPDATA"))
}

/// `%APPDATA%\Mudra\config.toml`.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

/// `%LOCALAPPDATA%\Mudra\logs` (D-18).
pub fn logs_dir() -> Result<PathBuf, ConfigError> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join(APP_NAME).join("logs"))
        .ok_or(ConfigError::MissingEnv("LOCALAPPDATA"))
}

/// Copie la configuration par défaut si le fichier n'existe pas encore.
/// Renvoie `true` si le fichier vient d'être créé.
pub fn ensure_default(path: &Path) -> Result<bool, ConfigError> {
    if path.exists() {
        return Ok(false);
    }
    let io = |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TOML).map_err(io)?;
    Ok(true)
}

/// Analyse un texte TOML. `path` ne sert qu'aux messages d'erreur.
pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
    toml::from_str::<Config>(text).map_err(|e| {
        let (line, column) = e
            .span()
            .map(|span| {
                let before = &text[..span.start.min(text.len())];
                let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
                let column = before
                    .rfind('\n')
                    .map_or(before.len(), |i| before.len() - i - 1)
                    + 1;
                (line, column)
            })
            .unwrap_or((0, 0));
        ConfigError::Parse {
            path: path.to_path_buf(),
            line,
            column,
            message: e.message().to_string(),
        }
    })
}

/// Lit et analyse le fichier, sans validation sémantique.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse(&text, path)
}

/// Lit, analyse et valide. Les avertissements sont renvoyés à côté de la
/// configuration ; la première erreur bloquante produit `ConfigError::Invalid`.
pub fn load_validated(
    path: &Path,
    known: &KnownGestures,
) -> Result<(Config, Vec<Issue>), ConfigError> {
    let config = load(path)?;
    let issues = validate(&config, known);
    if issues.iter().any(|i| i.level == Level::Error) {
        return Err(ConfigError::Invalid {
            path: path.to_path_buf(),
            issues,
        });
    }
    Ok((config, issues))
}

/// Développe `~` (profil utilisateur) et les variables `%NOM%`.
pub fn expand_path(raw: &str) -> PathBuf {
    let mut s = raw.to_string();
    if let Some(rest) = s.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
        && let Some(home) = std::env::var_os("USERPROFILE")
    {
        s = format!("{}{}", home.to_string_lossy(), rest);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) if !name.is_empty() => out.push_str(&value),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    PathBuf::from(out.replace('/', "\\"))
}

/// Résout un chemin de modèle relatif : dossier de configuration, puis dossier
/// de l'exécutable et ses parents (dépôt en développement), puis dossier courant.
pub fn resolve_model_path(raw: &str) -> PathBuf {
    let expanded = expand_path(raw);
    if expanded.is_absolute() {
        return expanded;
    }
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = config_dir() {
        bases.push(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        bases.extend(exe.ancestors().skip(1).take(4).map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        bases.push(cwd);
    }
    bases
        .iter()
        .map(|b| b.join(&expanded))
        .find(|p| p.exists())
        .unwrap_or(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_and_validates() {
        let config = parse(DEFAULT_CONFIG_TOML, Path::new("default.toml")).unwrap();
        assert_eq!(config.camera.width, 1280);
        assert_eq!(config.camera.fps, 60);
        assert_eq!(config.activation.wake_gesture, "open_palm");
        assert_eq!(config.bindings.len(), 10);
        assert_eq!(config.continuous.len(), 2);
        let issues = validate(&config, &KnownGestures::builtin());
        assert!(issues.iter().all(|i| i.level != Level::Error), "{issues:?}");
    }

    #[test]
    fn empty_file_gives_defaults() {
        let config = parse("", Path::new("x.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.defaults.static_enter, 0.85);
        assert_eq!(config.inference.max_hands, 1);
    }

    #[test]
    fn parse_error_reports_line() {
        let text = "[camera]\nwidth = 1280\nfps = \"soixante\"\n";
        match parse(text, Path::new("c.toml")) {
            Err(ConfigError::Parse { line, .. }) => assert_eq!(line, 3),
            other => panic!("attendu une erreur d'analyse, obtenu {other:?}"),
        }
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(parse("[camera]\nwidht = 3\n", Path::new("c.toml")).is_err());
    }

    #[test]
    fn device_accepts_name_or_index() {
        let c = parse("[camera]\ndevice = 1\n", Path::new("c.toml")).unwrap();
        assert_eq!(c.camera.device, DeviceSelector::Index(1));
        let c = parse(
            "[camera]\ndevice = \"Cam\"\nexposure = -6\n",
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(c.camera.device, DeviceSelector::Name("Cam".into()));
        assert_eq!(c.camera.exposure, Exposure::Manual(-6));
    }

    #[test]
    fn expands_home_and_variables() {
        // SAFETY: test mono-thread sur des variables propres à ce test.
        unsafe {
            std::env::set_var("USERPROFILE", r"C:\Users\test");
            std::env::set_var("MUDRA_TEST_VAR", "valeur");
        }
        assert_eq!(
            expand_path("~/Pictures/Mudra"),
            PathBuf::from(r"C:\Users\test\Pictures\Mudra")
        );
        assert_eq!(
            expand_path(r"C:\%MUDRA_TEST_VAR%\x"),
            PathBuf::from(r"C:\valeur\x")
        );
        assert_eq!(expand_path("100%"), PathBuf::from("100%"));
    }
}
