//! Format du dataset de Mudra (EF-62) : un fichier JSON Lines par session,
//! `dataset/<static|dynamic>/<label>/<session>.jsonl`.
//!
//! Première ligne : métadonnées de la session. Lignes suivantes : une trame par
//! ligne (horodatage, piste, latéralité, présence, 21×3 points image, 21×3
//! points monde). Aucune image n'est stockée (ENF-21).

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Version du format de fichier. À incrémenter à tout changement incompatible.
pub const FORMAT_VERSION: u32 = 1;

pub type Landmarks = [[f32; 3]; 21];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Static,
    Dynamic,
}

impl SessionKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            SessionKind::Static => "static",
            SessionKind::Dynamic => "dynamic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hand {
    Left,
    Right,
}

/// Première ligne du fichier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub format_version: u32,
    pub kind: SessionKind,
    pub label: String,
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub mirror: bool,
    /// Date de création, UTC, ISO 8601.
    pub created_at: String,
    /// Mode dynamique : nombre de clips et durée de chacun, en secondes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_seconds: Option<f32>,
}

/// Une main sur une trame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameRecord {
    /// Horodatage en microsecondes (horloge monotone de la session).
    pub t: u64,
    pub track: u32,
    pub hand: Hand,
    pub hand_score: f32,
    pub presence: f32,
    /// Points image normalisés : x / largeur, y / hauteur, z / largeur (image affichée).
    pub image: Landmarks,
    /// Points monde, en mètres.
    pub world: Landmarks,
    /// Mode dynamique : numéro de la répétition à laquelle appartient la trame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}:{line}: {message}")]
    Format {
        path: PathBuf,
        line: usize,
        message: String,
    },
    #[error("label « {0} » invalide : attendu des minuscules, chiffres et tirets bas (snake_case)")]
    InvalidLabel(String),
}

/// Les identifiants de gestes sont en snake_case (Annexe B) : ils servent de
/// noms de dossier et de labels de modèle.
pub fn validate_label(label: &str) -> Result<(), DatasetError> {
    let ok = !label.is_empty()
        && label.len() <= 48
        && label.starts_with(|c: char| c.is_ascii_lowercase())
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(DatasetError::InvalidLabel(label.to_string()))
    }
}

/// Jour civil (année, mois, jour) depuis le nombre de jours écoulés depuis 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Horodatage UTC : (`2026-10-09T20:15:30Z`, `20261009T201530Z`).
pub fn utc_timestamp(time: SystemTime) -> (String, String) {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (s / 3600, s % 3600 / 60, s % 60);
    (
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"),
        format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z"),
    )
}

/// Écrit une session, une ligne JSON par trame.
pub struct SessionWriter {
    path: PathBuf,
    out: BufWriter<File>,
    frames: u64,
}

