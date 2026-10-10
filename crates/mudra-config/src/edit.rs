//! Modification ciblée du fichier TOML, pour l'interface : bindings,
//! activation, contrôles continus, caméra. Le reste du fichier (commentaires,
//! ordre, mise en forme) est conservé tel quel.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::schema::{ActivationMode, Binding, DeviceSelector};

/// Une modification de la liste `[[bindings]]`. Les indices sont ceux de
/// `Config::bindings`.
#[derive(Debug, Clone, PartialEq)]
pub enum BindingEdit {
    Add(Binding),
    Replace(usize, Binding),
    Remove(usize),
    SetEnabled(usize, bool),
}

/// Une modification de la table `[activation]`.
#[derive(Debug, Clone, PartialEq)]
pub enum ActivationEdit {
    Mode(ActivationMode),
    WakeGesture(String),
    LockGesture(String),
}

/// Une modification d'une table `[[continuous]]`.
#[derive(Debug, Clone, PartialEq)]
pub enum ContinuousEdit {
    SetEnabled(bool),
    EnterGesture(String),
}

/// Une modification du fichier de configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigEdit {
    Binding(BindingEdit),
    Activation(ActivationEdit),
    /// L'indice est celui de `Config::continuous`.
    Continuous(usize, ContinuousEdit),
    /// Caméra à utiliser (`camera.device`).
    Camera(DeviceSelector),
}

/// Remplace une valeur en gardant le commentaire qui la suit sur sa ligne.
fn set(table: &mut Table, key: &str, new: impl Into<Value>) {
    let mut new = new.into();
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        None => {
            table.insert(key, Item::Value(new));
        }
    }
}

fn apply_camera_edit(doc: &mut DocumentMut, device: &DeviceSelector) -> Result<(), String> {
    if doc.get("camera").is_none() {
        doc.insert("camera", Item::Table(Table::new()));
    }
    let table = doc
        .get_mut("camera")
        .and_then(Item::as_table_mut)
        .ok_or("la caméra doit être écrite sous la forme [camera]")?;
    match device {
        DeviceSelector::Auto => set(table, "device", "auto"),
        DeviceSelector::Index(i) => set(table, "device", i64::from(*i)),
        DeviceSelector::Name(name) => set(table, "device", name.as_str()),
    }
    Ok(())
}

fn apply_activation_edit(doc: &mut DocumentMut, edit: &ActivationEdit) -> Result<(), String> {
    if doc.get("activation").is_none() {
        doc.insert("activation", Item::Table(Table::new()));
    }
    let table = doc
        .get_mut("activation")
        .and_then(Item::as_table_mut)
        .ok_or("l'activation doit être écrite sous la forme [activation]")?;
    match edit {
        ActivationEdit::Mode(mode) => {
            let mode = match mode {
                ActivationMode::WakeGesture => "wake_gesture",
                ActivationMode::AlwaysOn => "always_on",
            };
            set(table, "mode", mode);
        }
        ActivationEdit::WakeGesture(gesture) => set(table, "wake_gesture", gesture.as_str()),
        ActivationEdit::LockGesture(gesture) => set(table, "lock_gesture", gesture.as_str()),
    }
    Ok(())
}

fn apply_continuous_edit(
    doc: &mut DocumentMut,
    index: usize,
    edit: &ContinuousEdit,
) -> Result<(), String> {
    let table = doc
        .get_mut("continuous")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|tables| tables.get_mut(index))
        .ok_or_else(|| format!("contrôle continu {index} introuvable dans le fichier"))?;
    match edit {
        ContinuousEdit::SetEnabled(true) => {
            table.remove("enabled");
        }
        ContinuousEdit::SetEnabled(false) => set(table, "enabled", false),
        ContinuousEdit::EnterGesture(gesture) => set(table, "enter_gesture", gesture.as_str()),
    }
    Ok(())
}

/// Applique `edit` au texte TOML et renvoie le nouveau texte.
pub fn apply_edit(text: &str, edit: &ConfigEdit) -> Result<String, String> {
    match edit {
        ConfigEdit::Binding(edit) => apply_binding_edit(text, edit),
        ConfigEdit::Activation(edit) => {
            let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
            apply_activation_edit(&mut doc, edit)?;
            Ok(doc.to_string())
        }
        ConfigEdit::Continuous(index, edit) => {
            let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
            apply_continuous_edit(&mut doc, *index, edit)?;
            Ok(doc.to_string())
        }
        ConfigEdit::Camera(device) => {
            let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
            apply_camera_edit(&mut doc, device)?;
            Ok(doc.to_string())
        }
    }
}

