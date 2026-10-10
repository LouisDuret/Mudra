//! Media Foundation via le crate `windows` (D-02) : `IMFSourceReader` synchrone,
//! attributs `MF_LOW_LATENCY` et `MF_READWRITE_DISABLE_CONVERTERS`, sélection du
//! type natif exact.

use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::DirectShow::{
    CameraControl_Exposure, CameraControl_Flags_Auto, CameraControl_Flags_Manual, IAMCameraControl,
};
use windows::Win32::Media::KernelStreaming::{
    IKsControl, KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY,
    KSPROPERTY_CAMERACONTROL_FLAGS_MANUAL, KSPROPERTY_CAMERACONTROL_S, KSPROPERTY_TYPE_SET,
    PROPSETID_VIDCAP_CAMERACONTROL,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Threading::{
    AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority,
    THREAD_PRIORITY_ABOVE_NORMAL,
};
use windows::core::{GUID, Interface, PWSTR, w};

use crate::{
    CaptureConfig, CaptureError, DeviceInfo, Frame, FrameSource, NativeFormat, PixelFormat, now_us,
};

const VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn platform(context: &'static str) -> impl Fn(windows::core::Error) -> CaptureError {
    move |source| CaptureError::Platform { context, source }
}

/// Initialise COM (MTA) et Media Foundation pour le thread courant ; tout est
/// libéré à la destruction.
struct MfRuntime {
    com: bool,
}

impl MfRuntime {
    fn start() -> Result<Self, CaptureError> {
        // SAFETY: appels d'initialisation appariés avec ceux de `Drop`.
        unsafe {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(platform("MFStartup"))?;
            Ok(Self { com })
        }
    }
}

impl Drop for MfRuntime {
    fn drop(&mut self) {
        // SAFETY: apparié avec `start`.
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

fn subtype_name(guid: &GUID) -> (String, Option<PixelFormat>) {
    if *guid == MFVideoFormat_MJPG {
        ("MJPG".into(), Some(PixelFormat::Mjpeg))
    } else if *guid == MFVideoFormat_NV12 {
        ("NV12".into(), Some(PixelFormat::Nv12))
    } else if *guid == MFVideoFormat_YUY2 {
        ("YUY2".into(), Some(PixelFormat::Yuy2))
    } else {
        // Les sous-types vidéo portent leur FourCC dans le premier champ du GUID.
        let fourcc = guid.data1.to_le_bytes();
        let name = if fourcc.iter().all(|b| b.is_ascii_graphic()) {
            String::from_utf8_lossy(&fourcc).into_owned()
        } else {
            format!("{:08X}", guid.data1)
        };
        (name, None)
    }
}

fn describe_type(media_type: &IMFMediaType) -> Option<NativeFormat> {
    // SAFETY: lectures d'attributs sur un type de média valide.
    unsafe {
        let subtype = media_type.GetGUID(&MF_MT_SUBTYPE).ok()?;
        let size = media_type.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
        let rate = media_type.GetUINT64(&MF_MT_FRAME_RATE).ok()?;
        let (num, den) = ((rate >> 32) as u32, rate as u32);
        let (name, pixel_format) = subtype_name(&subtype);
        Some(NativeFormat {
            subtype: name,
            pixel_format,
            width: (size >> 32) as u32,
            height: size as u32,
            fps: if den == 0 {
                0.0
            } else {
                num as f32 / den as f32
            },
        })
    }
}

fn enumerate_activates() -> Result<Vec<(String, IMFActivate)>, CaptureError> {
    // SAFETY: tableaux alloués par Media Foundation, libérés par CoTaskMemFree
    // après avoir repris possession de chaque élément.
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).map_err(platform("MFCreateAttributes"))?;
        let attributes = attributes
            .ok_or_else(|| CaptureError::Other("attributs Media Foundation nuls".into()))?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(platform("SetGUID"))?;
        let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attributes, &mut array, &mut count)
            .map_err(platform("MFEnumDeviceSources"))?;
        let mut devices = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let Some(activate) = array.add(i).read() else {
                continue;
            };
            let mut name = PWSTR::null();
            let mut len = 0u32;
            let label = if activate
                .GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len)
                .is_ok()
            {
                let s = name.to_string().unwrap_or_default();
                CoTaskMemFree(Some(name.as_ptr() as *const c_void));
                s
            } else {
                format!("caméra {i}")
            };
            devices.push((label, activate));
        }
        CoTaskMemFree(Some(array as *const c_void));
        Ok(devices)
    }
}