impl SessionWriter {
    /// Crée `root/<static|dynamic>/<label>/<session>.jsonl` et écrit l'en-tête.
    /// `header.created_at` et `header.format_version` sont renseignés ici.
    pub fn create(root: &Path, mut header: SessionHeader) -> Result<Self, DatasetError> {
        validate_label(&header.label)?;
        let (iso, compact) = utc_timestamp(SystemTime::now());
        header.created_at = iso;
        header.format_version = FORMAT_VERSION;
        let dir = root.join(header.kind.dir_name()).join(&header.label);
        std::fs::create_dir_all(&dir).map_err(|source| DatasetError::Io {
            path: dir.clone(),
            source,
        })?;
        let mut path = dir.join(format!("{compact}.jsonl"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = dir.join(format!("{compact}-{n}.jsonl"));
        }
        let file = File::create(&path).map_err(|source| DatasetError::Io {
            path: path.clone(),
            source,
        })?;
        let mut writer = Self {
            path,
            out: BufWriter::new(file),
            frames: 0,
        };
        writer.line(&header)?;
        Ok(writer)
    }

    fn line<T: Serialize>(&mut self, value: &T) -> Result<(), DatasetError> {
        let io = |source| DatasetError::Io {
            path: self.path.clone(),
            source,
        };
        serde_json::to_writer(&mut self.out, value).map_err(|e| io(e.into()))?;
        self.out.write_all(b"\n").map_err(io)
    }

    pub fn write(&mut self, record: &FrameRecord) -> Result<(), DatasetError> {
        self.frames += 1;
        self.line(record)
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Vide le tampon et renvoie le chemin du fichier.
    pub fn finish(mut self) -> Result<PathBuf, DatasetError> {
        self.out.flush().map_err(|source| DatasetError::Io {
            path: self.path.clone(),
            source,
        })?;
        Ok(self.path)
    }
}

/// Lit une session complète.
pub fn read_session(path: &Path) -> Result<(SessionHeader, Vec<FrameRecord>), DatasetError> {
    let file = File::open(path).map_err(|source| DatasetError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let format = |line: usize, message: String| DatasetError::Format {
        path: path.to_path_buf(),
        line,
        message,
    };
    let mut header = None;
    let mut frames = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|source| DatasetError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        if header.is_none() {
            let h: SessionHeader =
                serde_json::from_str(&line).map_err(|e| format(i + 1, e.to_string()))?;
            if h.format_version != FORMAT_VERSION {
                return Err(format(
                    i + 1,
                    format!(
                        "format_version {} non pris en charge (attendu {FORMAT_VERSION})",
                        h.format_version
                    ),
                ));
            }
            header = Some(h);
        } else {
            frames.push(serde_json::from_str(&line).map_err(|e| format(i + 1, e.to_string()))?);
        }
    }
    let header =
        header.ok_or_else(|| format(1, "fichier vide : en-tête de session absent".into()))?;
    Ok((header, frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: SessionKind, label: &str) -> SessionHeader {
        SessionHeader {
            format_version: 0,
            kind,
            label: label.into(),
            camera: "Logitech StreamCam".into(),
            width: 1280,
            height: 720,
            fps: 60.0,
            mirror: true,
            created_at: String::new(),
            clips: None,
            clip_seconds: None,
        }
    }

    fn record(t: u64) -> FrameRecord {
        let mut image = [[0.0; 3]; 21];
        image[8] = [0.25, 0.5, -0.01];
        FrameRecord {
            t,
            track: 1,
            hand: Hand::Right,
            hand_score: 0.98,
            presence: 0.99,
            image,
            world: [[0.01; 3]; 21],
            clip: None,
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mudra-dataset-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip() {
        let root = temp_root("roundtrip");
        let mut writer =
            SessionWriter::create(&root, header(SessionKind::Static, "thumbs_up")).unwrap();
        for t in 0..5 {
            writer.write(&record(t * 16_667)).unwrap();
        }
        assert_eq!(writer.frames(), 5);
        let path = writer.finish().unwrap();
        assert!(path.starts_with(root.join("static").join("thumbs_up")));
        assert_eq!(path.extension().unwrap(), "jsonl");

        let (h, frames) = read_session(&path).unwrap();
        assert_eq!(h.format_version, FORMAT_VERSION);
        assert_eq!(h.label, "thumbs_up");
        assert!(h.created_at.ends_with('Z') && h.created_at.len() == 20);
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[4], record(4 * 16_667));
        // Une trame par ligne, plus l'en-tête.
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 6);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn two_sessions_in_the_same_second_do_not_collide() {
        let root = temp_root("collision");
        let a = SessionWriter::create(&root, header(SessionKind::Dynamic, "swipe_left"))
            .unwrap()
            .finish()
            .unwrap();
        let b = SessionWriter::create(&root, header(SessionKind::Dynamic, "swipe_left"))
            .unwrap()
            .finish()
            .unwrap();
        assert_ne!(a, b);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn labels_must_be_snake_case() {
        for ok in ["none", "thumbs_up", "geste_2"] {
            assert!(validate_label(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Thumbs", "pouce levé", "../x", "2fast", "a-b"] {
            assert!(validate_label(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn malformed_files_report_the_line() {
        let root = temp_root("malformed");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("bad.jsonl");
        let good = SessionWriter::create(&root, header(SessionKind::Static, "none"))
            .unwrap()
            .finish()
            .unwrap();
        let head = std::fs::read_to_string(good).unwrap();
        std::fs::write(&path, format!("{head}{{\"t\": 1}}\n")).unwrap();
        match read_session(&path) {
            Err(DatasetError::Format { line, .. }) => assert_eq!(line, 2),
            other => panic!("{other:?}"),
        }
        std::fs::write(&path, "").unwrap();
        assert!(read_session(&path).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn utc_timestamps() {
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_791_576_930);
        assert_eq!(utc_timestamp(t).0, "2026-10-09T20:15:30Z");
        assert_eq!(utc_timestamp(t).1, "20261009T201530Z");
        assert_eq!(utc_timestamp(UNIX_EPOCH).0, "1970-01-01T00:00:00Z");
        let leap = UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800);
        assert_eq!(utc_timestamp(leap).0, "2024-02-29T00:00:00Z");
    }
}