/// Contenu d'une table `[[bindings]]`, l'action sur une seule ligne.
fn binding_items(binding: &Binding) -> Result<Vec<(String, Item)>, String> {
    let text = toml::to_string(binding).map_err(|e| e.to_string())?;
    let doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let mut items = Vec::new();
    for (key, item) in doc.as_table().iter() {
        let item = match item.clone() {
            Item::Table(table) => Item::Value(Value::InlineTable(table.into_inline_table())),
            other => other,
        };
        items.push((key.to_string(), item));
    }
    Ok(items)
}

fn fill(table: &mut Table, binding: &Binding) -> Result<(), String> {
    table.clear();
    for (key, item) in binding_items(binding)? {
        table.insert(&key, item);
    }
    Ok(())
}

/// Applique `edit` au texte TOML et renvoie le nouveau texte.
pub fn apply_binding_edit(text: &str, edit: &BindingEdit) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    if doc.get("bindings").is_none() {
        doc.insert("bindings", Item::ArrayOfTables(Default::default()));
    }
    let bindings = doc
        .get_mut("bindings")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or("les bindings doivent être écrits sous la forme [[bindings]]")?;
    let missing = |i: &usize| format!("binding {i} introuvable dans le fichier");
    match edit {
        BindingEdit::Add(binding) => {
            let mut table = Table::new();
            fill(&mut table, binding)?;
            bindings.push(table);
        }
        BindingEdit::Replace(i, binding) => {
            let table = bindings.get_mut(*i).ok_or_else(|| missing(i))?;
            fill(table, binding)?;
        }
        BindingEdit::Remove(i) => {
            if *i >= bindings.len() {
                return Err(missing(i));
            }
            bindings.remove(*i);
        }
        BindingEdit::SetEnabled(i, enabled) => {
            let table = bindings.get_mut(*i).ok_or_else(|| missing(i))?;
            if *enabled {
                table.remove("enabled");
            } else {
                table.insert("enabled", toml_edit::value(false));
            }
        }
    }
    Ok(doc.to_string())
}