fn create_reader(source: &IMFMediaSource) -> Result<IMFSourceReader, CaptureError> {
    // SAFETY: création d'objets COM, erreurs propagées.
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 2).map_err(platform("MFCreateAttributes"))?;
        let attributes = attributes
            .ok_or_else(|| CaptureError::Other("attributs Media Foundation nuls".into()))?;
        attributes
            .SetUINT32(&MF_LOW_LATENCY, 1)
            .map_err(platform("MF_LOW_LATENCY"))?;
        attributes
            .SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)
            .map_err(platform("MF_READWRITE_DISABLE_CONVERTERS"))?;
        MFCreateSourceReaderFromMediaSource(source, &attributes)
            .map_err(platform("MFCreateSourceReaderFromMediaSource"))
    }
}

fn native_types(reader: &IMFSourceReader) -> Vec<(IMFMediaType, NativeFormat)> {
    let mut types = Vec::new();
    for index in 0.. {
        // SAFETY: l'énumération s'arrête à la première erreur (MF_E_NO_MORE_TYPES).
        let Ok(media_type) = (unsafe { reader.GetNativeMediaType(VIDEO_STREAM, index) }) else {
            break;
        };
        if let Some(format) = describe_type(&media_type) {
            types.push((media_type, format));
        }
    }
    types
}

