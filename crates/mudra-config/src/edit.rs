//! Modification ciblée des bindings dans le fichier TOML, pour l'onglet
//! « Gestes » de l'interface. Le reste du fichier (commentaires, ordre, mise en
//! forme) est conservé tel quel.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::schema::Binding;

/// Une modification de la liste `[[bindings]]`. Les indices sont ceux de
/// `Config::bindings`.
#[derive(Debug, Clone, PartialEq)]
pub enum BindingEdit {
    Add(Binding),
    Replace(usize, Binding),
    Remove(usize),
    SetEnabled(usize, bool),
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
    fn file_without_bindings_gets_its_first_one() {
        let text = apply_binding_edit("", &BindingEdit::Add(sample())).unwrap();
        assert_eq!(config(&text).bindings, vec![sample()]);
    }
}
