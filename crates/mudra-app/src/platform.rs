//! Intégration Windows de l'application : console, journaux, instance unique,
//! lancement à l'ouverture de session, ouverture de dossiers.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    SetStdHandle,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, w};

/// En release, le binaire est une application graphique sans console (EF-75).
/// Lancé depuis un terminal, il s'attache à la console parente pour que les
/// sous-commandes puissent écrire. Sans effet si la sortie est déjà redirigée.
pub fn attach_parent_console() {
    // SAFETY: manipulation des handles standard du processus au démarrage,
    // avant toute écriture.
    unsafe {
        let has_stdout =
            GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
        if has_stdout || AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        let console = CreateFileW(
            w!("CONOUT$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        );
        if let Ok(handle) = console {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
            let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}

fn level_filter(level: &str) -> LevelFilter {
    match level {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        _ => LevelFilter::INFO,
    }
}

/// Journaux fichiers rotatifs dans `%LOCALAPPDATA%\Mudra\logs` (D-18) ; les
/// sous-commandes écrivent aussi les avertissements sur la sortie d'erreur.
/// Le garde renvoyé doit vivre jusqu'à la fin du processus.
pub fn init_logging(level: &str, stderr: bool) -> Option<WorkerGuard> {
    let level = level_filter(level);
    // ONNX Runtime est bavard : on ne garde que ses avertissements.
    let targets = || {
        Targets::new()
            .with_default(level)
            .with_target("ort", LevelFilter::WARN)
    };
    let (file_layer, guard) = match mudra_config::logs_dir() {
        Ok(dir) if std::fs::create_dir_all(&dir).is_ok() => {
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("mudra")
                .filename_suffix("log")
                .max_log_files(14)
                .build(&dir);
            match appender {
                Ok(appender) => {
                    let (writer, guard) = tracing_appender::non_blocking(appender);
                    let layer = tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(writer);
                    (Some(layer.with_filter(targets())), Some(guard))
                }
                Err(_) => (None, None),
            }
        }
        _ => (None, None),
    };
    let stderr_layer = stderr.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .without_time()
            .with_filter(
                Targets::new()
                    .with_default(LevelFilter::WARN.min(level))
                    .with_target("ort", LevelFilter::ERROR),
            )
    });
    let _ = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
    guard
}

/// Boîte de message d'erreur, pour les échecs de l'application sans console.
pub fn error_box(message: &str) {
    // SAFETY: chaînes UTF-16 valides pendant l'appel.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("Mudra"),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Garde d'instance unique (EF-74) : mutex nommé, détenu jusqu'à la fin du processus.
pub struct SingleInstance(#[allow(dead_code)] HANDLE);

impl SingleInstance {
    /// Renvoie `None` si une autre instance de Mudra tourne déjà dans la session.
    pub fn acquire() -> Option<Self> {
        // SAFETY: création d'un mutex nommé ; GetLastError distingue l'existant.
        unsafe {
            let handle = CreateMutexW(None, true, w!("Local\\Mudra.SingleInstance")).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return None;
            }
            Some(Self(handle))
        }
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Lancement à l'ouverture de session (EF-74) : ajoute ou retire la valeur
/// `Mudra` de la clé `Run` de l'utilisateur. Aucun droit administrateur requis.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = HSTRING::from(format!("\"{}\"", exe.display()));
    // SAFETY: clé ouverte puis fermée ; tampons valides pendant les appels.
    unsafe {
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status.is_err() {
            return Err(format!("ouverture de la clé Run : {status:?}"));
        }
        let result = if enabled {
            let bytes =
                std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), (command.len() + 1) * 2);
            RegSetValueExW(key, w!("Mudra"), None, REG_SZ, Some(bytes))
        } else {
            RegDeleteValueW(key, w!("Mudra"))
        };
        let _ = RegCloseKey(key);
        // Retirer une valeur absente n'est pas une erreur.
        if result.is_err() && enabled {
            return Err(format!("écriture de la clé Run : {result:?}"));
        }
    }
    Ok(())
}

/// Ouvre un dossier ou un fichier dans l'Explorateur / l'application associée.
pub fn open_path(path: &Path) {
    let _ = std::fs::create_dir_all(if path.extension().is_some() {
        path.parent().unwrap_or(path)
    } else {
        path
    });
    // SAFETY: chaîne UTF-16 valide pendant l'appel.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path.as_os_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Heure locale `HH:MM:SS`, pour le journal de l'aperçu.
pub fn clock() -> String {
    // SAFETY: lecture de l'heure système.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

fn hwnd_of(window: &winit::window::Window) -> Option<windows::Win32::Foundation::HWND> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => {
            Some(windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _))
        }
        _ => None,
    }
}

/// Prépare la fenêtre d'overlay (EF-37) : jamais activée, absente de la barre
/// des tâches et d'Alt+Tab, légèrement translucide, coins arrondis.
pub fn make_overlay(window: &winit::window::Window) {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    let Some(hwnd) = hwnd_of(window) else { return };
    let size = window.inner_size();
    let radius = (14.0 * window.scale_factor()) as i32;
    // SAFETY: handle d'une fenêtre vivante de ce thread ; la région est cédée à la fenêtre.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let extra =
            (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT).0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | extra);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 238, LWA_ALPHA);
        let region = CreateRoundRectRgn(
            0,
            0,
            size.width as i32 + 1,
            size.height as i32 + 1,
            radius,
            radius,
        );
        SetWindowRgn(hwnd, Some(region), true);
    }
}

/// Affiche ou masque une fenêtre sans jamais lui donner le focus.
pub fn show_no_activate(window: &winit::window::Window, visible: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SetWindowPos, ShowWindow,
    };
    let Some(hwnd) = hwnd_of(window) else { return };
    // SAFETY: handle d'une fenêtre vivante de ce thread.
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        if visible {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
    }
}
