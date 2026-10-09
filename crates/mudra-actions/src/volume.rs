//! Volume du périphérique de rendu par défaut via Core Audio (D-16).

use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};

use crate::ActionError;

/// L'interface est mise en cache et redemandée périodiquement : un changement
/// de périphérique par défaut est ainsi suivi en moins de deux secondes, sans
/// abonnement `IMMNotificationClient`.
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Volume {
    endpoint: Option<(IAudioEndpointVolume, Instant)>,
}

fn audio(e: windows::core::Error) -> ActionError {
    ActionError::Audio(e.message())
}

impl Volume {
    fn acquire() -> windows::core::Result<IAudioEndpointVolume> {
        // SAFETY: COM est initialisé sur le thread des actions.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
        }
    }

    fn with<T>(
        &mut self,
        f: impl Fn(&IAudioEndpointVolume) -> windows::core::Result<T>,
    ) -> Result<T, ActionError> {
        let stale = self
            .endpoint
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() > REFRESH);
        if stale {
            self.endpoint = Some((Self::acquire().map_err(audio)?, Instant::now()));
        }
        let Some((endpoint, _)) = &self.endpoint else {
            return Err(ActionError::Audio("aucun périphérique audio".into()));
        };
        match f(endpoint) {
            Ok(v) => Ok(v),
            Err(_) => {
                // Périphérique retiré : on redemande l'interface et on réessaie une fois.
                let fresh = Self::acquire().map_err(audio)?;
                let result = f(&fresh).map_err(audio);
                self.endpoint = Some((fresh, Instant::now()));
                result
            }
        }
    }

    /// Volume maître dans [0, 1].
    pub fn get(&mut self) -> Result<f32, ActionError> {
        // SAFETY: appel COM sur une interface valide.
        self.with(|e| unsafe { e.GetMasterVolumeLevelScalar() })
    }

    pub fn set(&mut self, value: f32) -> Result<f32, ActionError> {
        let value = value.clamp(0.0, 1.0);
        // SAFETY: appel COM sur une interface valide.
        self.with(|e| unsafe { e.SetMasterVolumeLevelScalar(value, std::ptr::null()) })?;
        Ok(value)
    }

    pub fn step(&mut self, delta: f32) -> Result<f32, ActionError> {
        let current = self.get()?;
        self.set(current + delta)
    }

    /// Bascule le mode muet et renvoie le nouvel état.
    pub fn toggle_mute(&mut self) -> Result<bool, ActionError> {
        // SAFETY: appels COM sur une interface valide.
        self.with(|e| unsafe {
            let muted = !e.GetMute()?.as_bool();
            e.SetMute(muted, std::ptr::null())?;
            Ok(muted)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    /// Test matériel (§10.1) : `cargo test -p mudra-actions -- --ignored`.
    /// Relit le volume et le réécrit à l'identique : sans effet audible.
    #[test]
    #[ignore = "nécessite un périphérique audio"]
    fn reads_and_rewrites_the_master_volume() {
        // SAFETY: initialisation COM du thread de test.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut volume = Volume::default();
        let current = volume.get().expect("lecture du volume");
        assert!((0.0..=1.0).contains(&current));
        let written = volume.set(current).expect("écriture du volume");
        assert!((written - current).abs() < 1e-6);
        assert!((volume.get().unwrap() - current).abs() < 0.011);
    }
}