/// Écrit le fichier d'un seul coup : un fichier temporaire, puis un renommage.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Action, HandFilter, MediaKey};
    use crate::{DEFAULT_CONFIG_TOML, parse};

    fn config(text: &str) -> crate::Config {
        parse(text, Path::new("test.toml")).expect("TOML valide")
    }

    fn comments(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .collect()
    }

    fn sample() -> Binding {
        let mut binding = config(DEFAULT_CONFIG_TOML).bindings[0].clone();
        binding.gesture = "rock".into();
        binding.hand = HandFilter::Left;
        binding.repeat_ms = None;
        binding.action = Action::Media {
            key: MediaKey::Stop,
        };
        binding
    }

    #[test]
    fn add_appends_and_keeps_the_rest_of_the_file() {
        let before = config(DEFAULT_CONFIG_TOML);
        let text = apply_binding_edit(DEFAULT_CONFIG_TOML, &BindingEdit::Add(sample())).unwrap();
        let after = config(&text);
        assert_eq!(after.bindings.len(), before.bindings.len() + 1);
        assert_eq!(after.bindings.last(), Some(&sample()));
        assert_eq!(after.bindings[..before.bindings.len()], before.bindings[..]);
        assert_eq!(after.continuous, before.continuous);
        assert_eq!(after.inference, before.inference);
        assert_eq!(comments(&text), comments(DEFAULT_CONFIG_TOML));
    }

    #[test]
    fn replace_changes_only_the_target() {
        let before = config(DEFAULT_CONFIG_TOML);
        let text =
            apply_binding_edit(DEFAULT_CONFIG_TOML, &BindingEdit::Replace(2, sample())).unwrap();
        let after = config(&text);
        assert_eq!(after.bindings.len(), before.bindings.len());
        for (i, (a, b)) in after.bindings.iter().zip(&before.bindings).enumerate() {
            if i == 2 {
                assert_eq!(a, &sample());
            } else {
                assert_eq!(a, b);
            }
        }
        assert_eq!(after.continuous, before.continuous);
    }

    #[test]
    fn remove_and_toggle() {
        let before = config(DEFAULT_CONFIG_TOML);
        let text = apply_binding_edit(DEFAULT_CONFIG_TOML, &BindingEdit::Remove(0)).unwrap();
        assert_eq!(config(&text).bindings[..], before.bindings[1..]);

        let off =
            apply_binding_edit(DEFAULT_CONFIG_TOML, &BindingEdit::SetEnabled(1, false)).unwrap();
        let after = config(&off);
        assert!(!after.bindings[1].enabled);
        assert!(after.bindings[0].enabled);
        let on = apply_binding_edit(&off, &BindingEdit::SetEnabled(1, true)).unwrap();
        assert_eq!(config(&on), before);

        assert!(apply_binding_edit(DEFAULT_CONFIG_TOML, &BindingEdit::Remove(99)).is_err());
    }

    #[test]
    fn camera_edit_writes_a_name_an_index_or_auto() {
        let before = config(DEFAULT_CONFIG_TOML);
        let mut text = DEFAULT_CONFIG_TOML.to_string();
        for device in [
            DeviceSelector::Name("HD Webcam \"Pro\"".into()),
            DeviceSelector::Index(1),
            DeviceSelector::Auto,
        ] {
            text = apply_edit(&text, &ConfigEdit::Camera(device.clone())).unwrap();
            let after = config(&text);
            assert_eq!(after.camera.device, device);
            assert_eq!(after.camera.fps, before.camera.fps);
            assert_eq!(after.bindings, before.bindings);
            assert_eq!(comments(&text), comments(DEFAULT_CONFIG_TOML));
        }
        // Un fichier sans table [camera] en reçoit une.
        let text = apply_edit("", &ConfigEdit::Camera(DeviceSelector::Index(2))).unwrap();
        assert_eq!(config(&text).camera.device, DeviceSelector::Index(2));
    }

    #[test]
    fn activation_edits_keep_comments_and_everything_else() {
        let before = config(DEFAULT_CONFIG_TOML);
        let edits = [
            ActivationEdit::WakeGesture("rock".into()),
            ActivationEdit::LockGesture("v_sign".into()),
            ActivationEdit::Mode(ActivationMode::AlwaysOn),
        ];
        let mut text = DEFAULT_CONFIG_TOML.to_string();
        for edit in edits {
            text = apply_edit(&text, &ConfigEdit::Activation(edit)).unwrap();
        }
        let after = config(&text);
        assert_eq!(after.activation.wake_gesture, "rock");
        assert_eq!(after.activation.lock_gesture, "v_sign");
        assert_eq!(after.activation.mode, ActivationMode::AlwaysOn);
        assert_eq!(
            after.activation.wake_hold_ms,
            before.activation.wake_hold_ms
        );
        assert_eq!(after.bindings, before.bindings);
        assert_eq!(after.continuous, before.continuous);
        // Le commentaire de la ligne modifiée reste à sa place.
        assert!(text.contains("# wake_gesture | always_on"), "{text}");
        assert_eq!(comments(&text), comments(DEFAULT_CONFIG_TOML));

        // Un fichier sans table [activation] en reçoit une.
        let text = apply_edit(
            "",
            &ConfigEdit::Activation(ActivationEdit::Mode(ActivationMode::AlwaysOn)),
        )
        .unwrap();
        assert_eq!(config(&text).activation.mode, ActivationMode::AlwaysOn);
    }

    #[test]
    fn continuous_edits_target_one_control() {
        let before = config(DEFAULT_CONFIG_TOML);
        let edit = |text: &str, index, edit| {
            apply_edit(text, &ConfigEdit::Continuous(index, edit)).unwrap()
        };
        let off = edit(DEFAULT_CONFIG_TOML, 0, ContinuousEdit::SetEnabled(false));
        let after = config(&off);
        assert!(!after.continuous[0].enabled);
        assert!(after.continuous[1].enabled);
        assert_eq!(after.continuous[0].control, before.continuous[0].control);
        assert_eq!(
            config(&edit(&off, 0, ContinuousEdit::SetEnabled(true))),
            before
        );

        let moved = edit(
            DEFAULT_CONFIG_TOML,
            1,
            ContinuousEdit::EnterGesture("rock".into()),
        );
        let after = config(&moved);
        assert_eq!(after.continuous[1].enter_gesture, "rock");
        assert_eq!(after.continuous[0], before.continuous[0]);
        assert_eq!(after.continuous[1].control, before.continuous[1].control);
        assert_eq!(comments(&moved), comments(DEFAULT_CONFIG_TOML));

        let missing = ConfigEdit::Continuous(9, ContinuousEdit::SetEnabled(false));
        assert!(apply_edit(DEFAULT_CONFIG_TOML, &missing).is_err());
    }

    #[test]
    fn file_without_bindings_gets_its_first_one() {
        let text = apply_binding_edit("", &BindingEdit::Add(sample())).unwrap();
        assert_eq!(config(&text).bindings, vec![sample()]);
    }
}