/// Noms des caméras du PC, dans l'ordre de leur index. Ne les ouvre pas :
/// utilisable pendant que l'une d'elles est en service.
pub fn list_device_names() -> Result<Vec<String>, CaptureError> {
    let _runtime = MfRuntime::start()?;
    Ok(enumerate_activates()?
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// Liste les caméras et leurs formats natifs (EF-01, `mudra devices`).
pub fn list_devices() -> Result<Vec<DeviceInfo>, CaptureError> {
    let _runtime = MfRuntime::start()?;
    let mut devices = Vec::new();
    for (index, (name, activate)) in enumerate_activates()?.into_iter().enumerate() {
        let mut formats = Vec::new();
        // SAFETY: la source est arrêtée avant d'être relâchée.
        unsafe {
            if let Ok(source) = activate.ActivateObject::<IMFMediaSource>() {
                if let Ok(reader) = create_reader(&source) {
                    formats = native_types(&reader).into_iter().map(|(_, f)| f).collect();
                }
                let _ = source.Shutdown();
            }
            let _ = activate.ShutdownObject();
        }
        formats.dedup();
        devices.push(DeviceInfo {
            index: index as u32,
            name,
            formats,
        });
    }
    Ok(devices)
}

/// Choisit le format natif le plus proche de la demande (EF-02). Renvoie
/// l'indice retenu et `true` si la correspondance est exacte.
pub(crate) fn choose_format(
    formats: &[NativeFormat],
    config: &CaptureConfig,
) -> Option<(usize, bool)> {
    let wanted_pixels = i64::from(config.width) * i64::from(config.height);
    let score = |f: &NativeFormat| {
        let pixel_format = f.pixel_format?;
        let pixels = i64::from(f.width) * i64::from(f.height);
        Some((
            // Dans l'ordre : même sous-type, résolution la plus proche, fréquence la plus proche.
            u8::from(pixel_format != config.format),
            (pixels - wanted_pixels).abs() + i64::from(f.width.abs_diff(config.width)),
            (f.fps - config.fps as f32).abs(),
        ))
    };
    let (index, best) = formats
        .iter()
        .enumerate()
        .filter_map(|(i, f)| score(f).map(|s| (i, s)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    Some((index, best.0 == 0 && best.1 == 0 && best.2 < 0.5))
}

pub struct MfSource {
    // L'ordre des champs fixe l'ordre de destruction : lecteur, source, puis runtime.
    reader: IMFSourceReader,
    source: IMFMediaSource,
    name: String,
    format: NativeFormat,
    pixel_format: PixelFormat,
    exact: bool,
    /// Caméra demandée mais introuvable, remplacée par celle-ci.
    instead_of: Option<String>,
    seq: u64,
    _runtime: MfRuntime,
}

impl MfSource {
    /// Ouvre la caméra configurée dans le format demandé ou, à défaut, dans le
    /// format natif le plus proche (EF-02). En mode automatique, ou si la
    /// caméra configurée n'existe pas sur ce PC, la première caméra qui
    /// s'ouvre est retenue.
    pub fn open(config: &CaptureConfig) -> Result<Self, CaptureError> {
        let runtime = MfRuntime::start()?;
        let devices = enumerate_activates()?;
        if devices.is_empty() {
            return Err(CaptureError::NoDevice);
        }
        let names: Vec<String> = devices.iter().map(|(name, _)| name.clone()).collect();
        let (order, fallback) = config.device.candidates(&names);
        let mut first_error = None;
        for index in order {
            let (name, activate) = &devices[index];
            match Self::open_device(name, activate, config) {
                Ok((reader, source, format, exact)) => {
                    let instead_of = fallback.then(|| config.device.label());
                    if let Some(wanted) = &instead_of {
                        tracing::warn!(
                            demandee = %wanted,
                            utilisee = %name,
                            "caméra configurée introuvable, repli sur une autre"
                        );
                    }
                    let this = Self {
                        reader,
                        source,
                        name: name.clone(),
                        pixel_format: format.pixel_format.unwrap_or_default(),
                        format,
                        exact,
                        instead_of,
                        seq: 0,
                        _runtime: runtime,
                    };
                    this.apply_camera_settings(config);
                    return Ok(this);
                }
                Err(e) => {
                    tracing::debug!(camera = %name, erreur = %e, "caméra écartée");
                    first_error.get_or_insert(e);
                }
            }
        }
        Err(first_error.unwrap_or(CaptureError::NoDevice))
    }

    fn open_device(
        name: &str,
        activate: &IMFActivate,
        config: &CaptureConfig,
    ) -> Result<(IMFSourceReader, IMFMediaSource, NativeFormat, bool), CaptureError> {
        // SAFETY: objets COM valides ; la source est arrêtée dans `Drop`.
        let source = unsafe { activate.ActivateObject::<IMFMediaSource>() }
            .map_err(platform("ouverture de la caméra"))?;
        let opened = (|| {
            let reader = create_reader(&source)?;
            let types = native_types(&reader);
            let formats: Vec<NativeFormat> = types.iter().map(|(_, f)| f.clone()).collect();
            let (index, exact) = choose_format(&formats, config)
                .ok_or_else(|| CaptureError::NoUsableFormat(name.to_string()))?;
            let (media_type, format) = &types[index];
            // SAFETY: type natif issu de ce lecteur.
            unsafe { reader.SetCurrentMediaType(VIDEO_STREAM, None, media_type) }
                .map_err(platform("SetCurrentMediaType"))?;
            Ok::<_, CaptureError>((reader, format.clone(), exact))
        })();
        match opened {
            Ok((reader, format, exact)) => Ok((reader, source, format, exact)),
            Err(e) => {
                // SAFETY: libère la caméra si l'ouverture échoue en cours de route.
                unsafe {
                    let _ = source.Shutdown();
                    let _ = activate.ShutdownObject();
                }
                Err(e)
            }
        }
    }

    /// Vrai si le format ouvert est exactement celui demandé.
    pub fn is_exact(&self) -> bool {
        self.exact
    }

    /// Réglages caméra (EF-04), au mieux : un pilote qui refuse un réglage ne
    /// doit pas empêcher la capture.
    fn apply_camera_settings(&self, config: &CaptureConfig) {
        // SAFETY: interfaces obtenues par QueryInterface sur la source ouverte.
        unsafe {
            match self.source.cast::<IAMCameraControl>() {
                Ok(control) => {
                    let result = match config.exposure {
                        None => {
                            control.Set(CameraControl_Exposure.0, 0, CameraControl_Flags_Auto.0)
                        }
                        Some(value) => control.Set(
                            CameraControl_Exposure.0,
                            value,
                            CameraControl_Flags_Manual.0,
                        ),
                    };
                    if let Err(e) = result {
                        tracing::warn!(erreur = %e, "réglage de l'exposition refusé par la caméra");
                    }
                }
                Err(e) => tracing::debug!(erreur = %e, "IAMCameraControl indisponible"),
            }
            match self.source.cast::<IKsControl>() {
                Ok(ks) => {
                    let mut property = KSPROPERTY_CAMERACONTROL_S::default();
                    property.Property.Anonymous.Anonymous.Set = PROPSETID_VIDCAP_CAMERACONTROL;
                    property.Property.Anonymous.Anonymous.Id =
                        KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY.0 as u32;
                    property.Property.Anonymous.Anonymous.Flags = KSPROPERTY_TYPE_SET;
                    property.Value = i32::from(config.auto_exposure_priority);
                    property.Flags = KSPROPERTY_CAMERACONTROL_FLAGS_MANUAL as u32;
                    let size = size_of::<KSPROPERTY_CAMERACONTROL_S>() as u32;
                    let mut returned = 0u32;
                    let data = &mut property as *mut KSPROPERTY_CAMERACONTROL_S;
                    if let Err(e) =
                        ks.KsProperty(&(*data).Property, size, data.cast(), size, &mut returned)
                    {
                        tracing::debug!(erreur = %e, "priorité d'exposition automatique non réglable");
                    }
                }
                Err(e) => tracing::debug!(erreur = %e, "IKsControl indisponible"),
            }
        }
    }
}

impl FrameSource for MfSource {
    fn next_frame(&mut self, frame: &mut Frame) -> Result<(), CaptureError> {
        loop {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample: Option<IMFSample> = None;
            // SAFETY: lecture synchrone ; tous les pointeurs de sortie sont valides.
            unsafe {
                self.reader
                    .ReadSample(
                        VIDEO_STREAM,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|_| CaptureError::Disconnected)?;
            }
            let received_us = now_us();
            if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
                return Err(CaptureError::Disconnected);
            }
            // Pas d'échantillon (trou dans le flux) : on attend la trame suivante.
            let Some(sample) = sample else { continue };
            // SAFETY: le tampon est verrouillé le temps de la copie, puis déverrouillé.
            unsafe {
                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(platform("ConvertToContiguousBuffer"))?;
                let mut data = std::ptr::null_mut();
                let mut len = 0u32;
                buffer
                    .Lock(&mut data, None, Some(&mut len))
                    .map_err(platform("IMFMediaBuffer::Lock"))?;
                frame.data.clear();
                frame
                    .data
                    .extend_from_slice(std::slice::from_raw_parts(data, len as usize));
                let _ = buffer.Unlock();
            }
            self.seq += 1;
            frame.format = self.pixel_format;
            frame.width = self.format.width;
            frame.height = self.format.height;
            frame.timestamp_us = received_us;
            frame.device_timestamp = timestamp;
            frame.seq = self.seq;
            return Ok(());
        }
    }

    fn description(&self) -> (String, NativeFormat) {
        (self.name.clone(), self.format.clone())
    }

    fn notice(&self) -> Option<String> {
        self.instead_of.as_ref().map(|wanted| {
            format!(
                "Caméra « {wanted} » introuvable : « {} » est utilisée à la place",
                self.name
            )
        })
    }
}

impl Drop for MfSource {
    fn drop(&mut self) {
        // Arrêter la source libère la caméra : le voyant s'éteint (EF-06, ENF-24).
        // SAFETY: la source est valide jusqu'à la fin de `drop`.
        unsafe {
            let _ = self.source.Shutdown();
        }
    }
}

/// Élève la priorité du thread courant : MMCSS « Capture » si disponible,
/// sinon priorité au-dessus de la normale (§3.2).
pub fn promote_current_thread() {
    // SAFETY: agit uniquement sur le thread appelant.
    unsafe {
        let mut task_index = 0u32;
        let mmcss: windows::core::Result<HANDLE> =
            AvSetMmThreadCharacteristicsW(w!("Capture"), &mut task_index);
        if mmcss.is_err() {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceSelector;

    fn format(
        subtype: &str,
        pf: Option<PixelFormat>,
        width: u32,
        height: u32,
        fps: f32,
    ) -> NativeFormat {
        NativeFormat {
            subtype: subtype.into(),
            pixel_format: pf,
            width,
            height,
            fps,
        }
    }

    fn config(format: PixelFormat, width: u32, height: u32, fps: u32) -> CaptureConfig {
        CaptureConfig {
            device: DeviceSelector::Index(0),
            format,
            width,
            height,
            fps,
            exposure: None,
            auto_exposure_priority: false,
        }
    }

    fn streamcam() -> Vec<NativeFormat> {
        let mut formats = Vec::new();
        for (w, h) in [
            (1920, 1080),
            (1280, 720),
            (960, 540),
            (848, 480),
            (640, 360),
            (320, 240),
        ] {
            formats.push(format("MJPG", Some(PixelFormat::Mjpeg), w, h, 60.0));
            formats.push(format("MJPG", Some(PixelFormat::Mjpeg), w, h, 30.0));
            formats.push(format("NV12", Some(PixelFormat::Nv12), w, h, 30.0));
            formats.push(format("YUY2", Some(PixelFormat::Yuy2), w, h, 30.0));
        }
        formats.push(format("H264", None, 1920, 1080, 30.0));
        formats
    }

    #[test]
    fn exact_match_is_found() {
        let formats = streamcam();
        let (i, exact) =
            choose_format(&formats, &config(PixelFormat::Mjpeg, 1280, 720, 60)).unwrap();
        assert!(exact);
        assert_eq!(
            formats[i],
            format("MJPG", Some(PixelFormat::Mjpeg), 1280, 720, 60.0)
        );
    }

    #[test]
    fn falls_back_to_closest_resolution_and_rate() {
        let formats = streamcam();
        // 1024×576 n'existe pas : 960×540 est le plus proche.
        let (i, exact) =
            choose_format(&formats, &config(PixelFormat::Mjpeg, 1024, 576, 60)).unwrap();
        assert!(!exact);
        assert_eq!(
            (formats[i].width, formats[i].height, formats[i].fps),
            (960, 540, 60.0)
        );
        // NV12 à 60 fps n'existe pas : même sous-type, 30 fps.
        let (i, exact) = choose_format(&formats, &config(PixelFormat::Nv12, 640, 360, 60)).unwrap();
        assert!(!exact);
        assert_eq!(
            (
                formats[i].subtype.as_str(),
                formats[i].width,
                formats[i].fps
            ),
            ("NV12", 640, 30.0)
        );
    }

    #[test]
    fn other_subtype_when_requested_one_is_missing() {
        let formats = vec![
            format("YUY2", Some(PixelFormat::Yuy2), 640, 480, 30.0),
            format("H264", None, 1280, 720, 60.0),
        ];
        let (i, exact) =
            choose_format(&formats, &config(PixelFormat::Mjpeg, 1280, 720, 60)).unwrap();
        assert!(!exact);
        assert_eq!(formats[i].subtype, "YUY2");
        assert!(choose_format(&formats[1..], &config(PixelFormat::Mjpeg, 1280, 720, 60)).is_none());
    }

    #[test]
    fn fourcc_names() {
        assert_eq!(
            subtype_name(&MFVideoFormat_MJPG),
            ("MJPG".to_string(), Some(PixelFormat::Mjpeg))
        );
        assert_eq!(subtype_name(&MFVideoFormat_H264).0, "H264");
    }
}
