//! Création des sessions ONNX Runtime de la vision (D-06, D-07) : DirectML par
//! défaut, repli automatique sur CPU.

use std::path::Path;

use ort::ep::DirectML;
use ort::session::Session;

use crate::VisionError;
use crate::landmarks::LandmarkModel;
use crate::palm::PalmDetector;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    DirectMl,
    Cpu,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::DirectMl => "DirectML",
            Provider::Cpu => "CPU",
        }
    }
}

fn try_build(path: &Path, provider: Provider) -> ort::Result<Session> {
    // Pas d'attente active des threads d'inférence : le coût CPU au repos compte (ENF-04).
    let mut builder = Session::builder()?.with_intra_op_spinning(false)?;
    builder = match provider {
        // DirectML exige une exécution séquentielle sans motif mémoire.
        Provider::DirectMl => builder
            .with_memory_pattern(false)?
            .with_parallel_execution(false)?
            .with_execution_providers([DirectML::default().build().error_on_failure()])?,
        Provider::Cpu => builder.with_intra_threads(2)?,
    };
    builder.commit_from_file(path)
}

/// Charge un modèle avec le fournisseur demandé ; en cas d'échec de DirectML,
/// se replie sur CPU et le signale dans le journal.
pub fn build_session(path: &Path, requested: Provider) -> Result<(Session, Provider), VisionError> {
    let model_err = |e: ort::Error| VisionError::Model {
        path: path.display().to_string(),
        message: e.to_string(),
    };
    if !path.is_file() {
        return Err(VisionError::Model {
            path: path.display().to_string(),
            message: "fichier introuvable — lance scripts/fetch-models.ps1".into(),
        });
    }
    if requested == Provider::DirectMl {
        match try_build(path, Provider::DirectMl) {
            Ok(session) => return Ok((session, Provider::DirectMl)),
            Err(e) => {
                tracing::warn!(modele = %path.display(), erreur = %e, "DirectML indisponible, repli sur CPU");
            }
        }
    }
    try_build(path, Provider::Cpu)
        .map(|s| (s, Provider::Cpu))
        .map_err(model_err)
}

/// Les deux modèles de vision et le fournisseur réellement utilisé.
pub struct VisionSessions {
    pub palm: PalmDetector,
    pub landmarks: LandmarkModel,
    pub provider: Provider,
}

impl VisionSessions {
    pub fn load(palm: &Path, landmarks: &Path, requested: Provider) -> Result<Self, VisionError> {
        let (palm_session, p1) = build_session(palm, requested)?;
        // Les deux modèles tournent sur le même fournisseur.
        let (landmark_session, p2) = build_session(landmarks, p1)?;
        let (palm_session, provider) = if p1 != p2 {
            (build_session(palm, Provider::Cpu)?.0, Provider::Cpu)
        } else {
            (palm_session, p1)
        };
        Ok(Self {
            palm: PalmDetector::new(palm_session, palm)?,
            landmarks: LandmarkModel::new(landmark_session, landmarks)?,
            provider,
        })
    }
}
