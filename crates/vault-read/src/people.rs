use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PersonItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

pub type Person = PersonItem;

#[derive(Debug, Default, Deserialize)]
struct PeopleFile {
    #[serde(default)]
    people: Vec<Person>,
}

/// A missing or unparseable `people.json` is an empty registry — speaker labels fall
/// back to the raw hint value, mirroring the desktop store's tolerance.
pub fn read_people(vault: &Path) -> Vec<Person> {
    let raw = match std::fs::read(vault.join(paths::people_path())) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    serde_json::from_slice::<PeopleFile>(&raw)
        .map(|file| file.people)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_corrupt_people_file_is_empty() {
        let vault = tempfile::tempdir().unwrap();
        assert!(read_people(vault.path()).is_empty());

        std::fs::write(vault.path().join("people.json"), "not json").unwrap();
        assert!(read_people(vault.path()).is_empty());
    }

    #[test]
    fn reads_people_with_optional_names() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::write(
            vault.path().join("people.json"),
            serde_json::json!({
                "people": [
                    {"id": "bob_peters", "name": "Bob Peters"},
                    {"id": "anon"},
                ],
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            read_people(vault.path()),
            vec![
                Person {
                    id: "bob_peters".to_string(),
                    name: "Bob Peters".to_string(),
                },
                Person {
                    id: "anon".to_string(),
                    name: String::new(),
                },
            ]
        );
    }
}
